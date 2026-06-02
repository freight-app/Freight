//! DapServer — native-DAP passthrough for `freight dap`.
//!
//! Freight acts as a thin shim between VS Code and the native DAP adapter
//! (gdb --interpreter=dap, lldb-dap, etc.).  It handles `initialize` and
//! `launch`/`attach` itself — building the project and probing the adapter —
//! then hands the rest of the session to `run_passthrough`, which relays all
//! traffic bidirectionally until the session ends.
//!
//! Only debuggers with native DAP support are accepted.  The MI2 bridge has
//! been removed; point users toward `gdb --interpreter=dap` (GDB ≥ 14) or
//! `lldb-dap` / `lldb-vscode`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use freight_core::build::{build_project_with, BuildOutput};
use freight_core::event::silent;
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::{detect_debuggers, load_debugger_templates, GlobalConfig};
use serde_json::{json, Value};

use super::protocol::{read_dap_frame, spawn_dap_reader, write_dap};

// ---------------------------------------------------------------------------
// DapServer
// ---------------------------------------------------------------------------

pub struct DapServer {
    seq: i64,
    requests: Receiver<Value>,
    stdout: std::io::Stdout,
    init_request: Option<Value>,
}

impl DapServer {
    pub fn new() -> Self {
        Self {
            seq: 1,
            requests: spawn_dap_reader(),
            stdout: std::io::stdout(),
            init_request: None,
        }
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        loop {
            let request = match self.requests.recv_timeout(Duration::from_millis(50)) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match request["command"].as_str().unwrap_or("") {
                "initialize" => self.handle_initialize(&request)?,
                "launch"     => self.handle_launch(&request)?,
                "attach"     => self.handle_attach(&request)?,
                "disconnect" | "terminate" => {
                    self.response(&request, json!({}))?;
                    self.event("terminated", json!({}))?;
                    break;
                }
                // Any other request that arrives before launch (rare) gets an
                // empty success response so VS Code doesn't stall.
                _ => self.response(&request, json!({}))?,
            }
        }
        Ok(())
    }

    // ── Initialize ────────────────────────────────────────────────────────────

    fn handle_initialize(&mut self, request: &Value) -> anyhow::Result<()> {
        self.init_request = Some(request.clone());
        // Respond with minimal capabilities.  The native adapter will send its
        // own initialize response (with richer caps) once the relay starts.
        self.response(request, json!({
            "supportsConfigurationDoneRequest": true,
            "supportsTerminateRequest":         true,
        }))
    }

    // ── Launch ────────────────────────────────────────────────────────────────

    fn handle_launch(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
        if let Some(cwd) = config["cwd"].as_str() {
            if !cwd.is_empty() { std::env::set_current_dir(cwd)?; }
        }

        // Run mode: just spawn `freight run` and pipe its output to VS Code.
        if config["mode"].as_str().unwrap_or("run") != "debug" {
            return self.launch_run(request, &config);
        }

        match self.launch_debug(request, &config) {
            Ok(()) => {}
            Err(err) => {
                self.output_event(&format!("{err}\n"), "stderr")?;
                self.error_response(request, &err.to_string())?;
                self.event("terminated", json!({}))?;
            }
        }
        Ok(())
    }

    fn launch_run(&mut self, request: &Value, config: &Value) -> anyhow::Result<()> {
        let freight = std::env::current_exe()?;
        let args = freight_run_args(config);
        self.response(request, json!({}))?;

        let mut child = Command::new(&freight)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Drain stdout/stderr into output events, then send terminated.
        if let Some(stdout) = child.stdout.take() {
            pipe_to_output(stdout, "stdout", &mut self.stdout, &mut self.seq);
        }
        if let Some(stderr) = child.stderr.take() {
            pipe_to_output(stderr, "stderr", &mut self.stdout, &mut self.seq);
        }
        let status = child.wait()?;
        self.event("exited",     json!({ "exitCode": status.code().unwrap_or(0) }))?;
        self.event("terminated", json!({}))?;
        Ok(())
    }

    fn launch_debug(&mut self, launch_request: &Value, config: &Value) -> anyhow::Result<()> {
        let (adapter_bin, adapter_args, binary) = self.resolve_adapter_and_build(config)?;

        let mut fwd = launch_request.clone();
        fwd["arguments"]["program"] = Value::String(binary.to_string_lossy().into_owned());

        self.output_event(
            &format!("Debugging {} with {}\n", binary.display(), adapter_bin.display()),
            "console",
        )?;
        self.run_passthrough(&adapter_bin, &adapter_args, fwd)
    }

    // ── Attach ────────────────────────────────────────────────────────────────

    fn handle_attach(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
        if let Some(cwd) = config["cwd"].as_str() {
            if !cwd.is_empty() { std::env::set_current_dir(cwd)?; }
        }
        match self.attach_debug(request, &config) {
            Ok(()) => {}
            Err(err) => {
                self.output_event(&format!("{err}\n"), "stderr")?;
                self.error_response(request, &err.to_string())?;
                self.event("terminated", json!({}))?;
            }
        }
        Ok(())
    }

    fn attach_debug(&mut self, attach_request: &Value, config: &Value) -> anyhow::Result<()> {
        let (adapter_bin, adapter_args, _) = self.resolve_adapter_only(config)?;
        self.output_event(
            &format!("Attaching with {}\n", adapter_bin.display()),
            "console",
        )?;
        self.run_passthrough(&adapter_bin, &adapter_args, attach_request.clone())
    }

    // ── Passthrough relay ─────────────────────────────────────────────────────

    /// Spawn the native DAP adapter, bootstrap it with the saved `initialize`
    /// and the given `first_request` (launch or attach), then relay all
    /// subsequent traffic bidirectionally until the adapter exits.
    fn run_passthrough(
        &mut self,
        adapter_bin: &Path,
        adapter_args: &[String],
        first_request: Value,
    ) -> anyhow::Result<()> {
        let mut child = Command::new(adapter_bin)
            .args(adapter_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut adapter_stdin = child.stdin.take()
            .ok_or_else(|| anyhow::anyhow!("adapter stdin unavailable"))?;
        let adapter_stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("adapter stdout unavailable"))?;

        // Pump adapter stdout → channel so we can interleave with VS Code input.
        let (adapter_out_tx, adapter_out_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(adapter_stdout);
            loop {
                match read_dap_frame(&mut reader) {
                    Some(frame) => {
                        let header = format!("Content-Length: {}\r\n\r\n", frame.len());
                        let mut msg = header.into_bytes();
                        msg.extend_from_slice(&frame);
                        if adapter_out_tx.send(msg).is_err() { break; }
                    }
                    None => break,
                }
            }
        });

        // Bootstrap: send initialize to the adapter.  We already responded to
        // VS Code's initialize; the adapter needs its own before it can do
        // anything.  Drain the adapter's initialize response (don't forward —
        // VS Code already has ours).
        let init_req = self.init_request.clone().unwrap_or_else(|| json!({
            "type": "request", "seq": 1, "command": "initialize",
            "arguments": {
                "clientID": "vscode", "adapterID": "freight",
                "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path"
            }
        }));
        write_dap(&mut adapter_stdin, &init_req)?;
        let _ = adapter_out_rx.recv_timeout(Duration::from_secs(5));

        // Forward the launch/attach request.
        write_dap(&mut adapter_stdin, &first_request)?;

        // Relay loop: adapter → VS Code, VS Code → adapter.
        loop {
            while let Ok(bytes) = adapter_out_rx.try_recv() {
                self.stdout.write_all(&bytes)?;
                self.stdout.flush()?;
            }
            let request = match self.requests.recv_timeout(Duration::from_millis(20)) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Ok(Some(_)) = child.try_wait() { break; }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            write_dap(&mut adapter_stdin, &request)?;
            match request["command"].as_str().unwrap_or("") {
                "disconnect" | "terminate" => break,
                _ => {}
            }
        }

        let _ = child.wait();
        while let Ok(bytes) = adapter_out_rx.try_recv() {
            let _ = self.stdout.write_all(&bytes);
        }
        let _ = self.stdout.flush();
        Ok(())
    }

    // ── Adapter / build resolution ────────────────────────────────────────────

    /// Resolve the DAP adapter binary + args AND build the project binary.
    fn resolve_adapter_and_build(
        &self,
        config: &Value,
    ) -> anyhow::Result<(PathBuf, Vec<String>, PathBuf)> {
        let project_dir = find_project_dir()?;
        let manifest = load_manifest(&project_dir)?;
        let global_cfg = load_global_cfg(&project_dir);
        let debuggers = detect_debuggers(&load_debugger_templates());

        let (adapter_bin, adapter_args) = select_dap_adapter(&debuggers, config, &global_cfg)?;

        let features = string_array(&config["features"]);
        let output = build_project_with(
            "debug", &features,
            !config["noDefaultFeatures"].as_bool().unwrap_or(false),
            &[], &silent(),
        )?;
        let binary = select_binary(&output, &project_dir, config["bin"].as_str(), &manifest)?;
        Ok((adapter_bin, adapter_args, binary))
    }

    /// Resolve only the DAP adapter (for attach — no build needed).
    fn resolve_adapter_only(
        &self,
        config: &Value,
    ) -> anyhow::Result<(PathBuf, Vec<String>, ())> {
        let project_dir = find_project_dir().unwrap_or_default();
        let global_cfg = load_global_cfg(&project_dir);
        let debuggers = detect_debuggers(&load_debugger_templates());
        let (bin, args) = select_dap_adapter(&debuggers, config, &global_cfg)?;
        Ok((bin, args, ()))
    }

    // ── DAP I/O ───────────────────────────────────────────────────────────────

    fn response(&mut self, request: &Value, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        write_dap(&mut self.stdout, &json!({
            "type": "response", "seq": seq,
            "request_seq": request["seq"], "success": true,
            "command": request["command"], "body": body,
        }))
    }

    fn error_response(&mut self, request: &Value, message: &str) -> anyhow::Result<()> {
        let seq = self.next_seq();
        write_dap(&mut self.stdout, &json!({
            "type": "response", "seq": seq,
            "request_seq": request["seq"], "success": false,
            "command": request["command"], "message": message,
        }))
    }

    fn event(&mut self, event: &str, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        write_dap(&mut self.stdout, &json!({
            "type": "event", "seq": seq, "event": event, "body": body,
        }))
    }

    fn output_event(&mut self, output: &str, category: &str) -> anyhow::Result<()> {
        self.event("output", json!({ "category": category, "output": output }))
    }

    fn next_seq(&mut self) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }
}

// ---------------------------------------------------------------------------
// Adapter selection
// ---------------------------------------------------------------------------

/// Find a DAP-capable adapter for the requested debugger (or the first one
/// available).  Returns `(adapter_binary, adapter_args)`.
///
/// Args include `-iex "set debuginfod enabled off"` for GDB to prevent it
/// from blocking on an interactive prompt before processing DAP messages.
fn select_dap_adapter(
    debuggers: &[freight_core::toolchain::DetectedDebugger],
    config: &Value,
    global_cfg: &GlobalConfig,
) -> anyhow::Result<(PathBuf, Vec<String>)> {
    if debuggers.is_empty() {
        anyhow::bail!(
            "no debugger found on PATH; install GDB ≥ 14 (gdb --interpreter=dap) \
             or lldb-dap / lldb-vscode"
        );
    }

    let pref = config["debugger"].as_str().or(global_cfg.default_debugger.as_deref());
    let candidates: Vec<_> = if let Some(name) = pref {
        debuggers.iter()
            .filter(|d| d.template.name == name)
            .collect()
    } else {
        debuggers.iter().collect()
    };

    for debugger in &candidates {
        match debugger.template.name.as_str() {
            "gdb" => {
                let args = vec![
                    "--interpreter=dap".to_string(),
                    "-iex".to_string(),
                    "set debuginfod enabled off".to_string(),
                ];
                if probe_dap_support(&debugger.path, &args) {
                    return Ok((debugger.path.clone(), args));
                }
            }
            "lldb" => {
                if let Some(ref dap_bin) = debugger.dap_path {
                    return Ok((dap_bin.clone(), vec![]));
                }
            }
            _ => {}
        }
    }

    let name = pref.unwrap_or("gdb or lldb");
    anyhow::bail!(
        "no DAP-capable adapter found for '{name}'; \
         upgrade to GDB ≥ 14 (which supports --interpreter=dap) \
         or install lldb-dap alongside lldb"
    )
}

// ---------------------------------------------------------------------------
// DAP support probe
// ---------------------------------------------------------------------------

/// Spawn `bin args…`, send a minimal `initialize` request, and wait up to
/// 3 seconds for any response.  Returns `true` if the adapter responds.
fn probe_dap_support(bin: &Path, args: &[String]) -> bool {
    let Ok(mut child) = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Ok(mut stdin) = child.stdin.take().ok_or(()) else {
        let _ = child.kill(); return false;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill(); return false;
    };
    let probe = json!({
        "type": "request", "seq": 1, "command": "initialize",
        "arguments": { "clientID": "freight-probe", "adapterID": "freight" }
    });
    if write_dap(&mut stdin, &probe).is_err() {
        let _ = child.kill(); return false;
    }
    let (tx, rx) = mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let _ = tx.send(read_dap_frame(&mut reader).is_some());
    });
    let result = rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false);
    let _ = child.kill();
    result
}

// ---------------------------------------------------------------------------
// run mode output relay
// ---------------------------------------------------------------------------

/// Synchronously drain a child process stream to VS Code output events.
fn pipe_to_output(
    src: impl std::io::Read,
    category: &'static str,
    stdout: &mut std::io::Stdout,
    seq: &mut i64,
) {
    use std::io::BufRead;
    for line in std::io::BufReader::new(src).lines().map_while(Result::ok) {
        let s = *seq;
        *seq += 1;
        let msg = json!({
            "type": "event", "seq": s, "event": "output",
            "body": { "category": category, "output": format!("{line}\n") }
        });
        let _ = write_dap(stdout, &msg);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_global_cfg(project_dir: &Path) -> GlobalConfig {
    let mut cfg = GlobalConfig::load();
    if let Some(local) = GlobalConfig::load_local(project_dir) {
        cfg.apply_local(local);
    }
    cfg
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
    let candidates: Vec<_> = if let Some(name) = filter {
        output.binaries.iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
            .cloned().collect()
    } else {
        output.binaries.clone()
    };
    match candidates.len() {
        0 if filter.is_some() => anyhow::bail!(
            "no binary named '{}' — available: {}",
            filter.unwrap(),
            manifest.bins.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
        0 => {
            let fallback = project_dir.join("target").join("debug").join(&manifest.package.name);
            if fallback.exists() { Ok(fallback) }
            else { anyhow::bail!("no binary built — does the manifest declare [[bin]]?") }
        }
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => anyhow::bail!(
            "multiple binaries; set `bin` to one of: {}",
            candidates.iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect::<Vec<_>>().join(", ")
        ),
    }
}

fn freight_run_args(config: &Value) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    if config["release"].as_bool().unwrap_or(false) { args.push("--release".into()); }
    if let Some(p) = config["package"].as_str() { args.extend(["-p".into(), p.into()]); }
    if let Some(b) = config["bin"].as_str()     { args.extend(["--bin".into(), b.into()]); }
    let features = string_array(&config["features"]);
    if !features.is_empty() { args.extend(["--features".into(), features.join(",")]); }
    if config["noDefaultFeatures"].as_bool().unwrap_or(false) {
        args.push("--no-default-features".into());
    }
    let program_args = string_array(&config["args"]);
    if !program_args.is_empty() { args.push("--".into()); args.extend(program_args); }
    args
}

fn string_array(value: &Value) -> Vec<String> {
    value.as_array()
        .map(|arr| arr.iter().filter_map(|i| i.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}
