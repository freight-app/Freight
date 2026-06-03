//! DapServer — native-DAP passthrough for `freight dap`.
//!
//! Freight acts as a thin shim between any DAP client and the native DAP adapter
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
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use freight_core::build::{build_project_with, build_workspace_with, BuildOutput};
use freight_core::event::silent;
use freight_core::manifest::{find_manifest_dir, load_workspace_manifest};
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
    exit_requested: bool,
}

impl DapServer {
    pub fn new() -> Self {
        Self {
            seq: 1,
            requests: spawn_dap_reader(),
            stdout: std::io::stdout(),
            init_request: None,
            exit_requested: false,
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
                "launch" => self.handle_launch(&request)?,
                "attach" => self.handle_attach(&request)?,
                "freight/dapInfo" => self.handle_dap_info(&request)?,
                "disconnect" | "terminate" => {
                    self.exit_requested = true;
                    self.response(&request, json!({}))?;
                    self.event("terminated", json!({}))?;
                    break;
                }
                // Breakpoint config may arrive before launch when a client has
                // persistent breakpoints.  Respond with unverified placeholders;
                // clients usually re-send when they get GDB's `initialized` event
                // via the relay, at which point the binary is already loaded.
                cmd @ ("setBreakpoints" | "setFunctionBreakpoints") => {
                    self.buffer_pre_launch_config(&request, cmd)?;
                }
                "setExceptionBreakpoints" => {
                    self.response(&request, json!({}))?;
                }
                _ => self.response(&request, json!({}))?,
            }
            if self.exit_requested {
                break;
            }
        }
        Ok(())
    }

    // ── Initialize ────────────────────────────────────────────────────────────

    fn handle_initialize(&mut self, request: &Value) -> anyhow::Result<()> {
        self.init_request = Some(request.clone());
        // Respond with minimal capabilities. The native adapter sends
        // `initialized` once the binary is loaded and ready, which triggers
        // clients to send breakpoints at the right time.
        self.response(
            request,
            json!({
                "supportsConfigurationDoneRequest": true,
                "supportsTerminateRequest":         true,
            }),
        )
    }

    /// Reply to a pre-launch setBreakpoints/setFunctionBreakpoints with unverified
    /// placeholders. Clients can re-send the full set when they get GDB's own
    /// `initialized` event (forwarded by the relay), at which point GDB has the
    /// binary loaded and can verify the locations.
    fn buffer_pre_launch_config(&mut self, request: &Value, _cmd: &str) -> anyhow::Result<()> {
        let bps: Vec<Value> = request["arguments"]["breakpoints"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .map(|(i, _)| {
                        json!({
                            "id": i as i64 + 1,
                            "verified": false,
                            "message": "pending — waiting for debugger to start"
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.response(request, json!({ "breakpoints": bps }))
    }

    // ── Launch ────────────────────────────────────────────────────────────────

    fn handle_launch(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(cwd) = config_string(&config, "cwd") {
            if !cwd.is_empty() {
                std::env::set_current_dir(cwd)?;
            }
        }

        // Run mode: just spawn `freight run` and pipe its output to DAP output events.
        if config_string(&config, "mode").as_deref().unwrap_or("run") != "debug" {
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

        // Drain stdout/stderr concurrently into output events, then send terminated.
        let (tx, rx) = mpsc::channel::<(&'static str, String)>();
        if let Some(stdout) = child.stdout.take() {
            pipe_to_output(stdout, "stdout", tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            pipe_to_output(stderr, "stderr", tx);
        }
        loop {
            while let Ok((category, output)) = rx.try_recv() {
                self.output_event(&output, category)?;
            }
            if let Ok(stop_request) = self.requests.try_recv() {
                match stop_request["command"].as_str().unwrap_or("") {
                    "disconnect" | "terminate" => {
                        self.exit_requested = true;
                        let _ = child.kill();
                        self.response(&stop_request, json!({}))?;
                        self.event("terminated", json!({}))?;
                        return Ok(());
                    }
                    _ => self.response(&stop_request, json!({}))?,
                }
            }
            if let Some(status) = child.try_wait()? {
                while let Ok((category, output)) = rx.try_recv() {
                    self.output_event(&output, category)?;
                }
                self.event("exited", json!({ "exitCode": status.code().unwrap_or(0) }))?;
                self.event("terminated", json!({}))?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn launch_debug(&mut self, launch_request: &Value, config: &Value) -> anyhow::Result<()> {
        let (adapter_bin, adapter_args, binary) = self.resolve_adapter_and_build(config)?;

        let mut fwd = launch_request.clone();
        fwd["arguments"]["program"] = Value::String(binary.to_string_lossy().into_owned());
        normalize_launch_args(&mut fwd);

        self.output_event(
            &format!(
                "Debugging {} with {}\n",
                binary.display(),
                adapter_bin.display()
            ),
            "console",
        )?;
        self.run_passthrough(&adapter_bin, &adapter_args, fwd)
    }

    // ── Attach ────────────────────────────────────────────────────────────────

    fn handle_attach(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(cwd) = config_string(&config, "cwd") {
            if !cwd.is_empty() {
                std::env::set_current_dir(cwd)?;
            }
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
        let mut adapter_stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("adapter stdin unavailable"))?;
        let adapter_stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("adapter stdout unavailable"))?;

        // Pump adapter stdout into a channel so we can interleave it with client input.
        let (adapter_out_tx, adapter_out_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(adapter_stdout);
            loop {
                match read_dap_frame(&mut reader) {
                    Some(frame) => {
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

        // Bootstrap: send initialize to the native adapter.  We already responded
        // to the client's initialize; the adapter needs its own before it will
        // accept any other request.
        let init_req = self.init_request.clone().unwrap_or_else(|| {
            json!({
                "type": "request", "seq": 1, "command": "initialize",
                "arguments": {
                    "clientID": "freight", "adapterID": "freight",
                    "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path"
                }
            })
        });
        write_dap(&mut adapter_stdin, &init_req)?;

        // Drain the adapter's response to our initialize. It may also emit console
        // `output` events (startup banner) before it sends the response; those
        // are forwarded to the client.
        //
        // GDB's DAP emits `initialized` during launch so clients can send
        // setBreakpoints/configurationDone before the inferior starts. Breakpoints
        // may be reported as pending and verified shortly afterward; Freight
        // filters only the noisy console lines for that transient state.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match adapter_out_rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(ref bytes) => {
                    if gdb_msg_is(bytes, "response", "initialize") {
                        break; // discard; the client already has Freight's initialize response
                    }
                    self.stdout.write_all(bytes)?;
                    self.stdout.flush()?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Forward launch/attach so the adapter loads the binary.
        write_dap(&mut adapter_stdin, &first_request)?;

        let mut shutdown_requested = false;

        // Relay loop: adapter to client, client to adapter.
        loop {
            while let Ok(bytes) = adapter_out_rx.try_recv() {
                self.stdout.write_all(&bytes)?;
                self.stdout.flush()?;
            }
            let request = match self.requests.recv_timeout(Duration::from_millis(20)) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Ok(Some(_)) = child.try_wait() {
                        break;
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.exit_requested = true;
                    shutdown_requested = true;
                    break;
                }
            };
            write_dap(&mut adapter_stdin, &request)?;
            match request["command"].as_str().unwrap_or("") {
                "disconnect" | "terminate" => {
                    self.exit_requested = true;
                    shutdown_requested = true;
                    break;
                }
                _ => {}
            }
        }

        drop(adapter_stdin);
        if shutdown_requested {
            self.drain_adapter_shutdown(&mut child, &adapter_out_rx, Duration::from_millis(1200))?;
        } else {
            let _ = child.wait();
            self.flush_adapter_output(&adapter_out_rx)?;
        }
        Ok(())
    }

    fn drain_adapter_shutdown(
        &mut self,
        child: &mut Child,
        adapter_out_rx: &Receiver<Vec<u8>>,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.flush_adapter_output(adapter_out_rx)?;
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                self.flush_adapter_output(adapter_out_rx)?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn flush_adapter_output(&mut self, adapter_out_rx: &Receiver<Vec<u8>>) -> anyhow::Result<()> {
        while let Ok(bytes) = adapter_out_rx.try_recv() {
            self.stdout.write_all(&bytes)?;
        }
        self.stdout.flush()?;
        Ok(())
    }

    fn handle_dap_info(&mut self, request: &Value) -> anyhow::Result<()> {
        let project_dir = find_project_dir().unwrap_or_else(|_| PathBuf::new());
        let global_cfg = load_global_cfg(&project_dir);
        let debuggers = detect_debuggers(&load_debugger_templates());
        let debugger_names: Vec<Value> = debuggers.iter().map(|debugger| {
            json!({
                "name": debugger.template.name,
                "path": debugger.path.to_string_lossy(),
                "version": debugger.version,
                "dapPath": debugger.dap_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "selected": Some(debugger.template.name.as_str()) == global_cfg.default_debugger.as_deref(),
            })
        }).collect();
        self.response(
            request,
            json!({
                "name": "freight-dap",
                "schemaVersion": 1,
                "modes": ["run", "debug"],
                "requests": ["launch", "attach"],
                "config": {
                    "namespace": "freight",
                    "fields": [
                        "mode", "package", "bin", "features", "noDefaultFeatures",
                        "release", "debugger", "debuggerPath", "cwd"
                    ],
                    "standardForwardedFields": [
                        "args", "env", "stopAtEntry", "pid", "processName"
                    ]
                },
                "debuggers": debugger_names,
            }),
        )
    }

    // ── Adapter / build resolution ────────────────────────────────────────────

    /// Resolve the DAP adapter binary + args AND build the project binary.
    fn resolve_adapter_and_build(
        &self,
        config: &Value,
    ) -> anyhow::Result<(PathBuf, Vec<String>, PathBuf)> {
        let project_dir = find_project_dir()?;
        let global_cfg = load_global_cfg(&project_dir);
        let debuggers = detect_debuggers(&load_debugger_templates());

        let (adapter_bin, adapter_args) = select_dap_adapter(&debuggers, config, &global_cfg)?;

        let features = config_string_array(config, "features");
        let outputs = build_outputs_for_dap(&project_dir, config, &features)?;
        let bin_buf = config_string(config, "bin");
        let binary = select_binary_from_outputs(&outputs, bin_buf.as_deref())?;
        Ok((adapter_bin, adapter_args, binary))
    }

    /// Resolve only the DAP adapter (for attach — no build needed).
    fn resolve_adapter_only(&self, config: &Value) -> anyhow::Result<(PathBuf, Vec<String>, ())> {
        let project_dir = find_project_dir().unwrap_or_default();
        let global_cfg = load_global_cfg(&project_dir);
        let debuggers = detect_debuggers(&load_debugger_templates());
        let (bin, args) = select_dap_adapter(&debuggers, config, &global_cfg)?;
        Ok((bin, args, ()))
    }

    // ── DAP I/O ───────────────────────────────────────────────────────────────

    fn response(&mut self, request: &Value, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        write_dap(
            &mut self.stdout,
            &json!({
                "type": "response", "seq": seq,
                "request_seq": request["seq"], "success": true,
                "command": request["command"], "body": body,
            }),
        )
    }

    fn error_response(&mut self, request: &Value, message: &str) -> anyhow::Result<()> {
        let seq = self.next_seq();
        write_dap(
            &mut self.stdout,
            &json!({
                "type": "response", "seq": seq,
                "request_seq": request["seq"], "success": false,
                "command": request["command"], "message": message,
            }),
        )
    }

    fn event(&mut self, event: &str, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        write_dap(
            &mut self.stdout,
            &json!({
                "type": "event", "seq": seq, "event": event, "body": body,
            }),
        )
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
    if let Some(path) = config_string(config, "debuggerPath").filter(|p| !p.is_empty()) {
        return select_explicit_dap_adapter(PathBuf::from(path), config);
    }

    if debuggers.is_empty() {
        anyhow::bail!(
            "no debugger found on PATH; install GDB ≥ 14 (gdb --interpreter=dap) \
             or lldb-dap / lldb-vscode"
        );
    }

    let pref_buf = config_string(config, "debugger");
    let pref = pref_buf
        .as_deref()
        .or(global_cfg.default_debugger.as_deref());
    let candidates: Vec<_> = if let Some(name) = pref {
        debuggers
            .iter()
            .filter(|d| d.template.name == name)
            .collect()
    } else {
        debuggers.iter().collect()
    };

    for debugger in &candidates {
        match debugger.template.name.as_str() {
            "gdb" | "cuda-gdb" => {
                let args = gdb_dap_args();
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

fn select_explicit_dap_adapter(
    path: PathBuf,
    config: &Value,
) -> anyhow::Result<(PathBuf, Vec<String>)> {
    let path = resolve_debugger_path(path)
        .ok_or_else(|| anyhow::anyhow!("debuggerPath does not exist or is not executable"))?;

    let debugger_buf = config_string(config, "debugger");
    let debugger = debugger_buf.as_deref();
    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if debugger == Some("lldb") || basename.contains("lldb-dap") || basename.contains("lldb-vscode")
    {
        if probe_dap_support(&path, &[]) {
            return Ok((path, vec![]));
        }
        anyhow::bail!(
            "debuggerPath did not respond as a native DAP adapter: {}",
            path.display()
        );
    }

    if debugger == Some("gdb") || debugger == Some("cuda-gdb") || debugger.is_none() {
        let args = gdb_dap_args();
        if probe_dap_support(&path, &args) {
            return Ok((path, args));
        }
    }

    if debugger.is_none() && probe_dap_support(&path, &[]) {
        return Ok((path, vec![]));
    }

    anyhow::bail!(
        "debuggerPath is not DAP-capable with the selected debugger settings: {}",
        path.display()
    )
}

fn resolve_debugger_path(path: PathBuf) -> Option<PathBuf> {
    if path.exists() {
        return Some(path);
    }
    let name = path.to_str()?;
    if name.contains(std::path::MAIN_SEPARATOR) {
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn gdb_dap_args() -> Vec<String> {
    vec![
        "-q".to_string(),
        "--interpreter=dap".to_string(),
        "-iex".to_string(),
        "set debuginfod enabled off".to_string(),
    ]
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
        let _ = child.kill();
        return false;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return false;
    };
    let probe = json!({
        "type": "request", "seq": 1, "command": "initialize",
        "arguments": { "clientID": "freight-probe", "adapterID": "freight" }
    });
    if write_dap(&mut stdin, &probe).is_err() {
        let _ = child.kill();
        return false;
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

/// Synchronously drain a child process stream to DAP output events.
fn pipe_to_output(
    src: impl std::io::Read + Send + 'static,
    category: &'static str,
    tx: mpsc::Sender<(&'static str, String)>,
) {
    use std::io::BufRead;
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(src).lines().map_while(Result::ok) {
            if tx.send((category, format!("{line}\n"))).is_err() {
                break;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return true if `bytes` (a raw DAP frame with header) is a message of the
/// given `msg_type` and `command`.  Used to route GDB bootstrap messages
/// without fully parsing every frame.
fn gdb_msg_is(bytes: &[u8], msg_type: &str, command: &str) -> bool {
    let Some(msg) = dap_frame_msg(bytes) else {
        return false;
    };
    msg["type"] == msg_type && msg["command"] == command
}


fn dap_frame_msg(bytes: &[u8]) -> Option<Value> {
    let body = match bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => &bytes[i + 4..],
        None => return None,
    };
    serde_json::from_slice::<Value>(body).ok()
}

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

fn build_outputs_for_dap(
    project_dir: &Path,
    config: &Value,
    features: &[String],
) -> anyhow::Result<Vec<BuildOutput>> {
    let use_defaults = !config_bool(config, "noDefaultFeatures").unwrap_or(false);
    let package_buf = config_string(config, "package");
    let package = package_buf.as_deref();
    if load_workspace_manifest(project_dir).is_some() {
        return Ok(build_workspace_with(
            "dev",
            package,
            features,
            use_defaults,
            &silent(),
        )?);
    }
    if package.is_some() {
        anyhow::bail!("`package` can only be used when launching from a Freight workspace root");
    }
    Ok(vec![build_project_with(
        "dev",
        features,
        use_defaults,
        &[],
        &silent(),
    )?])
}

fn select_binary_from_outputs(
    outputs: &[BuildOutput],
    filter: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let all: Vec<PathBuf> = outputs
        .iter()
        .flat_map(|o| o.binaries.iter().cloned())
        .collect();
    let candidates: Vec<_> = filter
        .map(|name| {
            all.iter()
                .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
                .cloned()
                .collect()
        })
        .unwrap_or_else(|| all.clone());
    match candidates.len() {
        0 if filter.is_some() => anyhow::bail!(
            "no binary named '{}' — available: {}",
            filter.unwrap(),
            all.iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        0 => anyhow::bail!("no binary built — does the manifest declare [[bin]]?"),
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => anyhow::bail!(
            "multiple binaries; set `bin` to one of: {}",
            candidates
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn normalize_launch_args(msg: &mut Value) {
    let freight = msg
        .get("arguments")
        .and_then(|args| args.get("freight"))
        .cloned();
    if msg["arguments"]["args"].is_null() {
        msg["arguments"]["args"] = freight
            .as_ref()
            .and_then(|f| f.get("args"))
            .cloned()
            .unwrap_or_else(|| json!([]));
    }
    if msg["arguments"]["env"].is_null() {
        msg["arguments"]["env"] = freight
            .as_ref()
            .and_then(|f| f.get("env"))
            .cloned()
            .unwrap_or_else(|| json!({}));
    }
    if config_bool(&msg["arguments"], "stopAtEntry").unwrap_or(false) {
        msg["arguments"]["stopAtBeginningOfMainSubprogram"] = json!(true);
    }
}

fn freight_run_args(config: &Value) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    if config_bool(config, "release").unwrap_or(false) {
        args.push("--release".into());
    }
    if let Some(p) = config_string(config, "package") {
        args.extend(["-p".into(), p]);
    }
    if let Some(b) = config_string(config, "bin") {
        args.extend(["--bin".into(), b]);
    }
    let features = config_string_array(config, "features");
    if !features.is_empty() {
        args.extend(["--features".into(), features.join(",")]);
    }
    if config_bool(config, "noDefaultFeatures").unwrap_or(false) {
        args.push("--no-default-features".into());
    }
    let program_args = config_string_array(config, "args");
    if !program_args.is_empty() {
        args.push("--".into());
        args.extend(program_args);
    }
    args
}

fn config_namespace(config: &Value) -> Option<&Value> {
    config.get("freight").filter(|value| value.is_object())
}

fn config_value<'a>(config: &'a Value, key: &str) -> Option<&'a Value> {
    config_namespace(config)
        .and_then(|freight| freight.get(key))
        .or_else(|| config.get(key))
}

fn config_string(config: &Value, key: &str) -> Option<String> {
    config_value(config, key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn config_bool(config: &Value, key: &str) -> Option<bool> {
    config_value(config, key).and_then(Value::as_bool)
}

fn config_string_array(config: &Value, key: &str) -> Vec<String> {
    config_value(config, key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freight_namespace_overrides_top_level_launch_config() {
        let config = json!({
            "mode": "run",
            "bin": "top",
            "args": ["top-arg"],
            "freight": {
                "mode": "debug",
                "bin": "nested",
                "args": ["nested-arg"],
                "release": true,
                "features": ["simd", "trace"],
                "noDefaultFeatures": true
            }
        });

        assert_eq!(config_string(&config, "mode").as_deref(), Some("debug"));
        assert_eq!(config_string(&config, "bin").as_deref(), Some("nested"));
        assert_eq!(
            freight_run_args(&config),
            vec![
                "run",
                "--release",
                "--bin",
                "nested",
                "--features",
                "simd,trace",
                "--no-default-features",
                "--",
                "nested-arg"
            ]
        );
    }

    #[test]
    fn normalize_launch_args_accepts_freight_namespace() {
        let mut request = json!({
            "type": "request",
            "seq": 1,
            "command": "launch",
            "arguments": {
                "freight": {
                    "args": ["--case", "one"],
                    "env": { "FREIGHT_TEST": "1" },
                    "stopAtEntry": true
                }
            }
        });

        normalize_launch_args(&mut request);

        assert_eq!(request["arguments"]["args"], json!(["--case", "one"]));
        assert_eq!(request["arguments"]["env"], json!({ "FREIGHT_TEST": "1" }));
        assert_eq!(
            request["arguments"]["stopAtBeginningOfMainSubprogram"],
            json!(true)
        );
    }

    #[test]
    fn dap_frame_helpers_match_commands_and_events() {
        let response = dap_test_frame(json!({
            "type": "response",
            "seq": 1,
            "request_seq": 1,
            "success": true,
            "command": "launch"
        }));
        let event = dap_test_frame(json!({
            "type": "event",
            "seq": 2,
            "event": "initialized"
        }));

        assert!(gdb_msg_is(&response, "response", "launch"));
        assert!(!gdb_msg_is(&event, "response", "launch"));
    }

    fn dap_test_frame(msg: Value) -> Vec<u8> {
        let body = serde_json::to_vec(&msg).unwrap();
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend(body);
        frame
    }
}
