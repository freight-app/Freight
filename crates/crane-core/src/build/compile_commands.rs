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
        let src_abs = abs_dir.join(&source.path);
        let obj = object_path(project_dir, profile, &source.path);

        let mut args = vec![compile_bin.to_string_lossy().into_owned()];
        args.extend(compiler.template.assemble_flags(&settings));
        args.extend(compiler.template.compile_only_flag());
        args.push(src_abs.to_string_lossy().into_owned());
        args.extend(compiler.template.output_flag(&obj));

        commands.push(CompileCommand {
            directory: abs_dir.clone(),
            file: src_abs,
            arguments: args,
            output: obj,
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
