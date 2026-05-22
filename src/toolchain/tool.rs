//! Formatter and linter template loading and detection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

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
    vec![clang_format()]
}

pub fn load_linter_templates() -> Vec<ToolTemplate> {
    vec![clang_tidy()]
}

fn clang_format() -> ToolTemplate {
    let mut run = HashMap::new();
    run.insert("fix".into(), "-i".into());
    run.insert("check".into(), "--dry-run --Werror".into());
    let mut settings = HashMap::new();
    settings.insert("style".into(), "--style={value}".into());
    settings.insert("config".into(), "--style=file:{value}".into());
    let mut values = HashMap::new();
    values.insert("style".into(), vec![
        "Google".into(), "LLVM".into(), "Mozilla".into(), "WebKit".into(),
        "Chromium".into(), "Microsoft".into(), "GNU".into(), "file".into(),
    ]);
    ToolTemplate {
        kind: "formatter".into(),
        name: "clang-format".into(),
        family: "llvm".into(),
        binary: "clang-format".into(),
        version_arg: "--version".into(),
        version_regex: r"clang-format version (\d+\.\d+\.\d+)".into(),
        extensions: vec![
            ".cpp".into(), ".cc".into(), ".cxx".into(), ".c++".into(), ".cppm".into(),
            ".c".into(), ".h".into(), ".hpp".into(), ".hxx".into(), ".cu".into(), ".hip".into(),
        ],
        run, settings, values,
    }
}

fn clang_tidy() -> ToolTemplate {
    let mut run = HashMap::new();
    run.insert("check".into(), "".into());
    run.insert("fix".into(), "--fix --fix-errors".into());
    let mut settings = HashMap::new();
    settings.insert("checks".into(), "--checks={value}".into());
    settings.insert("config".into(), "--config-file={value}".into());
    ToolTemplate {
        kind: "linter".into(),
        name: "clang-tidy".into(),
        family: "llvm".into(),
        binary: "clang-tidy".into(),
        version_arg: "--version".into(),
        version_regex: r"LLVM version (\d+\.\d+\.\d+)".into(),
        extensions: vec![
            ".cpp".into(), ".cc".into(), ".cxx".into(), ".c++".into(), ".cppm".into(), ".c".into(),
        ],
        run, settings, values: HashMap::new(),
    }
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
