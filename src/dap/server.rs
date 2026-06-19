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

use crate::build::{build_project_with, build_workspace_with, BuildOutput};
use crate::event::silent;
use crate::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest, Manifest};
use crate::toolchain::{detect_debuggers, load_debugger_templates, GlobalConfig};
use crate::vendor::parse_triple;
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
        let bin_buf = config_string(config, "bin");
        let binary = if config_bool(config, "noBuild").unwrap_or(false) {
            existing_binary_for_dap(&project_dir, config, bin_buf.as_deref())?
        } else {
            let features = config_string_array(config, "features");
            let outputs = build_outputs_for_dap(&project_dir, config, &features)?;
            select_binary_from_outputs(&outputs, bin_buf.as_deref())?
        };
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
    debuggers: &[crate::toolchain::DetectedDebugger],
    config: &Value,
    global_cfg: &GlobalConfig,
) -> anyhow::Result<(PathBuf, Vec<String>)> {
    if let Some(path) = config_string(config, "debuggerPath").filter(|p| !p.is_empty()) {
        return select_explicit_dap_adapter(PathBuf::from(path), config, global_cfg);
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
                let args = dap_args_for_debugger(
                    debugger.template.name.as_str(),
                    &gdb_dap_args(),
                    config,
                    global_cfg,
                );
                if probe_dap_support(&debugger.path, &args) {
                    return Ok((debugger.path.clone(), args));
                }
            }
            "lldb" => {
                if let Some(ref dap_bin) = debugger.dap_path {
                    return Ok((
                        dap_bin.clone(),
                        dap_args_for_debugger("lldb", &[], config, global_cfg),
                    ));
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
    global_cfg: &GlobalConfig,
) -> anyhow::Result<(PathBuf, Vec<String>)> {
    let path = resolve_debugger_path(path)
        .ok_or_else(|| anyhow::anyhow!("debuggerPath does not exist or is not executable"))?;

    let debugger_buf = config_string(config, "debugger");
    let debugger = debugger_buf.as_deref();
    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if debugger == Some("lldb") || basename.contains("lldb-dap") || basename.contains("lldb-vscode")
    {
        if probe_dap_support(&path, &[]) {
            return Ok((
                path,
                dap_args_for_debugger(debugger.unwrap_or("lldb"), &[], config, global_cfg),
            ));
        }
        anyhow::bail!(
            "debuggerPath did not respond as a native DAP adapter: {}",
            path.display()
        );
    }

    if debugger == Some("gdb") || debugger == Some("cuda-gdb") || debugger.is_none() {
        let debugger_name = debugger.unwrap_or("gdb");
        let args = dap_args_for_debugger(debugger_name, &gdb_dap_args(), config, global_cfg);
        if probe_dap_support(&path, &args) {
            return Ok((path, args));
        }
    }

    if debugger.is_none() && probe_dap_support(&path, &[]) {
        return Ok((path, dap_args_for_debugger("lldb", &[], config, global_cfg)));
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

fn dap_args_for_debugger(
    debugger_name: &str,
    base: &[String],
    config: &Value,
    global_cfg: &GlobalConfig,
) -> Vec<String> {
    let mut args = base.to_vec();
    if let Some(instance) = global_cfg.debugger.debuggers.get(debugger_name) {
        args.extend(instance.args.iter().cloned());
    }
    args.extend(config_string_array(config, "debuggerArgs"));
    args.extend(config_string_array(config, "debugger_args"));
    args
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
            reader.read_line(&mut line).is_ok() && line.trim_start().starts_with("Content-Length:"),
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
    let profile = dap_profile(config);
    let use_defaults = !config_bool(config, "noDefaultFeatures").unwrap_or(false);
    let package_buf = config_string(config, "package");
    let package = package_buf.as_deref();
    if load_workspace_manifest(project_dir).is_some() {
        return Ok(build_workspace_with(
            &profile,
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
        &profile,
        features,
        use_defaults,
        &[],
        &silent(),
    )?])
}

fn existing_binary_for_dap(
    project_dir: &Path,
    config: &Value,
    filter: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let profile = dap_profile(config);
    let package_buf = config_string(config, "package");
    let package = package_buf.as_deref();
    let mut binaries = Vec::new();

    if let Some(ws) = load_workspace_manifest(project_dir) {
        for member in &ws.members {
            let member_dir = project_dir.join(member.trim_end_matches('/'));
            let member_name = member_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if let Some(pkg) = package {
                if member_name != pkg {
                    continue;
                }
            }
            let manifest = load_manifest(&member_dir)?;
            binaries.extend(binary_paths_for_manifest(&member_dir, &manifest, &profile));
        }
    } else {
        if package.is_some() {
            anyhow::bail!(
                "`package` can only be used when launching from a Freight workspace root"
            );
        }
        let manifest = load_manifest(project_dir)?;
        binaries.extend(binary_paths_for_manifest(project_dir, &manifest, &profile));
    }

    let binary = select_binary_from_paths(&binaries, filter)?;
    if !binary.exists() {
        let release_flag = if profile == "release" {
            " --release"
        } else {
            ""
        };
        anyhow::bail!(
            "{} does not exist; run `freight build{release_flag}` before debugging",
            binary.display()
        );
    }
    Ok(binary)
}

fn binary_paths_for_manifest(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
) -> Vec<PathBuf> {
    let target_os = dap_target_os(manifest);
    manifest
        .bins
        .iter()
        .map(|bin| {
            project_dir
                .join("target")
                .join(profile)
                .join(dap_executable_name(&bin.name, &target_os))
        })
        .collect()
}

fn select_binary_from_paths(paths: &[PathBuf], filter: Option<&str>) -> anyhow::Result<PathBuf> {
    let candidates: Vec<_> = filter
        .map(|name| {
            paths
                .iter()
                .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
                .cloned()
                .collect()
        })
        .unwrap_or_else(|| paths.to_vec());
    match candidates.len() {
        0 if filter.is_some() => anyhow::bail!(
            "no binary named '{}' — available: {}",
            filter.unwrap(),
            paths
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        0 => anyhow::bail!("no binary target declared — does the manifest declare [[bin]]?"),
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

fn dap_profile(config: &Value) -> String {
    config_string(config, "profile").unwrap_or_else(|| {
        if config_bool(config, "release").unwrap_or(false) {
            "release".to_string()
        } else {
            "debug".to_string()
        }
    })
}

fn dap_target_os(manifest: &Manifest) -> String {
    manifest
        .compiler
        .target
        .as_deref()
        .map(parse_triple)
        .map(|(_, os)| os)
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

fn dap_executable_name(name: &str, target_os: &str) -> String {
    if target_os == "windows" && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::types::DebuggerInstanceConfig;
    use crate::toolchain::debugger::{DapConfig, LaunchConfig};
    use crate::toolchain::{DebuggerTemplate, DetectedDebugger};
    use std::collections::HashMap;

    #[test]
    fn explicit_gdb_path_selects_gdb_dap_args() {
        let tmp = tempfile::tempdir().unwrap();
        let gdb = write_fake_dap(&tmp, "gdb");
        let config = json!({
            "debugger": "gdb",
            "debuggerPath": gdb,
        });

        let (bin, args) = select_dap_adapter(&[], &config, &GlobalConfig::default()).unwrap();

        assert_eq!(bin, PathBuf::from(config["debuggerPath"].as_str().unwrap()));
        assert_eq!(args, gdb_dap_args());
    }

    #[test]
    fn explicit_cuda_gdb_path_uses_gdb_dap_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let cuda_gdb = write_fake_dap(&tmp, "cuda-gdb");
        let config = json!({
            "debugger": "cuda-gdb",
            "debuggerPath": cuda_gdb,
        });

        let (bin, args) = select_dap_adapter(&[], &config, &GlobalConfig::default()).unwrap();

        assert_eq!(bin, PathBuf::from(config["debuggerPath"].as_str().unwrap()));
        assert_eq!(args, gdb_dap_args());
    }

    #[test]
    fn explicit_lldb_dap_path_uses_native_adapter_without_extra_args() {
        let tmp = tempfile::tempdir().unwrap();
        let lldb_dap = write_fake_dap(&tmp, "lldb-dap");
        let config = json!({
            "debugger": "lldb",
            "debuggerPath": lldb_dap,
        });

        let (bin, args) = select_dap_adapter(&[], &config, &GlobalConfig::default()).unwrap();

        assert_eq!(bin, PathBuf::from(config["debuggerPath"].as_str().unwrap()));
        assert!(args.is_empty());
    }

    #[test]
    fn detected_lldb_prefers_resolved_dap_adapter() {
        let tmp = tempfile::tempdir().unwrap();
        let lldb = write_fake_dap(&tmp, "lldb");
        let lldb_dap = write_fake_dap(&tmp, "lldb-dap");
        let debuggers = vec![detected_debugger("lldb", lldb, Some(lldb_dap.clone()))];
        let config = json!({ "debugger": "lldb" });

        let (bin, args) =
            select_dap_adapter(&debuggers, &config, &GlobalConfig::default()).unwrap();

        assert_eq!(bin, lldb_dap);
        assert!(args.is_empty());
    }

    #[test]
    fn detected_gdb_is_probed_for_dap_support() {
        let tmp = tempfile::tempdir().unwrap();
        let gdb = write_fake_dap(&tmp, "gdb");
        let debuggers = vec![detected_debugger("gdb", gdb.clone(), None)];
        let config = json!({ "debugger": "gdb" });

        let (bin, args) =
            select_dap_adapter(&debuggers, &config, &GlobalConfig::default()).unwrap();

        assert_eq!(bin, gdb);
        assert_eq!(args, gdb_dap_args());
    }

    #[test]
    fn global_default_debugger_is_used_when_config_omits_debugger() {
        let tmp = tempfile::tempdir().unwrap();
        let gdb = write_fake_dap(&tmp, "gdb");
        let lldb = write_fake_dap(&tmp, "lldb");
        let lldb_dap = write_fake_dap(&tmp, "lldb-dap");
        let debuggers = vec![
            detected_debugger("gdb", gdb, None),
            detected_debugger("lldb", lldb, Some(lldb_dap.clone())),
        ];
        let global = GlobalConfig {
            default_debugger: Some("lldb".to_string()),
            ..GlobalConfig::default()
        };

        let (bin, args) = select_dap_adapter(&debuggers, &json!({}), &global).unwrap();

        assert_eq!(bin, lldb_dap);
        assert!(args.is_empty());
    }

    #[test]
    fn config_and_launch_debugger_args_are_appended_for_gdb() {
        let tmp = tempfile::tempdir().unwrap();
        let gdb = write_fake_dap(&tmp, "gdb");
        let debuggers = vec![detected_debugger("gdb", gdb.clone(), None)];
        let mut global = GlobalConfig::default();
        global.debugger.debuggers.insert(
            "gdb".to_string(),
            DebuggerInstanceConfig {
                args: vec!["--config-arg".to_string()],
                settings: HashMap::new(),
            },
        );
        let config = json!({
            "debugger": "gdb",
            "debuggerArgs": ["--launch-arg"],
        });

        let (bin, args) = select_dap_adapter(&debuggers, &config, &global).unwrap();

        let mut expected = gdb_dap_args();
        expected.push("--config-arg".to_string());
        expected.push("--launch-arg".to_string());
        assert_eq!(bin, gdb);
        assert_eq!(args, expected);
    }

    #[test]
    fn config_and_launch_debugger_args_are_appended_for_lldb_dap() {
        let tmp = tempfile::tempdir().unwrap();
        let lldb = write_fake_dap(&tmp, "lldb");
        let lldb_dap = write_fake_dap(&tmp, "lldb-dap");
        let debuggers = vec![detected_debugger("lldb", lldb, Some(lldb_dap.clone()))];
        let mut global = GlobalConfig::default();
        global.debugger.debuggers.insert(
            "lldb".to_string(),
            DebuggerInstanceConfig {
                args: vec!["--config-arg".to_string()],
                settings: HashMap::new(),
            },
        );
        let config = json!({
            "debugger": "lldb",
            "debuggerArgs": ["--launch-arg"],
        });

        let (bin, args) = select_dap_adapter(&debuggers, &config, &global).unwrap();

        assert_eq!(bin, lldb_dap);
        assert_eq!(args, vec!["--config-arg", "--launch-arg"]);
    }

    #[test]
    fn explicit_non_dap_path_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let not_dap = write_fake_program(&tmp, "not-dap", "printf 'not dap\\n'\n");
        let config = json!({
            "debugger": "gdb",
            "debuggerPath": not_dap,
        });

        let err = select_dap_adapter(&[], &config, &GlobalConfig::default()).unwrap_err();

        assert!(err.to_string().contains("not DAP-capable"));
    }

    #[test]
    fn release_flag_selects_release_profile_when_profile_is_absent() {
        assert_eq!(profile_for_test(&json!({ "release": true })), "release");
        assert_eq!(profile_for_test(&json!({ "release": false })), "debug");
        assert_eq!(
            profile_for_test(&json!({ "release": true, "profile": "custom" })),
            "custom"
        );
    }

    fn profile_for_test(config: &Value) -> String {
        let profile_buf = config_string(config, "profile");
        profile_buf
            .as_deref()
            .unwrap_or_else(|| {
                if config_bool(config, "release").unwrap_or(false) {
                    "release"
                } else {
                    "debug"
                }
            })
            .to_string()
    }

    fn detected_debugger(name: &str, path: PathBuf, dap_path: Option<PathBuf>) -> DetectedDebugger {
        DetectedDebugger {
            template: DebuggerTemplate {
                name: name.to_string(),
                binary: name.to_string(),
                version_arg: "--version".to_string(),
                version_regex: ".*".to_string(),
                launch: LaunchConfig {
                    separator: String::new(),
                },
                dap: DapConfig::default(),
                settings: HashMap::new(),
                default_args: vec![],
            },
            version: "test".to_string(),
            path,
            dap_path,
        }
    }

    fn write_fake_dap(tmp: &tempfile::TempDir, name: &str) -> PathBuf {
        write_fake_program(
            tmp,
            name,
            "printf 'Content-Length: 2\\r\\n\\r\\n{}'\nsleep 1\n",
        )
    }

    fn write_fake_program(tmp: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let path = tmp.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        path
    }
}
