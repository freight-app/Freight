//! `freight dap` — build the project and exec into the native DAP adapter.
//!
//! Freight builds the project, resolves the binary path, then replaces itself
//! with the native adapter (GDB ≥ 14 or lldb-dap) via `exec`.  The editor
//! connects directly to the adapter with no proxy in the middle.

use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use freight_core::build::{build_project_with, build_workspace_with, BuildOutput};
use freight_core::event::silent;
use freight_core::manifest::{find_manifest_dir, load_workspace_manifest};
use freight_core::toolchain::{detect_debuggers, load_debugger_templates, GlobalConfig};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Build the project and exec into the native DAP adapter.
///
/// `config` is the DAP launch/attach arguments object from the editor.
/// On `attach`, no build is performed.
pub fn launch_dap(config: &Value, is_attach: bool) -> anyhow::Result<()> {
    let project_dir = find_project_dir().unwrap_or_default();
    let global_cfg = load_global_cfg(&project_dir);
    let debuggers = detect_debuggers(&load_debugger_templates());
    let (adapter_bin, mut adapter_args) = select_dap_adapter(&debuggers, config, &global_cfg)?;

    if !is_attach {
        let features = config_string_array(config, "features");
        let outputs = build_outputs_for_dap(&project_dir, config, &features)?;
        let bin_buf = config_string(config, "bin");
        let binary = select_binary_from_outputs(&outputs, bin_buf.as_deref())?;
        // Append the binary path as the final argument so the adapter loads it.
        adapter_args.push(binary.to_string_lossy().into_owned());
    }

    exec_adapter(&adapter_bin, &adapter_args)
}

/// Replace the current process with the adapter.  Never returns on success.
fn exec_adapter(bin: &Path, args: &[String]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let err = Command::new(bin).args(args).exec();
        anyhow::bail!("failed to exec {}: {err}", bin.display());
    }
    #[cfg(not(unix))]
    {
        // Non-Unix fallback: spawn and wait (no true exec).
        let status = Command::new(bin).args(args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

// ---------------------------------------------------------------------------
// Adapter selection
// ---------------------------------------------------------------------------

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
        debuggers.iter().filter(|d| d.template.name == name).collect()
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
    if debugger == Some("lldb")
        || basename.contains("lldb-dap")
        || basename.contains("lldb-vscode")
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
    let body = serde_json::to_vec(&probe).unwrap_or_default();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    if stdin.write_all(header.as_bytes()).is_err() || stdin.write_all(&body).is_err() {
        let _ = child.kill();
        return false;
    }
    let (tx, rx) = mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        // Read one header line to confirm the adapter speaks DAP.
        let mut line = String::new();
        let _ = tx.send(
            reader.read_line(&mut line).is_ok()
                && line.trim_start().starts_with("Content-Length:"),
        );
    });
    let result = rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false);
    let _ = child.kill();
    result
}

// ---------------------------------------------------------------------------
// Build helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Misc helpers
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

fn config_namespace(config: &Value) -> Option<&Value> {
    config.get("freight").filter(|v| v.is_object())
}

fn config_value<'a>(config: &'a Value, key: &str) -> Option<&'a Value> {
    config_namespace(config)
        .and_then(|f| f.get(key))
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
