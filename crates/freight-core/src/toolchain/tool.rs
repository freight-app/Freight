//! Formatter and linter template loading and detection.
//!
//! Templates live alongside their compiler family in `toolchains/` and use
//! `kind = "formatter"` or `kind = "linter"` to identify themselves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use rhai::{Array, Dynamic, Engine, Map, Scope};

use super::detect::templates_dir;
use super::script::quick_kind;
use crate::error::FreightError;
use crate::manifest::types::{FormatterConfig, LinterConfig};

// ── Template types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolTemplate {
    /// `"formatter"` or `"linter"`.
    pub kind: String,
    pub name: String,
    pub family: String,
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    /// File extensions this tool acts on (e.g. `[".cpp", ".h"]`).
    pub extensions: Vec<String>,
    /// Run-mode flags: `"fix"` → apply changes, `"check"` → report only.
    pub run: HashMap<String, String>,
    /// Named settings resolved from `freight.toml`. Each value is a flag
    /// pattern with `{value}` substituted from the manifest setting.
    pub settings: HashMap<String, String>,
    /// Valid values for each setting key, for LSP completions and help output.
    /// Keys with freeform values (paths, numbers, regex strings) are omitted.
    pub values: HashMap<String, Vec<String>>,
}

impl ToolTemplate {
    /// Assemble flags from `freight.toml` config for the given run mode.
    ///
    /// Order: mode flags → per-setting flags from the manifest.
    pub fn assemble_flags(&self, settings: &HashMap<String, String>, mode: &str) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();
        if let Some(mode_flags) = self.run.get(mode) {
            flags.extend(mode_flags.split_whitespace().map(String::from).filter(|s| !s.is_empty()));
        }
        for (key, value) in settings {
            if let Some(pattern) = self.settings.get(key) {
                let flag = pattern.replace("{value}", value);
                flags.extend(flag.split_whitespace().map(String::from).filter(|s| !s.is_empty()));
            }
        }
        flags
    }
}

// ── Detected tool ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DetectedTool {
    pub template: ToolTemplate,
    pub version: String,
    pub path: PathBuf,
}

impl DetectedTool {
    /// Build a `Command` for running this tool in the given mode over `files`.
    pub fn command(&self, settings: &HashMap<String, String>, mode: &str, files: &[PathBuf]) -> Command {
        let mut cmd = Command::new(&self.path);
        cmd.args(self.template.assemble_flags(settings, mode));
        cmd.args(files);
        cmd
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn load_formatter_templates() -> Vec<ToolTemplate> {
    let Some(dir) = templates_dir() else { return vec![] };
    load_tool_templates_from(&dir, "formatter")
}

pub fn load_linter_templates() -> Vec<ToolTemplate> {
    let Some(dir) = templates_dir() else { return vec![] };
    load_tool_templates_from(&dir, "linter")
}

pub fn load_tool_templates_from(dir: &Path, kind: &str) -> Vec<ToolTemplate> {
    let mut templates = Vec::new();

    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rhai") { continue; }
        if path.file_name().and_then(|n| n.to_str())
            .map(|n| n.starts_with('_')).unwrap_or(false) { continue; }
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        if quick_kind(&src) != kind { continue; }
        match eval_tool_rhai(&src, path.parent()) {
            Ok(t) => templates.push(t),
            Err(e) => eprintln!("warn: skipping {} {:?}: {e}", kind, path.file_name().unwrap_or_default()),
        }
    }

    templates.sort_by(|a, b| a.name.cmp(&b.name));
    templates
}

pub fn detect_tools(templates: &[ToolTemplate]) -> Vec<DetectedTool> {
    let mut found = Vec::new();
    for template in templates {
        let Some(path) = which(&template.binary) else { continue };
        let version = query_version(template, &path).unwrap_or_else(|| "unknown".into());
        found.push(DetectedTool { template: template.clone(), version, path });
    }
    found
}

// ── Manifest helpers ──────────────────────────────────────────────────────────

/// Pick the formatter matching `config.name`, or the first detected one.
pub fn select_formatter<'a>(
    detected: &'a [DetectedTool],
    config: &FormatterConfig,
) -> Option<&'a DetectedTool> {
    if let Some(name) = &config.name {
        detected.iter().find(|t| &t.template.name == name)
    } else {
        detected.first()
    }
}

/// Pick the linter matching `config.name`, or the first detected one.
pub fn select_linter<'a>(
    detected: &'a [DetectedTool],
    config: &LinterConfig,
) -> Option<&'a DetectedTool> {
    if let Some(name) = &config.name {
        detected.iter().find(|t| &t.template.name == name)
    } else {
        detected.first()
    }
}

/// Walk `src_dir` and return all files whose extension is in `extensions`.
pub fn collect_sources(src_dir: &Path, extensions: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(src_dir).into_iter().flatten() {
        let path = entry.path();
        if !path.is_file() { continue; }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extensions.iter().any(|e| e.trim_start_matches('.') == ext) {
            files.push(path.to_path_buf());
        }
    }
    files
}

// ── Rhai evaluation ───────────────────────────────────────────────────────────

fn eval_tool_rhai(src: &str, dir: Option<&Path>) -> Result<ToolTemplate, FreightError> {
    let mut engine = Engine::new();

    if let Some(base_dir) = dir.map(|d| d.to_path_buf()) {
        engine.register_custom_syntax(
            &["include", "$string$"],
            true,
            move |context: &mut rhai::EvalContext, inputs: &[rhai::Expression]| {
                let rel = inputs[0]
                    .get_string_value()
                    .ok_or_else(|| -> Box<rhai::EvalAltResult> {
                        "include: expected a string literal".into()
                    })?;
                let mut path = base_dir.join(rel);
                if path.extension().is_none() { path.set_extension("rhai"); }
                let src = std::fs::read_to_string(&path).map_err(|e| -> Box<rhai::EvalAltResult> {
                    format!("include: cannot read {}: {e}", path.display()).into()
                })?;
                let ast = context.engine().compile(&src).map_err(|e| -> Box<rhai::EvalAltResult> {
                    format!("include parse error: {e}").into()
                })?;
                let engine = context.engine();
                let scope  = context.scope_mut();
                engine.run_ast_with_scope(scope, &ast)?;
                Ok(Dynamic::UNIT)
            },
        ).map_err(|e| FreightError::TemplateError(format!("custom syntax error: {e}")))?;
    }

    let ast = engine
        .compile(src)
        .map_err(|e| FreightError::TemplateError(format!("tool script compile error: {e}")))?;

    let mut scope = Scope::new();
    for key in &["kind", "name", "family", "binary", "version_arg", "version_regex"] {
        scope.push(*key, String::new());
    }
    scope.push("extensions",  Array::new());
    scope.push("run",         Map::new());
    scope.push("settings",    Map::new());
    scope.push("values",      Map::new());

    engine
        .run_ast_with_scope(&mut scope, &ast)
        .map_err(|e| FreightError::TemplateError(format!("tool script error: {e}")))?;

    macro_rules! str { ($k:expr) => { scope.get_value::<String>($k).unwrap_or_default() }; }
    macro_rules! map { ($k:expr) => { scope.get_value::<Map>($k).unwrap_or_default() }; }

    let kind = str!("kind");
    if kind != "formatter" && kind != "linter" {
        return Err(FreightError::TemplateError(format!("not a formatter or linter template (kind = {kind:?})")));
    }

    let extensions: Vec<String> = scope
        .get_value::<Array>("extensions")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.try_cast::<String>())
        .collect();

    fn str_map(m: Map) -> HashMap<String, String> {
        m.into_iter()
            .filter_map(|(k, v)| v.try_cast::<String>().map(|s| (k.to_string(), s)))
            .collect()
    }

    fn values_map(m: Map) -> HashMap<String, Vec<String>> {
        m.into_iter()
            .filter_map(|(k, v)| {
                v.try_cast::<Array>().map(|arr| {
                    let vals = arr.into_iter().filter_map(|v| v.try_cast::<String>()).collect();
                    (k.to_string(), vals)
                })
            })
            .collect()
    }

    Ok(ToolTemplate {
        kind,
        name:          str!("name"),
        family:        str!("family"),
        binary:        str!("binary"),
        version_arg:   str!("version_arg"),
        version_regex: str!("version_regex"),
        extensions,
        run:      str_map(map!("run")),
        settings: str_map(map!("settings")),
        values:   values_map(map!("values")),
    })
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn which(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() { return Some(candidate); }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() { return Some(exe); }
        }
    }
    None
}

fn query_version(template: &ToolTemplate, path: &Path) -> Option<String> {
    if template.version_arg.is_empty() { return None; }
    let out = Command::new(path).arg(&template.version_arg).output().ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let re = Regex::new(&template.version_regex).ok()?;
    re.captures(&text)?.get(1).map(|m| m.as_str().to_string())
}
