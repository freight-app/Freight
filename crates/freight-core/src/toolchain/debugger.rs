//! Debugger detection and launch helpers.
//!
//! Debugger templates live alongside their compiler family in the toolchains
//! directory (e.g. `toolchains/gnu/gdb.rhai`, `toolchains/llvm/lldb.rhai`).
//! Each `.rhai` file sets `kind = "debugger"` to identify itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use rhai::{Array, Dynamic, Engine, Map, Scope};

use super::detect::templates_dir;
use super::script::quick_kind;
use crate::error::FreightError;
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
    pub fn launch_command(&self, binary: &Path, extra_flags: &[String], args: &[String]) -> Command {
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
            cfg["miDebuggerPath"] = serde_json::Value::String(
                dbg_path.to_string_lossy().into_owned()
            );
            cfg
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load all debugger templates by walking the toolchains directory for `.rhai`
/// files that declare `kind = "debugger"`.
pub fn load_debugger_templates() -> Vec<DebuggerTemplate> {
    let Some(dir) = templates_dir() else { return vec![] };
    load_debugger_templates_from(&dir)
}

pub fn load_debugger_templates_from(dir: &Path) -> Vec<DebuggerTemplate> {
    let mut templates = Vec::new();

    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rhai") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str())
            .map(|n| n.starts_with('_')).unwrap_or(false)
        {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        if quick_kind(&src) != "debugger" { continue; }
        match eval_debugger_rhai(&src, path.parent()) {
            Ok(t) => templates.push(t),
            Err(e) => eprintln!("warn: skipping debugger {:?}: {e}", path.file_name().unwrap_or_default()),
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

// ── Rhai evaluation ───────────────────────────────────────────────────────────

fn eval_debugger_rhai(src: &str, dir: Option<&Path>) -> Result<DebuggerTemplate, FreightError> {
    let mut engine = Engine::new();

    // Register include "path" so debugger templates can share a base if needed.
    if let Some(base_dir) = dir.map(|d| d.to_path_buf()) {
        engine.register_custom_syntax(
            &["include", "$string$"],
            true,
            move |context: &mut rhai::EvalContext, inputs: &[rhai::Expression]| {
                let path_str = inputs[0]
                    .get_string_value()
                    .ok_or_else(|| -> Box<rhai::EvalAltResult> {
                        "include: expected a string literal".into()
                    })?;
                let p = base_dir.join(path_str);
                let p = if p.extension().is_some() { p } else { p.with_extension("rhai") };
                let src = std::fs::read_to_string(&p)
                    .map_err(|e| -> Box<rhai::EvalAltResult> {
                        format!("include \"{path_str}\": {e}").into()
                    })?;
                let ast = context.engine().compile(&src)
                    .map_err(|e| -> Box<rhai::EvalAltResult> {
                        format!("include \"{path_str}\" compile error: {e}").into()
                    })?;
                let engine = context.engine();
                let scope  = context.scope_mut();
                engine.run_ast_with_scope(scope, &ast)?;
                Ok(Dynamic::UNIT)
            },
        ).expect("failed to register include syntax");
    }

    let ast = engine
        .compile(src)
        .map_err(|e| FreightError::TemplateError(format!("debugger script compile error: {e}")))?;

    let mut scope = Scope::new();
    for key in &["kind", "name", "binary", "version_arg", "version_regex"] {
        scope.push(*key, String::new());
    }
    scope.push("launch",       Map::new());
    scope.push("dap",          Map::new());
    scope.push("settings",     Map::new());
    scope.push("default_args", Array::new());

    engine
        .run_ast_with_scope(&mut scope, &ast)
        .map_err(|e| FreightError::TemplateError(format!("debugger script error: {e}")))?;

    macro_rules! str { ($k:expr) => { scope.get_value::<String>($k).unwrap_or_default() }; }
    macro_rules! map { ($k:expr) => { scope.get_value::<Map>($k).unwrap_or_default() }; }

    if str!("kind") != "debugger" {
        return Err(FreightError::TemplateError("not a debugger template".into()));
    }

    let launch_map = map!("launch");
    let separator = launch_map.get("separator")
        .and_then(|v| v.clone().try_cast::<String>())
        .unwrap_or_default();

    let dap_map = map!("dap");
    let dap_str = |key: &str| -> String {
        dap_map.get(key)
            .and_then(|v| v.clone().try_cast::<String>())
            .unwrap_or_default()
    };
    let dap_binaries: Vec<String> = dap_map.get("binaries")
        .and_then(|v| v.clone().try_cast::<Array>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.try_cast::<String>())
        .collect();

    let settings_map = map!("settings");
    let settings: HashMap<String, String> = settings_map.into_iter()
        .filter_map(|(k, v)| v.try_cast::<String>().map(|s| (k.to_string(), s)))
        .collect();

    let default_args: Vec<String> = scope
        .get_value::<Array>("default_args")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.try_cast::<String>())
        .collect();

    Ok(DebuggerTemplate {
        name:          str!("name"),
        binary:        str!("binary"),
        version_arg:   str!("version_arg"),
        version_regex: str!("version_regex"),
        launch: LaunchConfig { separator },
        dap: DapConfig {
            binaries:    dap_binaries,
            vscode_type: dap_str("vscode_type"),
            mi_mode:     dap_str("mi_mode"),
        },
        settings,
        default_args,
    })
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
