use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::CraneError;
use crate::manifest::types::Manifest;
use crate::toolchain::DetectedCompiler;
use super::compile::{resolve_compile_binary, select_compiler, settings_for_lang, object_path};
use super::discover::SourceFile;

#[derive(Debug, Serialize)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    pub arguments: Vec<String>,
    pub output: PathBuf,
}

/// Build a compile_commands.json entry for every source file.
///
/// `include_dirs` should be the fully-resolved set (project's own inc/ plus
/// all dep include dirs); callers that don't have dep dirs yet can pass just
/// the project-local dirs — clangd will still work for the root sources.
pub fn generate(
    project_dir: &Path,
    manifest: &Manifest,
    detected: &[DetectedCompiler],
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    feature_defines: &[String],
) -> Vec<CompileCommand> {
    let abs_dir = project_dir.canonicalize().unwrap_or_else(|_| project_dir.to_path_buf());
    // Prefix used to relativize any path under the project root.
    let prefix = format!("{}/", abs_dir.to_string_lossy());
    // Strip abs_dir prefix from a single argument string.
    // Handles both standalone paths and single-token flags like `-I/abs/path`.
    let rel = |s: String| -> String {
        // Standalone path (e.g. the source file or -o target).
        if let Some(rest) = s.strip_prefix(&prefix as &str) {
            return rest.to_owned();
        }
        // Single-token flags like -I/abs/path → -Irelative/path.
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
            select_compiler(&source.lang_key, &manifest.compiler.backend, detected)
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
        args.extend(compiler.template.compile_only_flag());
        // Source and output use paths relative to `directory` so the file is
        // portable when the project is moved or shared across machines.
        args.push(source.path.to_string_lossy().into_owned());
        args.extend(compiler.template.output_flag(&obj).into_iter().map(rel));

        // Make all remaining absolute paths under the project root relative;
        // this covers -I flags that assemble_flags built from include_dirs.
        let args: Vec<String> = args.into_iter().map(rel).collect();

        commands.push(CompileCommand {
            directory: abs_dir.clone(),
            file: source.path.clone(),
            arguments: args,
            output: PathBuf::from(rel(obj.to_string_lossy().into_owned())),
        });
    }

    commands
}

/// Serialise `commands` to `<project_dir>/compile_commands.json`.
pub fn write(project_dir: &Path, commands: &[CompileCommand]) -> Result<(), CraneError> {
    let path = project_dir.join("compile_commands.json");
    let json = serde_json::to_string_pretty(commands)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    std::fs::write(path, json)?;
    Ok(())
}
