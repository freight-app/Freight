use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::FreightError;
use crate::manifest::types::Manifest;
use crate::toolchain::DetectedCompiler;
use super::compile::{resolve_compile_binary, select_compiler, settings_for_lang, object_path};
use super::discover::SourceFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    /// Space-joined command string. Included alongside `arguments` for
    /// compatibility with older clangd versions and other tools that prefer
    /// the string form over the array form.
    pub command: String,
    pub arguments: Vec<String>,
    pub output: PathBuf,
}

/// Load an existing `compile_commands.json`.  Returns an empty vec on any error
/// (missing file, parse error) so callers can treat it as an optional cache.
pub fn load(project_dir: &Path) -> Vec<CompileCommand> {
    let path = project_dir.join("compile_commands.json");
    let Ok(bytes) = std::fs::read(&path) else { return vec![] };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Merge `new_entries` into `existing`, keyed on the canonical file path.
///
/// Entries in `existing` whose file is NOT present in `new_entries` are kept
/// as-is (useful when workspace members each own a subset of the full DB).
/// The result is sorted by file path for deterministic, diff-friendly output.
pub fn merge(existing: Vec<CompileCommand>, new_entries: Vec<CompileCommand>) -> Vec<CompileCommand> {
    let mut by_file: HashMap<PathBuf, CompileCommand> = existing
        .into_iter()
        .map(|c| (c.file.clone(), c))
        .collect();
    for entry in new_entries {
        by_file.insert(entry.file.clone(), entry);
    }
    let mut merged: Vec<CompileCommand> = by_file.into_values().collect();
    merged.sort_by(|a, b| a.file.cmp(&b.file));
    merged
}

/// Build a `CompileCommand` entry for every source file.
///
/// `include_dirs` should be the fully-resolved set (project's own `include/`
/// plus all dep include dirs); callers that only have the project-local dirs
/// will produce entries that still work for root sources.
///
/// Entries are sorted by file path for stable, diff-friendly output.
pub fn generate(
    project_dir: &Path,
    manifest: &Manifest,
    detected: &[DetectedCompiler],
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    extra_flags: &[String],
) -> Vec<CompileCommand> {
    let abs_dir = project_dir.canonicalize().unwrap_or_else(|_| project_dir.to_path_buf());
    let prefix = format!("{}/", abs_dir.to_string_lossy());

    // Strip the absolute project root from a single argument token so that the
    // database is portable when shared across machines or CI environments.
    // Handles both standalone paths and single-token flags like `-I/abs/path`.
    let rel = |s: String| -> String {
        if let Some(rest) = s.strip_prefix(&prefix as &str) {
            return rest.to_owned();
        }
        for flag in ["-I", "-isystem", "-iframework"] {
            if let Some(path_part) = s.strip_prefix(flag) {
                if let Some(rest) = path_part.strip_prefix(&prefix as &str) {
                    return format!("{flag}{rest}");
                }
            }
        }
        s
    };

    let mut commands = Vec::new();

    for source in sources {
        let Some(compiler) =
            select_compiler(&source.lang_key, &manifest.compiler.backend, detected, None)
        else {
            continue;
        };

        let settings = settings_for_lang(
            manifest, profile, &source.lang_key, include_dirs, project_dir, feature_defines,
        );
        let compile_bin = resolve_compile_binary(compiler, &source.lang_key);
        let obj = object_path(project_dir, profile, &source.path);

        let mut args = vec![compile_bin.to_string_lossy().into_owned()];
        args.extend(compiler.template.assemble_flags(&settings));
        args.extend(extra_flags.iter().cloned());
        args.extend(compiler.template.compile_only_flag());
        // Source and output use paths relative to `directory` so the entry is
        // portable when the project is moved or shared across machines.
        args.push(source.path.to_string_lossy().into_owned());
        args.extend(compiler.template.output_flag(&obj).into_iter().map(rel));

        let args: Vec<String> = args.into_iter().map(rel).collect();
        let command = args.join(" ");

        commands.push(CompileCommand {
            directory: abs_dir.clone(),
            file: source.path.clone(),
            command,
            arguments: args,
            output: PathBuf::from(rel(obj.to_string_lossy().into_owned())),
        });
    }

    commands.sort_by(|a, b| a.file.cmp(&b.file));
    commands
}

/// Serialise `commands` to `<project_dir>/compile_commands.json`.
///
/// Skips the write when the on-disk content is already identical so that LSP
/// servers (clangd, fortls, serve-d…) are not woken up unnecessarily on
/// incremental builds where nothing changed.
pub fn write(project_dir: &Path, commands: &[CompileCommand]) -> Result<(), FreightError> {
    let path = project_dir.join("compile_commands.json");
    let json = serde_json::to_string_pretty(commands)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    // Only write when content actually changed.
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing == json {
            return Ok(());
        }
    }
    std::fs::write(path, json)?;
    Ok(())
}
