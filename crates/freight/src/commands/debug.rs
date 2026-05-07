//! `freight debug` — build with debug symbols and launch an interactive debugger,
//! or generate IDE launch configurations via `--launch-json`.

use std::path::{Path, PathBuf};

use freight_core::build::{build_project, BuildOutput};
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::{detect_debuggers, load_debugger_templates, GlobalConfig};

use crate::output::{print_error, print_success, print_warning};

// ── Public entry point ────────────────────────────────────────────────────────

/// Build the project (debug profile) and launch an interactive debugger.
///
/// `binary_filter` selects which binary to debug when the project declares
/// multiple `[[bin]]` targets. If `None` and there is exactly one binary, it
/// is used automatically; otherwise an error is printed.
///
/// `debugger_pref` pins a specific debugger by name (e.g. `"gdb"` or `"lldb"`).
/// When `None` the first detected debugger is used.
///
/// If `launch_json` is `true`, no debugger is launched — instead a
/// `.vscode/launch.json` is written (or updated) with one configuration per
/// binary.
pub fn cmd_debug(
    binary_filter: Option<&str>,
    debugger_pref: Option<&str>,
    args: &[String],
    launch_json: bool,
) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let mut global_cfg = GlobalConfig::load();
    if let Some(local) = GlobalConfig::load_local(&project_dir) {
        global_cfg.apply_local(local);
    }

    // ── Detect debuggers ───────────────────────────────────────────────────────
    let templates = load_debugger_templates();
    if templates.is_empty() {
        print_warning("no debugger templates found in toolchains/debuggers/");
    }
    let debuggers = detect_debuggers(&templates);

    if debuggers.is_empty() && !launch_json {
        print_error("no debugger found on PATH — install lldb or gdb");
        return;
    }

    // ── launch.json generation mode ───────────────────────────────────────────
    if launch_json {
        gen_launch_json(&project_dir, &manifest, &debuggers);
        return;
    }

    // ── Select debugger ────────────────────────────────────────────────────────
    let debugger = if let Some(pref) = debugger_pref {
        match debuggers.iter().find(|d| d.template.name == pref) {
            Some(d) => d,
            None => {
                print_error(&format!(
                    "debugger '{pref}' not found on PATH (available: {})",
                    debuggers.iter().map(|d| d.template.name.as_str()).collect::<Vec<_>>().join(", ")
                ));
                return;
            }
        }
    } else {
        &debuggers[0]
    };

    // ── Build with debug profile ───────────────────────────────────────────────
    let output = match build_project("debug", &[], true, &[]) {
        Ok(o) => o,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    // ── Resolve binary ─────────────────────────────────────────────────────────
    let binary = match select_binary(&output, &project_dir, binary_filter, &manifest) {
        Ok(b) => b,
        Err(e) => { print_error(&e); return; }
    };

    use owo_colors::OwoColorize;
    println!(
        "  {} {} with {} {}",
        "Debugging".bold().cyan(),
        binary.file_name().unwrap_or_default().to_str().unwrap_or(""),
        debugger.template.name,
        debugger.version,
    );

    // ── Exec the debugger (replace the freight process on Unix) ──────────────────
    let extra_flags = debugger.assemble_flags(&global_cfg.debugger);
    let mut cmd = debugger.launch_command(&binary, &extra_flags, args);
    exec_or_run(&mut cmd);
}

// ── launch.json generation ────────────────────────────────────────────────────

fn gen_launch_json(
    project_dir: &Path,
    manifest: &freight_core::manifest::types::Manifest,
    debuggers: &[freight_core::toolchain::DetectedDebugger],
) {
    let vscode_dir = project_dir.join(".vscode");
    if let Err(e) = std::fs::create_dir_all(&vscode_dir) {
        print_error(&format!("cannot create .vscode/: {e}"));
        return;
    }

    let launch_path = vscode_dir.join("launch.json");

    // Read existing file so we can preserve non-freight configs.
    let existing: Option<serde_json::Value> = launch_path.exists()
        .then(|| std::fs::read_to_string(&launch_path).ok())
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok());

    // Collect existing non-freight configs.
    let mut kept: Vec<serde_json::Value> = existing
        .as_ref()
        .and_then(|v| v.get("configurations"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|c| c.get("generatedBy").and_then(|v| v.as_str()) != Some("freight"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    // Add one configuration per binary per detected debugger.
    let profile = "debug";
    for bin in &manifest.bins {
        let binary_path = project_dir.join("target").join(profile).join(&bin.name);
        let program = format!("${{workspaceFolder}}/target/{profile}/{}", bin.name);

        for dbg in debuggers {
            let label = if debuggers.len() == 1 {
                format!("Debug {} (freight)", bin.name)
            } else {
                format!("Debug {} ({}, freight)", bin.name, dbg.template.name)
            };

            let mut cfg = dbg.vscode_config(&label, &binary_path);
            // Use the portable ${workspaceFolder} form instead of the absolute path.
            cfg["program"] = serde_json::Value::String(program.clone());
            kept.push(cfg);
        }
    }

    let doc = serde_json::json!({
        "version": "0.2.0",
        "configurations": kept,
    });

    let json = serde_json::to_string_pretty(&doc).unwrap_or_default();
    match std::fs::write(&launch_path, json + "\n") {
        Ok(()) => print_success(&format!("wrote {}", launch_path.display())),
        Err(e) => print_error(&format!("cannot write launch.json: {e}")),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn select_binary(
    output: &BuildOutput,
    project_dir: &Path,
    filter: Option<&str>,
    manifest: &freight_core::manifest::types::Manifest,
) -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = if let Some(name) = filter {
        output.binaries.iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))
            .cloned()
            .collect()
    } else {
        output.binaries.clone()
    };

    match candidates.len() {
        0 if filter.is_some() => Err(format!(
            "no binary named '{}' — available: {}",
            filter.unwrap(),
            manifest.bins.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
        0 => {
            // Fall back: check target/debug/ for the package name binary.
            let fallback = project_dir.join("target").join("debug").join(&manifest.package.name);
            if fallback.exists() {
                Ok(fallback)
            } else {
                Err("no binary built — does the manifest declare [[bin]]?".into())
            }
        }
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => Err(format!(
            "multiple binaries: {}; specify one with `freight debug <name>`",
            candidates.iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// On Unix: exec() replaces the freight process so the terminal connects
/// directly to the debugger (no extra shell wrapping). On Windows: run and
/// forward the exit code.
fn exec_or_run(cmd: &mut std::process::Command) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("error: failed to exec debugger: {err}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        let code = cmd.status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|e| { eprintln!("error: {e}"); 1 });
        std::process::exit(code);
    }
}
