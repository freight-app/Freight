//! Debugger detection and launch helpers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

use crate::manifest::types::{DebuggerConfig, DebuggerInstanceConfig};

// ── Template types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DebuggerTemplate {
    pub name: String,
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    pub launch: LaunchConfig,
    pub dap: DapConfig,
    /// Named settings the template supports. Each key maps to a flag string.
    /// Enabled in the manifest via `[debugger.settings]`.
    pub settings: HashMap<String, String>,
    /// Default extra flags always prepended before the program separator.
    pub default_args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LaunchConfig {
    /// Token inserted between the debugger binary and `<program> [args]`.
    /// `"--"` for LLDB, `"--args"` for GDB.
    pub separator: String,
}

#[derive(Debug, Clone, Default)]
pub struct DapConfig {
    /// Adapter binary names to probe in order (e.g. `["lldb-dap", "lldb-vscode"]`).
    pub binaries: Vec<String>,
    /// VS Code launch.json `"type"` value (e.g. `"lldb"` or `"cppdbg"`).
    pub vscode_type: String,
    /// VS Code `"MIMode"` for `cppdbg` configurations (e.g. `"gdb"` or `"lldb"`).
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
    /// Resolve manifest `[debugger.<name>]` settings into a list of extra flags.
    ///
    /// Flags come from these sources, in order:
    /// 1. `default_args` declared in the template.
    /// 2. `args` from `[debugger.<name>]` in `freight.toml`.
    /// 3. Named `settings` from `[debugger.<name>]` resolved through the template's `settings` map.
    pub fn assemble_flags(&self, cfg: &DebuggerConfig) -> Vec<String> {
        let empty = DebuggerInstanceConfig::default();
        let instance = cfg.debuggers.get(&self.template.name).unwrap_or(&empty);
        let mut flags: Vec<String> = self.template.default_args.clone();
        flags.extend(instance.args.iter().cloned());
        for (key, &enabled) in &instance.settings {
            if enabled {
                if let Some(flag) = self.template.settings.get(key) {
                    flags.extend(flag.split_whitespace().map(String::from));
                }
            }
        }
        flags
    }

    /// Build the `Command` to launch this debugger interactively with `binary`.
    /// `extra_flags` (from [`assemble_flags`]) are inserted before the program separator.
    pub fn launch_command(
        &self,
        binary: &Path,
        extra_flags: &[String],
        args: &[String],
    ) -> Command {
        let mut cmd = Command::new(&self.path);
        cmd.args(extra_flags);
        let sep = &self.template.launch.separator;
        if !sep.is_empty() {
            cmd.arg(sep);
        }
        cmd.arg(binary);
        cmd.args(args);
        cmd
    }

    /// Generate a VS Code `launch.json` configuration object for this debugger.
    pub fn vscode_config(&self, label: &str, binary: &Path) -> serde_json::Value {
        let program = binary.to_string_lossy();
        let vscode_type = &self.template.dap.vscode_type;
        let mi_mode = &self.template.dap.mi_mode;

        if vscode_type == "lldb" {
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
            let dbg_path = self.dap_path.as_ref().unwrap_or(&self.path);
            cfg["miDebuggerPath"] =
                serde_json::Value::String(dbg_path.to_string_lossy().into_owned());
            cfg
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn load_debugger_templates() -> Vec<DebuggerTemplate> {
    vec![gdb(), cuda_gdb(), lldb(), rr(), cdb(), windbg()]
}

fn gdb() -> DebuggerTemplate {
    let mut settings = HashMap::new();
    settings.insert("tui".into(), "--tui".into());
    settings.insert("quiet".into(), "-q".into());
    settings.insert("batch".into(), "--batch".into());
    DebuggerTemplate {
        name: "gdb".into(),
        binary: "gdb".into(),
        version_arg: "--version".into(),
        version_regex: r"GNU gdb[^\d]+(\d+\.\d+)".into(),
        launch: LaunchConfig {
            separator: "--args".into(),
        },
        dap: DapConfig {
            binaries: vec![],
            vscode_type: "cppdbg".into(),
            mi_mode: "gdb".into(),
        },
        settings,
        default_args: vec![],
    }
}

fn lldb() -> DebuggerTemplate {
    let mut settings = HashMap::new();
    settings.insert("no_use_colors".into(), "--no-use-colors".into());
    settings.insert("batch".into(), "--batch".into());
    DebuggerTemplate {
        name: "lldb".into(),
        binary: "lldb".into(),
        version_arg: "--version".into(),
        version_regex: r"\b(\d+\.\d+\.\d+)\b".into(),
        launch: LaunchConfig {
            separator: "--".into(),
        },
        dap: DapConfig {
            binaries: vec!["lldb-dap".into(), "lldb-vscode".into()],
            vscode_type: "lldb".into(),
            mi_mode: "lldb".into(),
        },
        settings,
        default_args: vec![],
    }
}

/// `cuda-gdb` — NVIDIA CUDA debugger; extends GDB with GPU thread/memory support.
/// Activated automatically when a CUDA binary is debugged; requires CUDA toolkit.
fn cuda_gdb() -> DebuggerTemplate {
    let mut settings = HashMap::new();
    settings.insert("quiet".into(), "-q".into());
    DebuggerTemplate {
        name: "cuda-gdb".into(),
        binary: "cuda-gdb".into(),
        version_arg: "--version".into(),
        version_regex: r"NVIDIA cuda-gdb[^\d]+(\d+\.\d+)".into(),
        launch: LaunchConfig {
            separator: "--args".into(),
        },
        dap: DapConfig {
            binaries: vec![],
            vscode_type: "cppdbg".into(),
            mi_mode: "gdb".into(),
        },
        settings,
        default_args: vec![],
    }
}

/// `rr` — Mozilla Record & Replay debugger. Records program execution for
/// deterministic replay. Linux x86-64 only. Use `freight debug --record` to
/// record, then `freight debug --replay` to replay (CLI support pending).
fn rr() -> DebuggerTemplate {
    let mut settings = HashMap::new();
    settings.insert("chaos".into(), "-chaos".into());
    settings.insert("no_syscall_buffer".into(), "--no-syscall-buffer".into());
    DebuggerTemplate {
        name: "rr".into(),
        binary: "rr".into(),
        version_arg: "--version".into(),
        version_regex: r"rr version (\d+\.\d+\.\d+)".into(),
        // rr takes the program as first positional arg; no separator token
        launch: LaunchConfig {
            separator: "replay".into(),
        },
        dap: DapConfig {
            binaries: vec![],
            vscode_type: "cppdbg".into(),
            mi_mode: "gdb".into(),
        },
        settings,
        default_args: vec![],
    }
}

/// `cdb` — Windows Console Debugger (part of Debugging Tools for Windows).
/// Compatible with WinDbg command set; useful in CI/headless environments.
fn cdb() -> DebuggerTemplate {
    let mut settings = HashMap::new();
    settings.insert("nologo".into(), "-nologo".into());
    settings.insert("lines".into(), "-lines".into());
    DebuggerTemplate {
        name: "cdb".into(),
        binary: "cdb.exe".into(),
        version_arg: "".into(),
        version_regex: r"(\d+\.\d+\.\d+\.\d+)".into(),
        launch: LaunchConfig {
            separator: "-o".into(),
        },
        dap: DapConfig {
            binaries: vec![],
            vscode_type: "cppdbg".into(),
            mi_mode: "".into(),
        },
        settings,
        default_args: vec!["-nologo".into()],
    }
}

/// `windbg` — Windows Debugger GUI (Debugging Tools for Windows).
fn windbg() -> DebuggerTemplate {
    DebuggerTemplate {
        name: "windbg".into(),
        binary: "windbg.exe".into(),
        version_arg: "".into(),
        version_regex: r"(\d+\.\d+\.\d+\.\d+)".into(),
        launch: LaunchConfig {
            separator: "--".into(),
        },
        dap: DapConfig {
            binaries: vec![],
            vscode_type: "cppdbg".into(),
            mi_mode: "".into(),
        },
        settings: HashMap::new(),
        default_args: vec![],
    }
}

/// Probe PATH for each debugger template binary and return detected debuggers.
pub fn detect_debuggers(templates: &[DebuggerTemplate]) -> Vec<DetectedDebugger> {
    let mut found = Vec::new();
    for template in templates {
        let Some(path) = which(&template.binary) else {
            continue;
        };
        let version = query_version(template, &path).unwrap_or_else(|| "unknown".into());
        let dap_path = template.dap.binaries.iter().find_map(|b| which(b));
        found.push(DetectedDebugger {
            template: template.clone(),
            version,
            path,
            dap_path,
        });
    }
    found
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn which(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
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
    let out = Command::new(path)
        .arg(&template.version_arg)
        .output()
        .ok()?;
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
