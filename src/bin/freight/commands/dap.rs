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
                "threads" => self.response(&request, json!({ "threads": [{ "id": 1, "name": "freight" }] }))?,
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
            anyhow::bail!("no debugger found on PATH; install gdb or lldb");
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

        if debugger.template.name != "gdb" {
            anyhow::bail!(
                "freight dap currently supports gdb/MI; selected debugger '{}' has no Freight DAP backend yet",
                debugger.template.name
            );
        }

        let features = string_array(&config["features"]);
        let output = build_project_with(
            "debug",
            &features,
            !config["noDefaultFeatures"].as_bool().unwrap_or(false),
            &[],
            &silent(),
        )?;
        let binary = select_binary(&output, &project_dir, config["bin"].as_str(), &manifest)?;

        self.output(
            &format!(
                "Debugging {} with {} {}\n",
                binary.display(),
                debugger.template.name,
                debugger.version
            ),
            "console",
        )?;

        let mut child = Command::new(&debugger.path)
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
        self.gdb_command(&format!("-file-exec-and-symbols {}", mi_quote(&binary.display().to_string())))?;
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

    fn handle_stack_trace(&mut self, request: &Value) -> anyhow::Result<()> {
        let line = self.gdb_command("-stack-list-frames")?;
        let frames = parse_stack_frames(&line);
        self.response(
            request,
            json!({
                "stackFrames": frames,
                "totalFrames": frames.len(),
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
            Err(err) => {
                self.response(
                    request,
                    json!({ "result": format!("<error: {err}>"), "variablesReference": 0 }),
                )
            }
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
        let bytes = serde_json::to_vec(&value)?;
        write!(self.stdout, "Content-Length: {}\r\n\r\n", bytes.len())?;
        self.stdout.write_all(&bytes)?;
        self.stdout.flush()?;
        Ok(())
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

fn spawn_dap_reader() -> Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            let mut content_length = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).ok().filter(|n| *n > 0).is_none() {
                    return;
                }
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    break;
                }
                if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                    content_length = rest.trim().parse::<usize>().ok();
                }
            }
            let Some(len) = content_length else {
                return;
            };
            let mut body = vec![0; len];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                if tx.send(value).is_err() {
                    return;
                }
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
    let mut escaped = false;
    for (idx, ch) in text[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(mi_c_string(&format!("\"{}\"", &text[start..start + idx])));
        }
    }
    None
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
    line.split("frame={")
        .skip(1)
        .enumerate()
        .map(|(idx, rest)| {
            let frame = rest.split('}').next().unwrap_or(rest);
            let file = mi_field(frame, "fullname").or_else(|| mi_field(frame, "file"));
            json!({
                "id": idx + 1,
                "name": mi_field(frame, "func").unwrap_or_else(|| "<unknown>".into()),
                "source": file.as_ref().map(|file| json!({ "name": path_base_name(file), "path": file })),
                "line": mi_field(frame, "line").and_then(|n| n.parse::<u64>().ok()).unwrap_or(0),
                "column": 1,
            })
        })
        .collect()
}

fn parse_variables(line: &str) -> Vec<Value> {
    line.split("{name=\"")
        .skip(1)
        .filter_map(|rest| {
            let name_end = rest.find('"')?;
            let name = mi_c_string(&format!("\"{}\"", &rest[..name_end]));
            let value = mi_field(rest, "value").unwrap_or_else(|| "<unavailable>".into());
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
