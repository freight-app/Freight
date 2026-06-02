//! DapServer — MI2 bridge + passthrough relay for `freight dap`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use freight_core::build::{build_project_with, BuildOutput};
use freight_core::event::silent;
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::{detect_debuggers, load_debugger_templates, GlobalConfig};
use serde_json::{json, Value};

use super::protocol::*;

// ---------------------------------------------------------------------------
// Breakpoint specs
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct BreakpointSpec {
    pub line: u64,
    pub condition: Option<String>,
    pub hit_condition: Option<u64>,
}

#[derive(Clone)]
pub struct FuncBreakpointSpec {
    pub name: String,
    pub condition: Option<String>,
}

// ---------------------------------------------------------------------------
// DapServer
// ---------------------------------------------------------------------------

pub struct DapServer {
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
    breakpoints: HashMap<String, Vec<BreakpointSpec>>,
    func_breakpoints: Vec<FuncBreakpointSpec>,
    exception_filters: Vec<String>,
    applied_bkpt_numbers: Vec<u64>,
    breakpoints_applied: bool,
    debug_started: bool,
    configuration_done: bool,
    attached: bool,
    init_request: Option<Value>,
    varobjs: HashMap<u64, String>,
    varobj_seq: u64,
    setvar_varobjs: HashMap<(u64, String), String>,
    top_varobjs: Vec<String>,
}

impl DapServer {
    pub fn new() -> Self {
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
            func_breakpoints: Vec::new(),
            exception_filters: Vec::new(),
            applied_bkpt_numbers: Vec::new(),
            breakpoints_applied: false,
            debug_started: false,
            configuration_done: false,
            attached: false,
            init_request: None,
            varobjs: HashMap::new(),
            varobj_seq: 1000,
            setvar_varobjs: HashMap::new(),
            top_varobjs: Vec::new(),
        }
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        loop {
            self.drain_debugger_events();
            let request = match self.requests.recv_timeout(Duration::from_millis(50)) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let command = request["command"].as_str().unwrap_or("");
            match command {
                "initialize"              => self.handle_initialize(&request)?,
                "launch"                  => self.handle_launch(&request)?,
                "attach"                  => self.handle_attach(&request)?,
                "setBreakpoints"          => self.handle_set_breakpoints(&request)?,
                "setFunctionBreakpoints"  => self.handle_set_function_breakpoints(&request)?,
                "setExceptionBreakpoints" => self.handle_set_exception_breakpoints(&request)?,
                "configurationDone"       => self.handle_configuration_done(&request)?,
                "threads"                 => self.handle_threads(&request)?,
                "stackTrace"              => self.handle_stack_trace(&request)?,
                "scopes"                  => self.handle_scopes(&request)?,
                "variables"               => self.handle_variables(&request)?,
                "setVariable"             => self.handle_set_variable(&request)?,
                "evaluate"                => self.handle_evaluate(&request)?,
                "continue" => {
                    let cmd = exec_cmd("-exec-continue", &request, false);
                    self.handle_exec(&request, &cmd, json!({ "allThreadsContinued": true }))?
                }
                "next" => {
                    let cmd = step_cmd("next", &request);
                    self.handle_exec(&request, &cmd, json!({}))?
                }
                "stepIn" => {
                    let cmd = step_cmd("stepIn", &request);
                    self.handle_exec(&request, &cmd, json!({}))?
                }
                "stepOut" => {
                    let cmd = exec_cmd("-exec-finish", &request, true);
                    self.handle_exec(&request, &cmd, json!({}))?
                }
                "pause" => {
                    let cmd = exec_cmd("-exec-interrupt", &request, true);
                    self.handle_exec(&request, &cmd, json!({}))?
                }
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

    // ── Initialize ────────────────────────────────────────────────────────────

    fn handle_initialize(&mut self, request: &Value) -> anyhow::Result<()> {
        self.init_request = Some(request.clone());
        self.response(request, json!({
            "supportsConfigurationDoneRequest": true,
            "supportsTerminateRequest": true,
            "supportsEvaluateForHovers": true,
            "supportsSetVariable": true,
            "supportsStepBack": false,
            "supportsConditionalBreakpoints": true,
            "supportsHitConditionalBreakpoints": true,
            "supportsFunctionBreakpoints": true,
            "supportsStepGranularities": true,
            "exceptionBreakpointFilters": [
                { "filter": "cpp_throw", "label": "C++: on throw",  "default": false },
                { "filter": "cpp_catch", "label": "C++: on catch",  "default": false },
                { "filter": "sig_segv",  "label": "SIGSEGV",        "default": false },
                { "filter": "sig_abrt",  "label": "SIGABRT",        "default": false },
            ]
        }))
    }

    // ── Launch ────────────────────────────────────────────────────────────────

    fn handle_launch(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
        if let Some(cwd) = config["cwd"].as_str() {
            if !cwd.is_empty() { std::env::set_current_dir(cwd)?; }
        }

        if config["mode"].as_str().unwrap_or("run") == "debug" {
            match self.launch_debug(request, &config) {
                Ok(true) => {}
                Ok(false) => {
                    self.event("initialized", json!({}))?;
                    self.response(request, json!({}))?;
                    if self.configuration_done { self.start_debuggee()?; }
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
            Ok(()) => self.response(request, json!({}))?,
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
        self.output(&format!("$ {} {}\n", freight.display(), shell_words(&args)), "console")?;
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

    fn launch_debug(&mut self, launch_request: &Value, config: &Value) -> anyhow::Result<bool> {
        let (debugger, binary) = self.resolve_debugger_and_build(config)?;
        match debugger.template.name.as_str() {
            "gdb" => {
                let (major, _) = gdb_version(&debugger.path);
                if major >= 14 {
                    self.output(&format!(
                        "Debugging {} with {} {} (native DAP)\n",
                        binary.display(), debugger.template.name, debugger.version
                    ), "console")?;
                    let mut fwd = launch_request.clone();
                    fwd["arguments"]["program"] =
                        Value::String(binary.to_string_lossy().into_owned());
                    self.run_passthrough(&debugger.path, &["--interpreter=dap"], fwd)
                } else {
                    self.output(&format!(
                        "Debugging {} with {} {} (GDB/MI2 bridge)\n",
                        binary.display(), debugger.template.name, debugger.version
                    ), "console")?;
                    self.launch_gdb_mi2(
                        &debugger.path,
                        &["--interpreter=mi2", "--quiet"],
                        Some(&binary), config,
                    )?;
                    Ok(false)
                }
            }
            "lldb" => {
                if let Some(ref dap_bin) = debugger.dap_path {
                    self.output(&format!(
                        "Debugging {} with {} (native DAP)\n",
                        binary.display(), dap_bin.display()
                    ), "console")?;
                    let mut fwd = launch_request.clone();
                    fwd["arguments"]["program"] =
                        Value::String(binary.to_string_lossy().into_owned());
                    self.run_passthrough(dap_bin, &[], fwd)
                } else {
                    self.output(&format!(
                        "Debugging {} with lldb {} (LLDB/MI2 bridge; install lldb-dap for full support)\n",
                        binary.display(), debugger.version
                    ), "console")?;
                    self.launch_gdb_mi2(
                        &debugger.path,
                        &["--interpreter=mi", "--quiet"],
                        Some(&binary), config,
                    )?;
                    Ok(false)
                }
            }
            "rr" => {
                self.output(&format!(
                    "Replaying with rr {} (GDB/MI2 bridge) — recording must exist\n",
                    debugger.version
                ), "console")?;
                self.launch_gdb_mi2(
                    &debugger.path,
                    &["replay", "--", "--interpreter=mi2", "--quiet"],
                    None, config,
                )?;
                Ok(false)
            }
            "cdb" | "windbg" => anyhow::bail!(
                "{} does not support a freight DAP backend; \
                 use the 'ms-vscode.cpptools' VS Code extension (cppdbg) for Windows debuggers",
                debugger.template.name
            ),
            other => anyhow::bail!(
                "debugger '{other}' has no Freight DAP backend; \
                 supported: gdb (≥14 native DAP, older via MI2), \
                 lldb (native DAP with lldb-dap, or MI2 fallback), \
                 rr (MI2 replay)"
            ),
        }
    }

    // ── Attach ────────────────────────────────────────────────────────────────

    fn handle_attach(&mut self, request: &Value) -> anyhow::Result<()> {
        let config = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
        if let Some(cwd) = config["cwd"].as_str() {
            if !cwd.is_empty() { std::env::set_current_dir(cwd)?; }
        }
        match self.attach_debug(request, &config) {
            Ok(true) => {}
            Ok(false) => {
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

    fn attach_debug(&mut self, attach_request: &Value, config: &Value) -> anyhow::Result<bool> {
        let pid = config["pid"].as_u64()
            .ok_or_else(|| anyhow::anyhow!("attach requires 'pid' in launch arguments"))?;

        let global_cfg = GlobalConfig::load();
        let debuggers = detect_debuggers(&load_debugger_templates());
        if debuggers.is_empty() { anyhow::bail!("no debugger found on PATH"); }
        let debugger_pref = config["debugger"].as_str()
            .or(global_cfg.default_debugger.as_deref());
        let debugger = if let Some(pref) = debugger_pref {
            debuggers.iter().find(|d| d.template.name == pref)
                .ok_or_else(|| anyhow::anyhow!("debugger '{pref}' not found on PATH"))?
        } else {
            &debuggers[0]
        };

        self.attached = true;
        match debugger.template.name.as_str() {
            "gdb" => {
                let (major, _) = gdb_version(&debugger.path);
                if major >= 14 {
                    self.output(&format!(
                        "Attaching to pid {} with {} {} (native DAP)\n",
                        pid, debugger.template.name, debugger.version
                    ), "console")?;
                    self.run_passthrough(&debugger.path, &["--interpreter=dap"], attach_request.clone())
                } else {
                    self.output(&format!(
                        "Attaching to pid {} with {} {} (GDB/MI2 bridge)\n",
                        pid, debugger.template.name, debugger.version
                    ), "console")?;
                    self.launch_gdb_mi2(&debugger.path, &["--interpreter=mi2", "--quiet"], None, config)?;
                    if let Some(prog) = config["program"].as_str() {
                        let _ = self.gdb_command(&format!("-file-exec-and-symbols {}", mi_quote(prog)));
                    }
                    self.gdb_command(&format!("-target-attach {pid}"))?;
                    Ok(false)
                }
            }
            "lldb" => {
                if let Some(ref dap_bin) = debugger.dap_path {
                    self.output(&format!(
                        "Attaching to pid {} with {} (native DAP)\n",
                        pid, dap_bin.display()
                    ), "console")?;
                    self.run_passthrough(dap_bin, &[], attach_request.clone())
                } else {
                    self.output(&format!(
                        "Attaching to pid {} with lldb {} (LLDB/MI2 bridge)\n",
                        pid, debugger.version
                    ), "console")?;
                    self.launch_gdb_mi2(&debugger.path, &["--interpreter=mi", "--quiet"], None, config)?;
                    if let Some(prog) = config["program"].as_str() {
                        let _ = self.gdb_command(&format!("-file-exec-and-symbols {}", mi_quote(prog)));
                    }
                    self.gdb_command(&format!("-target-attach {pid}"))?;
                    Ok(false)
                }
            }
            other => anyhow::bail!("attach is not supported for debugger '{other}'; use gdb or lldb"),
        }
    }

    // ── Passthrough relay ─────────────────────────────────────────────────────

    fn run_passthrough(
        &mut self,
        adapter_bin: &Path,
        adapter_args: &[&str],
        first_request: Value,
    ) -> anyhow::Result<bool> {
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

        let init_req = self.init_request.clone().unwrap_or_else(|| json!({
            "type": "request", "seq": 1, "command": "initialize",
            "arguments": {
                "clientID": "vscode", "adapterID": "freight",
                "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path"
            }
        }));
        write_dap(&mut adapter_stdin, &init_req)?;
        let _ = adapter_out_rx.recv_timeout(Duration::from_secs(5));
        write_dap(&mut adapter_stdin, &first_request)?;

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
        Ok(true)
    }

    // ── MI2 bridge setup ──────────────────────────────────────────────────────

    fn launch_gdb_mi2(
        &mut self,
        debugger_path: &Path,
        args: &[&str],
        binary: Option<&Path>,
        config: &Value,
    ) -> anyhow::Result<()> {
        let mut child = Command::new(debugger_path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("debugger stdout unavailable"))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| anyhow::anyhow!("debugger stderr unavailable"))?;
        let stdin = child.stdin.take()
            .ok_or_else(|| anyhow::anyhow!("debugger stdin unavailable"))?;
        self.gdb_rx = Some(spawn_gdb_reader(stdout));
        spawn_process_output(stderr, "stderr", self.output_tx.clone());
        self.gdb_stdin = Some(stdin);
        self.gdb = Some(child);
        let _ = self.gdb_command("-gdb-set target-async on");
        let _ = self.gdb_command("-gdb-set breakpoint pending on");
        if let Some(bin) = binary {
            self.gdb_command(&format!(
                "-file-exec-and-symbols {}", mi_quote(&bin.display().to_string())
            ))?;
            let program_args = string_array(&config["args"]);
            if !program_args.is_empty() {
                self.gdb_command(&format!(
                    "-exec-arguments {}",
                    program_args.iter().map(|a| mi_quote(a)).collect::<Vec<_>>().join(" ")
                ))?;
            }
        }
        self.apply_breakpoints()?;
        Ok(())
    }

    // ── Breakpoints ───────────────────────────────────────────────────────────

    fn handle_set_breakpoints(&mut self, request: &Value) -> anyhow::Result<()> {
        let args = &request["arguments"];
        let path = args["source"]["path"].as_str().unwrap_or("").to_string();
        let requested = args["breakpoints"].as_array().cloned().unwrap_or_default();
        if !path.is_empty() {
            let specs = requested.iter().filter_map(|bp| {
                let line = bp["line"].as_u64()?;
                let condition = bp["condition"].as_str()
                    .filter(|s| !s.is_empty()).map(str::to_string);
                let hit_condition = bp["hitCondition"].as_str()
                    .and_then(|s| s.trim().parse::<u64>().ok());
                Some(BreakpointSpec { line, condition, hit_condition })
            }).collect();
            self.breakpoints.insert(path.clone(), specs);
            self.breakpoints_applied = false;
            if self.gdb.is_some() { self.apply_breakpoints()?; }
        }
        self.response(request, json!({
            "breakpoints": requested.iter().map(|bp| json!({
                "verified": true,
                "source": args["source"].clone(),
                "line": bp["line"].clone(),
            })).collect::<Vec<_>>()
        }))
    }

    fn handle_set_function_breakpoints(&mut self, request: &Value) -> anyhow::Result<()> {
        let bps = request["arguments"]["breakpoints"].as_array().cloned().unwrap_or_default();
        self.func_breakpoints = bps.iter().filter_map(|bp| {
            let name = bp["name"].as_str().filter(|s| !s.is_empty())?.to_string();
            let condition = bp["condition"].as_str()
                .filter(|s| !s.is_empty()).map(str::to_string);
            Some(FuncBreakpointSpec { name, condition })
        }).collect();
        self.breakpoints_applied = false;
        if self.gdb.is_some() { self.apply_breakpoints()?; }
        self.response(request, json!({
            "breakpoints": bps.iter().map(|bp| json!({
                "verified": self.gdb.is_some(),
                "name": bp["name"].clone(),
            })).collect::<Vec<_>>()
        }))
    }

    fn handle_set_exception_breakpoints(&mut self, request: &Value) -> anyhow::Result<()> {
        let filters = request["arguments"]["filters"].as_array().cloned().unwrap_or_default();
        self.exception_filters = filters.iter()
            .filter_map(|f| f.as_str().map(str::to_string)).collect();
        self.breakpoints_applied = false;
        if self.gdb.is_some() { self.apply_breakpoints()?; }
        self.response(request, json!({ "breakpoints": [] }))
    }

    fn apply_breakpoints(&mut self) -> anyhow::Result<()> {
        if self.gdb.is_none() || self.breakpoints_applied { return Ok(()); }

        if !self.applied_bkpt_numbers.is_empty() {
            let nums = self.applied_bkpt_numbers.iter()
                .map(|n| n.to_string()).collect::<Vec<_>>().join(" ");
            let _ = self.gdb_command(&format!("-break-delete {nums}"));
            self.applied_bkpt_numbers.clear();
        }

        for (file, specs) in self.breakpoints.clone() {
            for spec in specs {
                let loc = mi_quote(&format!("{}:{}", file, spec.line));
                let cmd = if let Some(ref cond) = spec.condition {
                    format!("-break-insert -c {} {loc}", mi_quote(cond))
                } else {
                    format!("-break-insert {loc}")
                };
                match self.gdb_command(&cmd) {
                    Ok(line) => {
                        if let Some(n) = mi_field(&line, "number").and_then(|s| s.parse().ok()) {
                            self.applied_bkpt_numbers.push(n);
                            if let Some(count) = spec.hit_condition {
                                let _ = self.gdb_command(&format!("-break-after {n} {count}"));
                            }
                        }
                    }
                    Err(err) => self.output(&format!("{err}\n"), "stderr")?,
                }
            }
        }

        for spec in self.func_breakpoints.clone() {
            let cmd = if let Some(ref cond) = spec.condition {
                format!("-break-insert -f -c {} {}", mi_quote(cond), mi_quote(&spec.name))
            } else {
                format!("-break-insert -f {}", mi_quote(&spec.name))
            };
            if let Ok(line) = self.gdb_command(&cmd) {
                if let Some(n) = mi_field(&line, "number").and_then(|s| s.parse().ok()) {
                    self.applied_bkpt_numbers.push(n);
                }
            }
        }

        for filter in self.exception_filters.clone() {
            let cmd = match filter.as_str() {
                "cpp_throw" => Some("-catch-throw"),
                "cpp_catch" => Some("-catch-catch"),
                "sig_segv"  => Some("-catch-signal SIGSEGV"),
                "sig_abrt"  => Some("-catch-signal SIGABRT"),
                _ => None,
            };
            if let Some(cmd) = cmd {
                if let Ok(line) = self.gdb_command(cmd) {
                    if let Some(n) = mi_field(&line, "number").and_then(|s| s.parse().ok()) {
                        self.applied_bkpt_numbers.push(n);
                    }
                }
            }
        }

        self.breakpoints_applied = true;
        Ok(())
    }

    fn handle_configuration_done(&mut self, request: &Value) -> anyhow::Result<()> {
        self.configuration_done = true;
        self.response(request, json!({}))?;
        self.start_debuggee()
    }

    // ── Execution control ─────────────────────────────────────────────────────

    fn handle_exec(&mut self, request: &Value, command: &str, body: Value) -> anyhow::Result<()> {
        self.drop_varobjs();
        if let Err(err) = self.gdb_command(command) {
            self.output(&format!("{err}\n"), "stderr")?;
        }
        self.response(request, body)
    }

    fn start_debuggee(&mut self) -> anyhow::Result<()> {
        if self.gdb.is_none() || self.debug_started { return Ok(()); }
        self.debug_started = true;
        let cmd = if self.attached { "-exec-continue" } else { "-exec-run" };
        if let Err(err) = self.gdb_command(cmd) {
            self.output(&format!("{err}\n"), "stderr")?;
            self.event("terminated", json!({}))?;
        }
        Ok(())
    }

    // ── Threads ───────────────────────────────────────────────────────────────

    fn handle_threads(&mut self, request: &Value) -> anyhow::Result<()> {
        if self.gdb.is_none() {
            return self.response(request, json!({ "threads": [{ "id": 1, "name": "main" }] }));
        }
        let threads = match self.gdb_command("-thread-list-ids") {
            Ok(line) => {
                let ids = mi_parse_list_value(&line, "thread-id");
                if ids.is_empty() {
                    vec![json!({ "id": 1, "name": "main" })]
                } else {
                    ids.iter().filter_map(|m| {
                        let id: u64 = m.get("thread-id")?.parse().ok()?;
                        Some(json!({ "id": id, "name": format!("thread {id}") }))
                    }).collect()
                }
            }
            Err(_) => vec![json!({ "id": 1, "name": "main" })],
        };
        self.response(request, json!({ "threads": threads }))
    }

    // ── Stack trace ───────────────────────────────────────────────────────────

    fn handle_stack_trace(&mut self, request: &Value) -> anyhow::Result<()> {
        let line = self.gdb_command("-stack-list-frames")?;
        let frames = parse_stack_frames(&line);
        let total = frames.len();
        self.response(request, json!({ "stackFrames": frames, "totalFrames": total }))
    }

    // ── Scopes ────────────────────────────────────────────────────────────────

    fn handle_scopes(&mut self, request: &Value) -> anyhow::Result<()> {
        let frame_id = request["arguments"]["frameId"].as_u64().unwrap_or(0);
        self.response(request, json!({
            "scopes": [{
                "name": "Locals",
                "variablesReference": frame_id + 1,
                "expensive": false,
            }]
        }))
    }

    // ── Variables ─────────────────────────────────────────────────────────────

    fn handle_variables(&mut self, request: &Value) -> anyhow::Result<()> {
        let var_ref = request["arguments"]["variablesReference"].as_u64().unwrap_or(1);

        if var_ref < 1000 {
            let frame_id = var_ref - 1;
            if frame_id > 0 {
                let _ = self.gdb_command(&format!("-stack-select-frame {frame_id}"));
            }
            let line = self.gdb_command("-stack-list-variables --all-values")?;
            if frame_id > 0 {
                let _ = self.gdb_command("-stack-select-frame 0");
            }
            let raw = parse_raw_variables(&line);
            let mut result = Vec::new();
            for (name, value, type_) in raw {
                let vo_name = format!("frt_{}", self.varobj_seq);
                self.varobj_seq += 1;
                match self.gdb_command(&format!("-var-create {} * {}", vo_name, mi_quote(&name))) {
                    Ok(info) => {
                        self.top_varobjs.push(vo_name.clone());
                        let numchild: u64 = mi_field(&info, "numchild")
                            .and_then(|s| s.parse().ok()).unwrap_or(0);
                        let display = mi_field(&info, "value").unwrap_or(value);
                        self.setvar_varobjs.insert((var_ref, name.clone()), vo_name.clone());
                        let ref_id = if numchild > 0 {
                            let id = self.varobj_seq;
                            self.varobj_seq += 1;
                            self.varobjs.insert(id, vo_name);
                            id
                        } else {
                            0
                        };
                        result.push(json!({
                            "name": name,
                            "value": display,
                            "type": type_,
                            "variablesReference": ref_id,
                        }));
                    }
                    Err(_) => {
                        result.push(json!({
                            "name": name,
                            "value": value,
                            "type": type_,
                            "variablesReference": 0,
                        }));
                    }
                }
            }
            self.response(request, json!({ "variables": result }))
        } else {
            let vo_name = self.varobjs.get(&var_ref).cloned();
            match vo_name {
                None => self.response(request, json!({ "variables": [] })),
                Some(name) => {
                    let line = self.gdb_command(&format!(
                        "-var-list-children --all-values {}", mi_quote(&name)
                    ))?;
                    let children = self.expand_varobj_children(var_ref, &line)?;
                    self.response(request, json!({ "variables": children }))
                }
            }
        }
    }

    fn expand_varobj_children(&mut self, parent_ref: u64, line: &str) -> anyhow::Result<Vec<Value>> {
        let raw = mi_parse_list_value(line, "child");
        let mut result = Vec::new();
        for child in raw {
            let child_name = child.get("name").cloned().unwrap_or_default();
            let exp = child.get("exp").cloned().unwrap_or_else(|| child_name.clone());
            let value = child.get("value").cloned()
                .unwrap_or_else(|| "<unavailable>".to_string());
            let type_ = child.get("type").cloned().unwrap_or_default();
            let numchild: u64 = child.get("numchild")
                .and_then(|s| s.parse().ok()).unwrap_or(0);
            self.setvar_varobjs.insert((parent_ref, exp.clone()), child_name.clone());
            let ref_id = if numchild > 0 {
                let id = self.varobj_seq;
                self.varobj_seq += 1;
                self.varobjs.insert(id, child_name);
                id
            } else {
                0
            };
            result.push(json!({
                "name": exp,
                "value": value,
                "type": type_,
                "variablesReference": ref_id,
            }));
        }
        Ok(result)
    }

    fn drop_varobjs(&mut self) {
        if self.gdb.is_some() {
            let names = self.top_varobjs.clone();
            for name in &names {
                let _ = self.gdb_command(&format!("-var-delete {}", mi_quote(name)));
            }
        }
        self.varobjs.clear();
        self.setvar_varobjs.clear();
        self.top_varobjs.clear();
    }

    // ── setVariable ───────────────────────────────────────────────────────────

    fn handle_set_variable(&mut self, request: &Value) -> anyhow::Result<()> {
        let parent_ref = request["arguments"]["variablesReference"].as_u64().unwrap_or(0);
        let name = request["arguments"]["name"].as_str().unwrap_or("").to_string();
        let value = request["arguments"]["value"].as_str().unwrap_or("");
        match self.setvar_varobjs.get(&(parent_ref, name.clone())).cloned() {
            Some(vo_name) => {
                match self.gdb_command(&format!("-var-assign {} {}", mi_quote(&vo_name), mi_quote(value))) {
                    Ok(line) => {
                        let new_val = mi_field(&line, "value").unwrap_or_else(|| value.to_string());
                        self.response(request, json!({ "value": new_val }))
                    }
                    Err(err) => self.error_response(request, &err.to_string()),
                }
            }
            None => self.error_response(request, &format!("variable '{name}' not found")),
        }
    }

    // ── Evaluate ─────────────────────────────────────────────────────────────

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

    // ── DAP I/O ───────────────────────────────────────────────────────────────

    fn response(&mut self, request: &Value, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        self.write(json!({
            "type": "response", "seq": seq,
            "request_seq": request["seq"], "success": true,
            "command": request["command"], "body": body,
        }))
    }

    fn error_response(&mut self, request: &Value, message: &str) -> anyhow::Result<()> {
        let seq = self.next_seq();
        self.write(json!({
            "type": "response", "seq": seq,
            "request_seq": request["seq"], "success": false,
            "command": request["command"], "message": message,
        }))
    }

    fn event(&mut self, event: &str, body: Value) -> anyhow::Result<()> {
        let seq = self.next_seq();
        self.write(json!({
            "type": "event", "seq": seq, "event": event, "body": body,
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

    // ── Shutdown ──────────────────────────────────────────────────────────────

    fn shutdown(&mut self) {
        self.varobjs.clear();
        self.setvar_varobjs.clear();
        self.top_varobjs.clear();
        if let Some(mut stdin) = self.gdb_stdin.take() {
            let _ = writeln!(stdin, "-gdb-exit");
        }
        if let Some(mut child) = self.gdb.take() { let _ = child.kill(); }
        if let Some(mut child) = self.run_process.take() { let _ = child.kill(); }
    }

    // ── GDB/MI2 event loop ────────────────────────────────────────────────────

    fn drain_debugger_events(&mut self) {
        while let Ok((category, output)) = self.output_rx.try_recv() {
            let _ = self.output(&output, &category);
        }
        while let Some(line) = self.gdb_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            let _ = self.handle_gdb_line(&line, None);
        }
        if let Some(child) = self.run_process.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let code = status.code().unwrap_or(0);
                let _ = self.event("exited", json!({ "exitCode": code }));
                let _ = self.event("terminated", json!({}));
                self.run_process = None;
            }
        }
        if let Some(child) = self.gdb.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
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
            let reason = mi_field(line, "reason").unwrap_or_default();
            if reason.starts_with("exited") {
                let exit_code: i64 = mi_field(line, "exit-code")
                    .and_then(|s| {
                        let s = s.trim_start_matches("0o");
                        i64::from_str_radix(s, 8).ok().or_else(|| s.parse().ok())
                    })
                    .unwrap_or(if reason == "exited-normally" { 0 } else { -1 });
                self.event("exited", json!({ "exitCode": exit_code }))?;
                self.event("terminated", json!({}))?;
            } else if reason == "exited-signalled" {
                let sig = mi_field(line, "signal-name").unwrap_or_else(|| "signal".into());
                self.output(&format!("Program terminated with signal {sig}\n"), "stderr")?;
                self.event("exited", json!({ "exitCode": -1 }))?;
                self.event("terminated", json!({}))?;
            } else {
                self.event("stopped", json!({
                    "reason": stopped_reason(&reason),
                    "threadId": mi_field(line, "thread-id")
                        .and_then(|s| s.parse::<u64>().ok()).unwrap_or(1),
                    "allThreadsStopped": true,
                }))?;
            }
            return Ok(None);
        }
        let Some((token, result)) = parse_mi_result(line) else {
            return Ok(None);
        };
        if waiting_for != Some(token) { return Ok(None); }
        if result == "error" {
            anyhow::bail!("{}", mi_field(line, "msg").unwrap_or_else(|| line.to_string()));
        }
        Ok(Some(line.to_string()))
    }

    fn gdb_command(&mut self, command: &str) -> anyhow::Result<String> {
        let token = self.gdb_token;
        self.gdb_token += 1;
        let stdin = self.gdb_stdin.as_mut()
            .ok_or_else(|| anyhow::anyhow!("debugger is not running"))?;
        writeln!(stdin, "{token}{command}")?;
        stdin.flush()?;
        loop {
            let line = self.gdb_rx.as_ref()
                .ok_or_else(|| anyhow::anyhow!("debugger output unavailable"))?
                .recv_timeout(Duration::from_secs(30))?;
            if let Some(result) = self.handle_gdb_line(&line, Some(token))? {
                return Ok(result);
            }
        }
    }

    // ── Debugger resolution + build ───────────────────────────────────────────

    fn resolve_debugger_and_build(
        &self,
        config: &Value,
    ) -> anyhow::Result<(freight_core::toolchain::DetectedDebugger, PathBuf)> {
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
        let debugger_pref = config["debugger"].as_str()
            .or(global_cfg.default_debugger.as_deref());
        let debugger = if let Some(pref) = debugger_pref {
            debuggers.iter().find(|d| d.template.name == pref)
                .ok_or_else(|| anyhow::anyhow!("debugger '{pref}' not found on PATH"))?
                .clone()
        } else {
            debuggers[0].clone()
        };
        let features = string_array(&config["features"]);
        let output = build_project_with(
            "debug", &features,
            !config["noDefaultFeatures"].as_bool().unwrap_or(false),
            &[], &silent(),
        )?;
        let binary = select_binary(&output, &project_dir, config["bin"].as_str(), &manifest)?;
        Ok((debugger, binary))
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn exec_cmd(base: &str, request: &Value, thread: bool) -> String {
    if thread {
        if let Some(tid) = request["arguments"]["threadId"].as_u64() {
            return format!("{base} --thread {tid}");
        }
    }
    base.to_string()
}

fn step_cmd(kind: &str, request: &Value) -> String {
    let granularity = request["arguments"]["granularity"].as_str().unwrap_or("statement");
    let instruction = granularity == "instruction";
    let base = match (kind, instruction) {
        ("next",   true)  => "-exec-next-instruction",
        ("next",   false) => "-exec-next",
        ("stepIn", true)  => "-exec-step-instruction",
        ("stepIn", false) => "-exec-step",
        _                 => "-exec-next",
    };
    if let Some(tid) = request["arguments"]["threadId"].as_u64() {
        format!("{base} --thread {tid}")
    } else {
        base.to_string()
    }
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
        output.binaries.iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
            .cloned().collect::<Vec<_>>()
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
            "multiple binaries built; set `bin` to one of: {}",
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
    if let Some(b) = config["bin"].as_str() { args.extend(["--bin".into(), b.into()]); }
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

fn shell_words(args: &[String]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.chars().all(|c| c.is_ascii_alphanumeric() || "_./:=+-".contains(c)) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
