//! Debugger detection and launch helpers.
//!
//! Debugger templates live in `toolchains/debuggers/` (a subdirectory of the
//! compiler templates directory). Each `.toml` file describes how to invoke a
//! debugger and, optionally, its DAP adapter for IDE integration.

use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde::Deserialize;

use super::detect::{templates_dir};

// ── Template types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct DebuggerTemplate {
    pub name: String,
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    pub launch: LaunchConfig,
    #[serde(default)]
    pub dap: DapConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LaunchConfig {
    /// Token inserted between the debugger binary and `<program> [args]`.
    /// `"--"` for LLDB, `"--args"` for GDB.
    pub separator: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DapConfig {
    /// Adapter binary names to probe in order (e.g. `["lldb-dap", "lldb-vscode"]`).
    #[serde(default)]
    pub binaries: Vec<String>,
    /// VS Code launch.json `"type"` value (e.g. `"lldb"` or `"cppdbg"`).
    #[serde(default)]
    pub vscode_type: String,
    /// VS Code `"MIMode"` for `cppdbg` configurations (e.g. `"gdb"` or `"lldb"`).
    #[serde(default)]
    pub mi_mode: String,
}

// ── Detected debugger ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DetectedDebugger {
    pub template: DebuggerTemplate,
    pub version: String,
    pub path: PathBuf,
    /// Resolved DAP adapter path, if one of `dap.binaries` is on PATH.
    pub dap_path: Option<PathBuf>,
}

impl DetectedDebugger {
    /// Build the `Command` to launch this debugger interactively with `binary`.
    /// Args are appended after the binary (passed to the debugged program).
    pub fn launch_command(&self, binary: &Path, args: &[String]) -> Command {
        let mut cmd = Command::new(&self.path);
        let sep = &self.template.launch.separator;
        if !sep.is_empty() {
            cmd.arg(sep);
        }
        cmd.arg(binary);
        cmd.args(args);
        cmd
    }

    /// Generate a VS Code `launch.json` configuration object for this debugger.
    ///
    /// `name` is the human-readable label; `binary` is the built executable path.
    pub fn vscode_config(&self, label: &str, binary: &Path) -> serde_json::Value {
        let program = binary.to_string_lossy();
        let vscode_type = &self.template.dap.vscode_type;
        let mi_mode = &self.template.dap.mi_mode;

        if vscode_type == "lldb" {
            // CodeLLDB extension format
            serde_json::json!({
                "name": label,
                "type": "lldb",
                "request": "launch",
                "program": program,
                "args": [],
                "cwd": "${workspaceFolder}",
                "generatedBy": "freight"
            })
        } else {
            // Microsoft C/C++ extension (cppdbg) format — works with both GDB and LLDB
            let mut cfg = serde_json::json!({
                "name": label,
                "type": "cppdbg",
                "request": "launch",
                "program": program,
                "args": [],
                "cwd": "${workspaceFolder}",
                "externalConsole": false,
                "generatedBy": "freight"
            });
            if !mi_mode.is_empty() {
                cfg["MIMode"] = serde_json::Value::String(mi_mode.clone());
            }
            if let Some(dap) = &self.dap_path {
                cfg["miDebuggerPath"] = serde_json::Value::String(
                    dap.to_string_lossy().into_owned()
                );
            } else {
                cfg["miDebuggerPath"] = serde_json::Value::String(
                    self.path.to_string_lossy().into_owned()
                );
            }
            cfg
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Return the `toolchains/debuggers/` directory, if it exists.
pub fn debuggers_dir() -> Option<PathBuf> {
    let d = templates_dir()?.join("debuggers");
    if d.is_dir() { Some(d) } else { None }
}

/// Load all debugger templates from the `toolchains/debuggers/` directory.
pub fn load_debugger_templates() -> Vec<DebuggerTemplate> {
    let Some(dir) = debuggers_dir() else { return vec![] };

    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut templates = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        match toml_edit::de::from_str::<DebuggerTemplate>(&src) {
            Ok(t) => templates.push(t),
            Err(e) => eprintln!(
                "warn: skipping debugger template {:?}: {e}",
                path.file_name().unwrap_or_default()
            ),
        }
    }

    templates.sort_by(|a, b| a.name.cmp(&b.name));
    templates
}

/// Probe PATH for each debugger template binary and return detected debuggers.
pub fn detect_debuggers(templates: &[DebuggerTemplate]) -> Vec<DetectedDebugger> {
    let mut found = Vec::new();
    for template in templates {
        let Some(path) = which(&template.binary) else { continue };
        let version = query_version(template, &path).unwrap_or_else(|| "unknown".into());
        let dap_path = template.dap.binaries.iter().find_map(|b| which(b));
        found.push(DetectedDebugger { template: template.clone(), version, path, dap_path });
    }
    found
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn which(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() { return Some(candidate); }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if candidate.exists() {
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}

fn query_version(template: &DebuggerTemplate, path: &Path) -> Option<String> {
    let out = Command::new(path).arg(&template.version_arg).output().ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let re = Regex::new(&template.version_regex).ok()?;
    re.captures(&text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
}
