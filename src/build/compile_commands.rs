use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::compile::{object_path, resolve_compile_binary, select_compiler, settings_for_lang};
use super::discover::SourceFile;
use crate::error::FreightError;
use crate::manifest::types::{Backend, Manifest};
use crate::toolchain::DetectedCompiler;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CompileCommandsCache {
    signature: u64,
    sources: HashMap<PathBuf, SourceStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SourceStamp {
    mtime_secs: u64,
    mtime_nanos: u32,
}

/// Load an existing `compile_commands.json`.  Returns an empty vec on any error
/// (missing file, parse error) so callers can treat it as an optional cache.
pub fn load(project_dir: &Path) -> Vec<CompileCommand> {
    load_from(&project_dir.join("compile_commands.json"))
}

/// Load a compile database from an explicit file path.
pub fn load_from(path: &Path) -> Vec<CompileCommand> {
    let Ok(bytes) = std::fs::read(path) else {
        return vec![];
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Merge `new_entries` into `existing`, keyed on the canonical file path.
///
/// Entries in `existing` whose file is NOT present in `new_entries` are kept
/// as-is (useful when workspace members each own a subset of the full DB).
/// The result is sorted by file path for deterministic, diff-friendly output.
pub fn merge(
    existing: Vec<CompileCommand>,
    new_entries: Vec<CompileCommand>,
) -> Vec<CompileCommand> {
    let mut by_file: HashMap<PathBuf, CompileCommand> =
        existing.into_iter().map(|c| (c.file.clone(), c)).collect();
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
    target_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    extra_flags: &[String],
) -> Vec<CompileCommand> {
    let abs_dir = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
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
        let Some(compiler) = select_compiler(&source.lang_key, backend, detected, None) else {
            continue;
        };

        let settings = settings_for_lang(
            manifest,
            profile,
            &source.lang_key,
            include_dirs,
            project_dir,
            feature_defines,
        );
        let compile_bin = resolve_compile_binary(compiler, &source.lang_key);
        let obj = object_path(target_dir, profile, &source.path);

        let mut args = vec![compile_bin.to_string_lossy().into_owned()];
        args.extend(compiler.template.assemble_flags(&settings));
        args.extend(clangd_only_flags(&source.lang_key));
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

fn clangd_only_flags(lang_key: &str) -> Vec<String> {
    if matches!(
        lang_key,
        "c" | "cpp" | "objc" | "objcpp" | "cuda" | "hip" | "opencl"
    ) {
        vec!["-Wno-gnu-include-next".to_string()]
    } else {
        vec![]
    }
}

/// Incrementally update compile commands by reusing existing entries for
/// unchanged sources and regenerating only missing, dirty, or invalidated ones.
pub fn generate_incremental(
    project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    extra_flags: &[String],
    dirty_sources: Option<&[PathBuf]>,
) -> Vec<CompileCommand> {
    let signature = generation_signature(
        manifest,
        backend,
        detected,
        profile,
        include_dirs,
        feature_defines,
        extra_flags,
    );
    let previous_cache = load_cache(project_dir, profile);
    let signature_changed = previous_cache
        .as_ref()
        .is_none_or(|cache| cache.signature != signature);
    let existing = load(project_dir);

    if existing.is_empty() || signature_changed {
        return generate(
            project_dir,
            target_dir,
            manifest,
            backend,
            detected,
            profile,
            sources,
            include_dirs,
            feature_defines,
            extra_flags,
        );
    }

    let source_paths: HashSet<PathBuf> = sources.iter().map(|src| src.path.clone()).collect();
    let existing_files: HashSet<PathBuf> =
        existing.iter().map(|entry| entry.file.clone()).collect();
    let dirty: HashSet<PathBuf> = dirty_sources
        .map(|paths| paths.iter().cloned().collect())
        .unwrap_or_else(|| {
            dirty_sources_from_stamps(project_dir, sources, previous_cache.as_ref())
        });

    let to_regenerate: Vec<SourceFile> = sources
        .iter()
        .filter(|src| dirty.contains(&src.path) || !existing_files.contains(&src.path))
        .cloned()
        .collect();

    let retained: Vec<CompileCommand> = existing
        .into_iter()
        .filter(|entry| source_paths.contains(&entry.file) && !dirty.contains(&entry.file))
        .collect();

    if to_regenerate.is_empty() {
        let mut retained = retained;
        retained.sort_by(|a, b| a.file.cmp(&b.file));
        return retained;
    }

    merge(
        retained,
        generate(
            project_dir,
            target_dir,
            manifest,
            backend,
            detected,
            profile,
            &to_regenerate,
            include_dirs,
            feature_defines,
            extra_flags,
        ),
    )
}

/// Serialise `commands` to `<project_dir>/compile_commands.json` and refresh
/// the sidecar cache used by [`generate_incremental`].
///
/// Skips the JSON write when the on-disk content is already identical so that
/// LSP servers (clangd, fortls, serve-d…) are not woken up unnecessarily on
/// incremental builds where nothing changed.
pub fn write_incremental_cache(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    extra_flags: &[String],
) -> Result<(), FreightError> {
    let signature = generation_signature(
        manifest,
        backend,
        detected,
        profile,
        include_dirs,
        feature_defines,
        extra_flags,
    );
    let cache = CompileCommandsCache {
        signature,
        sources: sources
            .iter()
            .filter_map(|src| {
                source_stamp(project_dir, &src.path).map(|stamp| (src.path.clone(), stamp))
            })
            .collect(),
    };
    save_cache(project_dir, profile, &cache)
}

/// Serialise `commands` to `<project_dir>/compile_commands.json`.
pub fn write(project_dir: &Path, commands: &[CompileCommand]) -> Result<(), FreightError> {
    write_to(&project_dir.join("compile_commands.json"), commands)
}

/// Serialise `commands` to an explicit compile database path.
/// Compiler flags the real (GCC) compiler needs but clangd's clang front-end
/// rejects with "unknown argument". The generated `compile_commands.json` is
/// consumed only by clangd, so these are stripped on write. (`-fmodules-ts`
/// enables GCC's modules; clang dropped it in favour of standard module flags.)
const CLANGD_INCOMPATIBLE_FLAGS: &[&str] = &["-fmodules-ts"];

/// Drop flags clangd can't parse from each command (rebuilding the joined
/// `command` string to match the filtered `arguments`).
fn sanitize_for_clangd(commands: &[CompileCommand]) -> Vec<CompileCommand> {
    commands
        .iter()
        .map(|c| {
            if !c
                .arguments
                .iter()
                .any(|a| CLANGD_INCOMPATIBLE_FLAGS.contains(&a.as_str()))
            {
                return c.clone();
            }
            let mut c = c.clone();
            c.arguments
                .retain(|a| !CLANGD_INCOMPATIBLE_FLAGS.contains(&a.as_str()));
            c.command = c.arguments.join(" ");
            c
        })
        .collect()
}

pub fn write_to(path: &Path, commands: &[CompileCommand]) -> Result<(), FreightError> {
    let commands = sanitize_for_clangd(commands);
    let json = serde_json::to_string_pretty(&commands)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    // Only write when content actually changed.
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == json {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{clangd_only_flags, sanitize_for_clangd, CompileCommand};
    use std::path::PathBuf;

    #[test]
    fn sanitize_strips_gcc_only_module_flag() {
        let cmd = CompileCommand {
            directory: PathBuf::from("/p"),
            file: PathBuf::from("src/a.cpp"),
            command: "g++ -std=c++20 -fmodules-ts -c src/a.cpp -o a.o".to_string(),
            arguments: vec![
                "g++".into(),
                "-std=c++20".into(),
                "-fmodules-ts".into(),
                "-c".into(),
                "src/a.cpp".into(),
                "-o".into(),
                "a.o".into(),
            ],
            output: PathBuf::from("a.o"),
        };
        let out = sanitize_for_clangd(&[cmd]);
        assert!(
            !out[0].arguments.iter().any(|a| a == "-fmodules-ts"),
            "clangd-incompatible -fmodules-ts must be removed from arguments"
        );
        assert!(
            !out[0].command.contains("-fmodules-ts"),
            "and from the joined command string"
        );
        // Other flags are untouched.
        assert!(out[0].arguments.iter().any(|a| a == "-std=c++20"));
    }

    #[test]
    fn c_family_compile_commands_suppress_include_next_extension_warning() {
        for lang in ["c", "cpp", "objc", "objcpp", "cuda", "hip", "opencl"] {
            assert!(
                clangd_only_flags(lang).contains(&"-Wno-gnu-include-next".to_string()),
                "{lang} should suppress clangd #include_next extension noise"
            );
        }
    }

    #[test]
    fn non_clangd_languages_do_not_get_clang_warning_flags() {
        for lang in ["fortran", "ada", "d", "asm", "ispc", "zig"] {
            assert!(
                clangd_only_flags(lang).is_empty(),
                "{lang} should not receive clang-only diagnostic flags"
            );
        }
    }
}

fn generation_signature(
    manifest: &Manifest,
    backend: &Backend,
    detected: &[DetectedCompiler],
    profile: &str,
    include_dirs: &[PathBuf],
    feature_defines: &[String],
    extra_flags: &[String],
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    profile.hash(&mut hasher);
    backend.0.hash(&mut hasher);
    serde_json::to_string(manifest)
        .unwrap_or_default()
        .hash(&mut hasher);
    include_dirs.hash(&mut hasher);
    feature_defines.hash(&mut hasher);
    extra_flags.hash(&mut hasher);
    for compiler in detected {
        compiler.path.hash(&mut hasher);
        compiler.template.name.hash(&mut hasher);
        compiler.template.family.hash(&mut hasher);
        compiler.version.hash(&mut hasher);
    }
    hasher.finish()
}

fn dirty_sources_from_stamps(
    project_dir: &Path,
    sources: &[SourceFile],
    cache: Option<&CompileCommandsCache>,
) -> HashSet<PathBuf> {
    let Some(cache) = cache else {
        return sources.iter().map(|src| src.path.clone()).collect();
    };
    sources
        .iter()
        .filter(|src| cache.sources.get(&src.path) != source_stamp(project_dir, &src.path).as_ref())
        .map(|src| src.path.clone())
        .collect()
}

fn source_stamp(project_dir: &Path, source: &Path) -> Option<SourceStamp> {
    let modified = project_dir.join(source).metadata().ok()?.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(SourceStamp {
        mtime_secs: duration.as_secs(),
        mtime_nanos: duration.subsec_nanos(),
    })
}

fn cache_path(project_dir: &Path, profile: &str) -> PathBuf {
    project_dir
        .join("target")
        .join(profile)
        .join("compile_commands.cache.json")
}

fn load_cache(project_dir: &Path, profile: &str) -> Option<CompileCommandsCache> {
    let path = cache_path(project_dir, profile);
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_cache(
    project_dir: &Path,
    profile: &str,
    cache: &CompileCommandsCache,
) -> Result<(), FreightError> {
    let path = cache_path(project_dir, profile);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    std::fs::write(path, json)?;
    Ok(())
}
