//! `freight dap` — Debug Adapter Protocol server for Freight editors.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use freight_core::build::{build_project_with, BuildOutput};
use freight_core::event::silent;
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::{detect_debuggers, load_debugger_templates, GlobalConfig};
use serde_json::{json, Value};

#[derive(clap::Args)]
pub struct Args {}

impl Args {
    pub fn run(self) {
        if let Err(err) = DapServer::new().run() {
            eprintln!("freight dap: {err}");
        }
    }
}

struct DapServer {
    seq: i64,
    requests: Receiver<Value>,
    output_rx: Receiver<(String, String)>,
    output_tx: mpsc::Sender<(String, String)>,
    stdout: std::io::Stdout,
    run_process: Option<Child>,
    gdb: Option<Child>,
    gdb_stdin: Option<ChildStdin>,
    gdb_rx: Option<Receiver<String>>,
    gdb_token: u64,
    breakpoints: HashMap<String, Vec<u64>>,
    breakpoints_applied: bool,
    debug_started: bool,
    configuration_done: bool,
}

impl DapServer {
    fn new() -> Self {
        let (output_tx, output_rx) = mpsc::channel();
        Self {
            seq: 1,
            requests: spawn_dap_reader(),
            output_rx,
            output_tx,
            stdout: std::io::stdout(),
            run_process: None,
            gdb: None,
            gdb_stdin: None,
            gdb_rx: None,
            gdb_token: 1,
            breakpoints: HashMap::new(),
            breakpoints_applied: false,
            debug_started: false,
            configuration_done: false,
        }
    }

    fn run(&mut self) -> anyhow::Result<()> {
        loop {
            self.drain_debugger_events();
            let request = match self.requests.recv_timeout(Duration::from_millis(50)) {
                Ok(request) => request,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let command = request["command"].as_str().unwrap_or("");
            match command {
                "initialize" => self.handle_initialize(&request)?,
                "launch" => self.handle_launch(&request)?,
                "setBreakpoints" => self.handle_set_breakpoints(&request)?,
                "configurationDone" => self.handle_configuration_done(&request)?,
                "threads" => self.handle_threads(&request)?,
                "stackTrace" => self.handle_stack_trace(&request)?,
                "scopes" => self.response(
                    &request,
                    json!({ "scopes": [{ "name": "Locals", "variablesReference": 1, "expensive": false }] }),
                )?,
                "variables" => self.handle_variables(&request)?,
                "evaluate" => self.handle_evaluate(&request)?,
                "continue" => self.handle_exec(&request, "-exec-continue", json!({ "allThreadsContinued": true }))?,
                "next" => self.handle_exec(&request, "-exec-next", json!({}))?,
                "stepIn" => self.handle_exec(&request, "-exec-step", json!({}))?,
                "stepOut" => self.handle_exec(&request, "-exec-finish", json!({}))?,
                "pause" => self.handle_exec(&request, "-exec-interrupt", json!({}))?,
                "disconnect" | "terminate" => {
                    self.shutdown();
                    self.response(&request, json!({}))?;
                    self.event("terminated", json!({}))?;
                    break;
                }
                _ => self.response(&request, json!({}))?,
            }
        }
        self.shutdown();
        Ok(())
    }

    fn handle_initialize(&mut self, request: &Value) -> anyhow::Result<()> {
        self.response(
            request,
            json!({
                "supportsConfigurationDoneRequest": true,
                "supportsTerminateRequest": true,
                "supportsEvaluateForHovers": true,
                "supportsSetVariable": false,
                "supportsStepBack": false,
            }),
        )
    }

    fn handle_launch(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
        if let Some(cwd) = config["cwd"].as_str() {
            if !cwd.is_empty() {
                std::env::set_current_dir(cwd)?;
            }
        }

        if config["mode"].as_str().unwrap_or("run") == "debug" {
            match self.launch_debug(&config) {
                Ok(()) => {
                    self.event("initialized", json!({}))?;
                    self.response(request, json!({}))?;
                    if self.configuration_done {
                        self.start_debuggee()?;
                    }
                }
                Err(err) => {
                    self.output(&format!("{err}\n"), "stderr")?;
                    self.error_response(request, &err.to_string())?;
                    self.event("terminated", json!({}))?;
                }
            }
            return Ok(());
        }

        match self.launch_run(&config) {
            Ok(()) => {
                self.event("initialized", json!({}))?;
                self.response(request, json!({}))?;
            }
            Err(err) => {
                self.output(&format!("{err}\n"), "stderr")?;
                self.error_response(request, &err.to_string())?;
                self.event("terminated", json!({}))?;
            }
        }
        Ok(())
    }

    fn launch_run(&mut self, config: &Value) -> anyhow::Result<()> {
        let freight = std::env::current_exe()?;
        let args = freight_run_args(config);
        self.output(
            &format!("$ {} {}\n", freight.display(), shell_words(&args)),
            "console",
        )?;
        let mut child = Command::new(freight)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(stdout) = child.stdout.take() {
            spawn_process_output(stdout, "stdout", self.output_tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_process_output(stderr, "stderr", self.output_tx.clone());
        }
        self.run_process = Some(child);
        Ok(())
    }

    fn launch_debug(&mut self, config: &Value) -> anyhow::Result<()> {
        let project_dir = find_project_dir()?;
        let manifest = load_manifest(&project_dir)?;
        let mut global_cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            global_cfg.apply_local(local);
        }
        let debuggers = detect_debuggers(&load_debugger_templates());
        if debuggers.is_empty() {
            anyhow::bail!(
                "no debugger found on PATH; install gdb (≥14 for native DAP) or lldb (with lldb-dap)"
            );
        }

        let debugger_pref = config["debugger"]
            .as_str()
            .or(global_cfg.default_debugger.as_deref());
        let debugger = if let Some(pref) = debugger_pref {
            debuggers
                .iter()
                .find(|d| d.template.name == pref)
                .ok_or_else(|| anyhow::anyhow!("debugger '{pref}' not found on PATH"))?
        } else {
            &debuggers[0]
        };

        let features = string_array(&config["features"]);
        let output = build_project_with(
            "debug",
            &features,
            !config["noDefaultFeatures"].as_bool().unwrap_or(false),
            &[],
            &silent(),
        )?;
        let binary = select_binary(&output, &project_dir, config["bin"].as_str(), &manifest)?;

        // Decide on passthrough vs MI2 bridge.
        match debugger.template.name.as_str() {
            "gdb" => {
                let (major, _minor) = gdb_version(&debugger.path);
                if major >= 14 {
                    self.output(
                        &format!(
                            "Debugging {} with {} {} (native DAP)\n",
                            binary.display(),
                            debugger.template.name,
                            debugger.version
                        ),
                        "console",
                    )?;
                    self.run_passthrough(
                        &debugger.path,
                        &["--interpreter=dap"],
                        &binary,
                        config,
                    )
                } else {
                    self.output(
                        &format!(
                            "Debugging {} with {} {} (GDB/MI2 bridge)\n",
                            binary.display(),
                            debugger.template.name,
                            debugger.version
                        ),
                        "console",
                    )?;
                    self.launch_gdb_mi2(&debugger.path, &binary, config)
                }
            }
            "lldb" => {
                if let Some(dap_bin) = find_lldb_dap(&debugger.path) {
                    self.output(
                        &format!(
                            "Debugging {} with {} (native DAP)\n",
                            binary.display(),
                            dap_bin.display()
                        ),
                        "console",
                    )?;
                    self.run_passthrough(&dap_bin, &[], &binary, config)
                } else {
                    anyhow::bail!(
                        "lldb-dap / lldb-vscode not found; install lldb-dap alongside lldb for DAP support"
                    );
                }
            }
            other => {
                anyhow::bail!(
                    "debugger '{other}' has no Freight DAP backend; supported: gdb (≥14 native, older via MI2), lldb (with lldb-dap)"
                );
            }
        }
    }

    /// Passthrough mode: build, then proxy all DAP messages to the adapter subprocess,
    /// injecting `arguments.program` into the `launch` request.
    fn run_passthrough(
        &mut self,
        adapter_bin: &Path,
        adapter_args: &[&str],
        binary: &Path,
        _config: &Value,
    ) -> anyhow::Result<()> {
        // Collect all buffered requests up to (and including) `launch`, inject
        // `program`, then forward them to the adapter.  After `launch` we enter
        // a simple relay loop.

        // Drain any requests that arrived before we were called (initialize, …).
        // We cannot do that here because we don't own `self.requests` in a
        // reentrant way – instead we run the adapter subprocess and do the relay
        // synchronously on this thread, returning only when the adapter exits.

        let mut child = Command::new(adapter_bin)
            .args(adapter_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let mut adapter_stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("adapter stdin unavailable"))?;
        let adapter_stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("adapter stdout unavailable"))?;

        // Channel: adapter stdout bytes → our stdout
        let (adapter_out_tx, adapter_out_rx) = mpsc::channel::<Vec<u8>>();
        {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(adapter_stdout);
                loop {
                    match read_dap_frame(&mut reader) {
                        Some(frame) => {
                            // Re-wrap with Content-Length header
                            let header = format!("Content-Length: {}\r\n\r\n", frame.len());
                            let mut msg = header.into_bytes();
                            msg.extend_from_slice(&frame);
                            if adapter_out_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            });
        }

        let binary_str = binary.to_string_lossy().into_owned();
        let mut launch_forwarded = false;

        loop {
            // Forward any bytes from adapter stdout to our stdout.
            while let Ok(bytes) = adapter_out_rx.try_recv() {
                self.stdout.write_all(&bytes)?;
                self.stdout.flush()?;
            }

            // Read next request from VS Code (with a short timeout so we can
            // keep draining adapter output).
            let mut request = match self.requests.recv_timeout(Duration::from_millis(20)) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Check if adapter has exited.
                    if let Ok(Some(_)) = child.try_wait() {
                        break;
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            // Inject `program` into the launch request.
            if request["command"].as_str() == Some("launch") && !launch_forwarded {
                launch_forwarded = true;
                if let Some(args) = request["arguments"].as_object_mut() {
                    args.insert("program".to_string(), Value::String(binary_str.clone()));
                }
            }

            write_dap(&mut adapter_stdin, &request)?;

            // If this was `disconnect` / `terminate`, break after forwarding.
            match request["command"].as_str().unwrap_or("") {
                "disconnect" | "terminate" => break,
                _ => {}
            }
        }

        // Drain remaining adapter output.
        let _ = child.wait();
        while let Ok(bytes) = adapter_out_rx.try_recv() {
            let _ = self.stdout.write_all(&bytes);
        }
        let _ = self.stdout.flush();
        Ok(())
    }

    /// GDB/MI2 bridge (fallback for GDB < 14).
    fn launch_gdb_mi2(
        &mut self,
        gdb_path: &Path,
        binary: &Path,
        config: &Value,
    ) -> anyhow::Result<()> {
        let mut child = Command::new(gdb_path)
            .args(["--interpreter=mi2", "--quiet"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("gdb stdout unavailable"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("gdb stderr unavailable"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("gdb stdin unavailable"))?;
        self.gdb_rx = Some(spawn_gdb_reader(stdout));
        spawn_process_output(stderr, "stderr", self.output_tx.clone());
        self.gdb_stdin = Some(stdin);
        self.gdb = Some(child);
        self.gdb_command("-gdb-set target-async on")?;
        self.gdb_command("-gdb-set breakpoint pending on")?;
        self.gdb_command(&format!(
            "-file-exec-and-symbols {}",
            mi_quote(&binary.display().to_string())
        ))?;
        let program_args = string_array(&config["args"]);
        if !program_args.is_empty() {
            self.gdb_command(&format!(
                "-exec-arguments {}",
                program_args
                    .iter()
                    .map(|arg| mi_quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
            ))?;
        }
        self.apply_breakpoints()?;
        Ok(())
    }

    fn handle_set_breakpoints(&mut self, request: &Value) -> anyhow::Result<()> {
        let args = &request["arguments"];
        let path = args["source"]["path"].as_str().unwrap_or("").to_string();
        let requested = args["breakpoints"].as_array().cloned().unwrap_or_default();
        let lines = requested
            .iter()
            .filter_map(|bp| bp["line"].as_u64())
            .collect::<Vec<_>>();
        if !path.is_empty() {
            self.breakpoints.insert(path.clone(), lines);
            // Clear the flag so apply_breakpoints will re-apply everything.
            self.breakpoints_applied = false;
            if self.gdb.is_some() {
                self.apply_breakpoints()?;
            }
        }
        self.response(
            request,
            json!({
                "breakpoints": requested.iter().map(|bp| {
                    json!({
                        "verified": true,
                        "source": args["source"].clone(),
                        "line": bp["line"].clone(),
                    })
                }).collect::<Vec<_>>()
            }),
        )
    }

    fn handle_configuration_done(&mut self, request: &Value) -> anyhow::Result<()> {
        self.configuration_done = true;
        self.response(request, json!({}))?;
        self.start_debuggee()
    }

    fn handle_exec(&mut self, request: &Value, command: &str, body: Value) -> anyhow::Result<()> {
        if let Err(err) = self.gdb_command(command) {
            self.output(&format!("{err}\n"), "stderr")?;
        }
        self.response(request, body)
    }

    fn handle_threads(&mut self, request: &Value) -> anyhow::Result<()> {
        if self.gdb.is_none() {
            return self.response(
                request,
                json!({ "threads": [{ "id": 1, "name": "main" }] }),
            );
        }

        let threads = match self.gdb_command("-thread-list-ids") {
            Ok(line) => {
                // Parse: ^done,thread-ids={thread-id="1",thread-id="2",...},number-of-threads="N"
                let ids = mi_parse_list_value(&line, "thread-id");
                if ids.is_empty() {
                    vec![json!({ "id": 1, "name": "main" })]
                } else {
                    ids.iter()
                        .filter_map(|m| {
                            let id_str = m.get("thread-id")?;
                            let id: u64 = id_str.parse().ok()?;
                            Some(json!({ "id": id, "name": format!("thread {id}") }))
                        })
                        .collect()
                }
            }
            Err(_) => vec![json!({ "id": 1, "name": "main" })],
        };

        self.response(request, json!({ "threads": threads }))
    }

    fn handle_stack_trace(&mut self, request: &Value) -> anyhow::Result<()> {
        let line = self.gdb_command("-stack-list-frames")?;
        let frames = parse_stack_frames(&line);
        let total = frames.len();
        self.response(
            request,
            json!({
                "stackFrames": frames,
                "totalFrames": total,
            }),
        )
    }

    fn handle_variables(&mut self, request: &Value) -> anyhow::Result<()> {
        let line = self.gdb_command("-stack-list-variables --simple-values")?;
        self.response(request, json!({ "variables": parse_variables(&line) }))
    }

    fn handle_evaluate(&mut self, request: &Value) -> anyhow::Result<()> {
        let expression = request["arguments"]["expression"].as_str().unwrap_or("").trim();
        if expression.is_empty() {
            return self.response(request, json!({ "result": "", "variablesReference": 0 }));
        }
        match self.gdb_command(&format!("-data-evaluate-expression {}", mi_quote(expression))) {
            Ok(line) => {
                let value = mi_field(&line, "value").unwrap_or_else(|| "<unavailable>".into());
                self.response(request, json!({ "result": value, "variablesReference": 0 }))
            }
            Err(err) => self.response(
                request,
                json!({ "result": format!("<error: {err}>"), "variablesReference": 0 }),
            ),
        }
    }

    fn start_debuggee(&mut self) -> anyhow::Result<()> {
        if self.gdb.is_none() || self.debug_started {
            return Ok(());
        }
        self.debug_started = true;
        if let Err(err) = self.gdb_command("-exec-run") {
            self.output(&format!("{err}\n"), "stderr")?;
            self.event("terminated", json!({}))?;
        }
        Ok(())
    }

    fn apply_breakpoints(&mut self) -> anyhow::Result<()> {
        if self.gdb.is_none() || self.breakpoints_applied {
            return Ok(());
        }
        let _ = self.gdb_command("-break-delete");
        for (file, lines) in self.breakpoints.clone() {
            for line in lines {
                if let Err(err) = self.gdb_command(&format!(
                    "-break-insert {}",
                    mi_quote(&format!("{file}:{line}"))
                )) {
                    self.output(&format!("{err}\n"), "stderr")?;
                }
            }
        }
        self.breakpoints_applied = true;
        Ok(())
    }

    fn gdb_command(&mut self, command: &str) -> anyhow::Result<String> {
        let token = self.gdb_token;
        self.gdb_token += 1;
        let stdin = self
            .gdb_stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("gdb is not running"))?;
        writeln!(stdin, "{token}{command}")?;
        stdin.flush()?;

        loop {
            let line = self
                .gdb_rx
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("gdb output unavailable"))?
                .recv_timeout(Duration::from_secs(30))?;
            if let Some(result) = self.handle_gdb_line(&line, Some(token))? {
                return Ok(result);
            }
        }
    }

    fn drain_debugger_events(&mut self) {
        while let Ok((category, output)) = self.output_rx.try_recv() {
            let _ = self.output(&output, &category);
        }
        while let Some(line) = self.gdb_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            let _ = self.handle_gdb_line(&line, None);
        }
        if let Some(child) = self.run_process.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let _ = self.event("exited", json!({ "exitCode": status.code().unwrap_or(0) }));
                let _ = self.event("terminated", json!({}));
                self.run_process = None;
            }
        }
        if let Some(child) = self.gdb.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let _ = self.event("exited", json!({ "exitCode": status.code().unwrap_or(0) }));
                let _ = self.event("terminated", json!({}));
                self.gdb = None;
                self.gdb_stdin = None;
            }
        }
    }

    fn handle_gdb_line(&mut self, line: &str, waiting_for: Option<u64>) -> anyhow::Result<Option<String>> {
        if let Some(rest) = line.strip_prefix('~') {
            self.output(&mi_c_string(rest), "console")?;
            return Ok(None);
        }
        if let Some(rest) = line.strip_prefix('&') {
            self.output(&mi_c_string(rest), "stderr")?;
            return Ok(None);
        }
        if line.starts_with("*stopped") {
            if line.contains("exited") {
                self.event("terminated", json!({}))?;
            } else {
                self.event(
                    "stopped",
                    json!({
                        "reason": stopped_reason(line),
                        "threadId": 1,
                        "allThreadsStopped": true,
                    }),
                )?;
            }
            return Ok(None);
        }
        let Some((token, result)) = parse_mi_result(line) else {
            return Ok(None);
        };
        if waiting_for != Some(token) {
            return Ok(None);
        }
        if result == "error" {
            anyhow::bail!("{}", mi_field(line, "msg").unwrap_or_else(|| line.to_string()));
        }
        Ok(Some(line.to_string()))
    }

    fn response(&mut self, request: &Value, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        self.write(json!({
            "type": "response",
            "seq": seq,
            "request_seq": request["seq"],
            "success": true,
            "command": request["command"],
            "body": body,
        }))
    }

    fn error_response(&mut self, request: &Value, message: &str) -> anyhow::Result<()> {
        let seq = self.next_seq();
        self.write(json!({
            "type": "response",
            "seq": seq,
            "request_seq": request["seq"],
            "success": false,
            "command": request["command"],
            "message": message,
        }))
    }

    fn event(&mut self, event: &str, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        self.write(json!({
            "type": "event",
            "seq": seq,
            "event": event,
            "body": body,
        }))
    }

    fn output(&mut self, output: &str, category: &str) -> anyhow::Result<()> {
        self.event("output", json!({ "category": category, "output": output }))
    }

    fn write(&mut self, value: Value) -> anyhow::Result<()> {
        write_dap(&mut self.stdout, &value)
    }

    fn next_seq(&mut self) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    fn shutdown(&mut self) {
        if let Some(mut stdin) = self.gdb_stdin.take() {
            let _ = writeln!(stdin, "-gdb-exit");
        }
        if let Some(mut child) = self.gdb.take() {
            let _ = child.kill();
        }
        if let Some(mut child) = self.run_process.take() {
            let _ = child.kill();
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: DAP framing
// ---------------------------------------------------------------------------

/// Write a DAP message: `Content-Length: N\r\n\r\n{json}`.
fn write_dap(out: &mut impl Write, msg: &Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    write!(out, "Content-Length: {}\r\n\r\n", bytes.len())?;
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

/// Read one DAP frame from `reader`.  Returns the raw JSON body bytes, or
/// `None` on EOF / parse error.
fn read_dap_frame(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok().filter(|n| *n > 0).is_none() {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

// ---------------------------------------------------------------------------
// Helper: debugger detection
// ---------------------------------------------------------------------------

/// Run `gdb --version` and return `(major, minor)`.  Returns `(0, 0)` on failure.
fn gdb_version(path: &Path) -> (u32, u32) {
    let out = Command::new(path)
        .arg("--version")
        .output()
        .ok();
    let text = out
        .as_ref()
        .and_then(|o| std::str::from_utf8(&o.stdout).ok().map(str::to_string))
        .unwrap_or_default();
    // "GNU gdb (…) 14.2" — find the last occurrence of a "X.Y" token on the
    // first line.
    let first_line = text.lines().next().unwrap_or("");
    for token in first_line.split_whitespace().rev() {
        let mut parts = token.split('.');
        if let (Some(maj), Some(min)) = (parts.next(), parts.next()) {
            if let (Ok(major), Ok(minor)) = (maj.parse::<u32>(), min.parse::<u32>()) {
                return (major, minor);
            }
        }
    }
    (0, 0)
}

/// Look for `lldb-dap` or `lldb-vscode` next to `lldb_path`, then on `PATH`.
fn find_lldb_dap(lldb_path: &Path) -> Option<PathBuf> {
    let dir = lldb_path.parent()?;
    for name in &["lldb-dap", "lldb-vscode"] {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // Fall back to PATH.
    for name in &["lldb-dap", "lldb-vscode"] {
        if let Ok(out) = Command::new("which").arg(name).output() {
            if out.status.success() {
                let p = PathBuf::from(std::str::from_utf8(&out.stdout).unwrap_or("").trim());
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// GDB/MI2 parser — recursive-descent, one-level nesting
// ---------------------------------------------------------------------------

/// Parse a GDB/MI output line and collect all occurrences of `key="{...}"` or
/// `key="{key=val,...}"` blocks, returning them as flat string maps.
///
/// This handles the common cases for `-stack-list-frames` (frame={…}) and
/// `-thread-list-ids` (thread-id="N") without breaking on nested values.
fn mi_parse_list_value(s: &str, key: &str) -> Vec<HashMap<String, String>> {
    let mut result = Vec::new();
    let needle_brace = format!("{key}={{");
    let needle_quote = format!("{key}=\"");

    // Collect brace-delimited records: key={field=val,...}
    let mut search = s;
    while let Some(pos) = search.find(&needle_brace) {
        let after = &search[pos + needle_brace.len()..];
        let (record, consumed) = parse_mi_record(after);
        result.push(record);
        search = &search[pos + needle_brace.len() + consumed..];
    }

    // Collect quoted scalar records: key="value"  (e.g. thread-id="1")
    if result.is_empty() {
        let mut search2 = s;
        while let Some(pos) = search2.find(&needle_quote) {
            let after = &search2[pos + needle_quote.len()..];
            if let Some(end) = find_string_end(after) {
                let val = mi_c_string(&format!("\"{}\"", &after[..end]));
                let mut map = HashMap::new();
                map.insert(key.to_string(), val);
                result.push(map);
                search2 = &search2[pos + needle_quote.len() + end + 1..];
            } else {
                break;
            }
        }
    }

    result
}

/// Parse a GDB/MI `{field="value",...}` record starting *after* the opening
/// `{`.  Returns the flat map and the number of bytes consumed (including the
/// closing `}`).
fn parse_mi_record(s: &str) -> (HashMap<String, String>, usize) {
    let mut map = HashMap::new();
    let mut i = 0;
    let bytes = s.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'}' {
            i += 1;
            break;
        }
        // Find the `=` that separates key from value.
        let eq = match s[i..].find('=') {
            Some(p) => i + p,
            None => break,
        };
        let field_name = s[i..eq].trim().to_string();
        i = eq + 1;

        if i >= bytes.len() {
            break;
        }

        let value = if bytes[i] == b'"' {
            // Quoted string value.
            i += 1; // skip opening quote
            let end = find_string_end(&s[i..]).unwrap_or(0);
            let val = mi_c_string(&format!("\"{}\"", &s[i..i + end]));
            i += end + 1; // skip content + closing quote
            val
        } else if bytes[i] == b'{' {
            // Nested record — skip it (we only need one level).
            let depth_start = i;
            i += 1;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    b'"' => {
                        i += 1;
                        let end = find_string_end(&s[i..]).unwrap_or(0);
                        i += end + 1;
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
            s[depth_start..i].to_string()
        } else {
            // Unquoted token (e.g. integer).
            let end = s[i..].find(|c| c == ',' || c == '}').unwrap_or(s[i..].len());
            let val = s[i..i + end].to_string();
            i += end;
            val
        };

        if !field_name.is_empty() {
            map.insert(field_name, value);
        }

        // Skip comma separator.
        if i < bytes.len() && bytes[i] == b',' {
            i += 1;
        }
    }

    (map, i)
}

/// Find the index of the unescaped closing `"` in a string that starts right
/// after the opening quote.
fn find_string_end(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(idx);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Unchanged helpers
// ---------------------------------------------------------------------------

fn spawn_dap_reader() -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            match read_dap_frame(&mut reader) {
                Some(body) => {
                    if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                        if tx.send(value).is_err() {
                            return;
                        }
                    }
                }
                None => return,
            }
        }
    });
    rx
}

fn spawn_gdb_reader(stdout: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if !line.trim().is_empty() && tx.send(line.trim().to_string()).is_err() {
                break;
            }
        }
    });
    rx
}

fn spawn_process_output(
    stdout: impl Read + Send + 'static,
    category: &'static str,
    tx: mpsc::Sender<(String, String)>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send((category.to_string(), format!("{line}\n"))).is_err() {
                break;
            }
        }
    });
}

fn find_project_dir() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    find_manifest_dir(&cwd).ok_or_else(|| anyhow::anyhow!("no freight.toml found"))
}

fn select_binary(
    output: &BuildOutput,
    project_dir: &Path,
    filter: Option<&str>,
    manifest: &freight_core::manifest::types::Manifest,
) -> anyhow::Result<PathBuf> {
    let candidates = if let Some(name) = filter {
        output
            .binaries
            .iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        output.binaries.clone()
    };
    match candidates.len() {
        0 if filter.is_some() => anyhow::bail!(
            "no binary named '{}' - available: {}",
            filter.unwrap(),
            manifest
                .bins
                .iter()
                .map(|bin| bin.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        0 => {
            let fallback = project_dir.join("target").join("debug").join(&manifest.package.name);
            if fallback.exists() {
                Ok(fallback)
            } else {
                anyhow::bail!("no binary built - does the manifest declare [[bin]]?")
            }
        }
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => anyhow::bail!(
            "multiple binaries built; set `bin` to one of: {}",
            candidates
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn freight_run_args(config: &Value) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    if config["release"].as_bool().unwrap_or(false) {
        args.push("--release".into());
    }
    if let Some(package) = config["package"].as_str() {
        args.extend(["-p".into(), package.into()]);
    }
    if let Some(bin) = config["bin"].as_str() {
        args.extend(["--bin".into(), bin.into()]);
    }
    let features = string_array(&config["features"]);
    if !features.is_empty() {
        args.extend(["--features".into(), features.join(",")]);
    }
    if config["noDefaultFeatures"].as_bool().unwrap_or(false) {
        args.push("--no-default-features".into());
    }
    let program_args = string_array(&config["args"]);
    if !program_args.is_empty() {
        args.push("--".into());
        args.extend(program_args);
    }
    args
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn shell_words(args: &[String]) -> String {
    args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.chars().all(|c| c.is_ascii_alphanumeric() || "_./:=+-".contains(c)) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn mi_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn mi_c_string(value: &str) -> String {
    serde_json::from_str::<String>(value).unwrap_or_else(|_| value.trim_matches('"').to_string())
}

fn parse_mi_result(line: &str) -> Option<(u64, &str)> {
    let caret = line.find('^')?;
    let token = line[..caret].parse().ok()?;
    let rest = &line[caret + 1..];
    let end = rest.find(',').unwrap_or(rest.len());
    Some((token, &rest[..end]))
}

fn mi_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("{field}=\"");
    let start = text.find(&needle)? + needle.len();
    let end = find_string_end(&text[start..])?;
    Some(mi_c_string(&format!("\"{}\"", &text[start..start + end])))
}

fn stopped_reason(line: &str) -> &'static str {
    if line.contains("breakpoint-hit") {
        "breakpoint"
    } else if line.contains("end-stepping-range") || line.contains("function-finished") {
        "step"
    } else {
        "pause"
    }
}

fn parse_stack_frames(line: &str) -> Vec<Value> {
    mi_parse_list_value(line, "frame")
        .into_iter()
        .enumerate()
        .map(|(idx, frame)| {
            let file = frame
                .get("fullname")
                .or_else(|| frame.get("file"))
                .cloned();
            json!({
                "id": idx + 1,
                "name": frame.get("func").cloned().unwrap_or_else(|| "<unknown>".to_string()),
                "source": file.as_ref().map(|f| json!({ "name": path_base_name(f), "path": f })),
                "line": frame.get("line").and_then(|n| n.parse::<u64>().ok()).unwrap_or(0),
                "column": 1,
            })
        })
        .collect()
}

fn parse_variables(line: &str) -> Vec<Value> {
    mi_parse_list_value(line, "variable")
        .into_iter()
        .filter_map(|var| {
            let name = var.get("name")?.clone();
            let value = var
                .get("value")
                .cloned()
                .unwrap_or_else(|| "<unavailable>".to_string());
            Some(json!({
                "name": name,
                "value": value,
                "variablesReference": 0,
            }))
        })
        .collect()
}

fn path_base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}
