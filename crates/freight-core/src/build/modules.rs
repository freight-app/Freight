//! C++20 named-module support: scanning, DAG, and phased compilation.
//!
//! Pipeline:
//!  1. Scan C++ sources for `export module` / `module` / `import` declarations.
//!  2. Build a DAG from import edges between MIUs; Kahn's topo-sort into parallel batches.
//!  3. Compile each MIU batch in parallel, producing one BMI (.pcm) per module.
//!     GCC one-step: `-fmodule-output=`; Clang two-step: `--precompile` then `-c`.
//!  4. Compile MImplUs + regular TUs in parallel, injecting `-fmodule-file=` per import.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rayon::prelude::*;

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Backend, Manifest};
use crate::toolchain::template::{BuildSettings, ModuleStyle};
use crate::toolchain::DetectedCompiler;

use super::compile::{
    compile_one, dep_file_path, is_up_to_date, object_path,
    resolve_compile_binary, select_compiler, settings_for_lang, CompileResult,
};
use super::diagnostics::format_compiler_diagnostics;
use super::discover::SourceFile;

// ── Public types ──────────────────────────────────────────────────────────────

/// The role a C++ source file plays in the module system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleRole {
    /// `export module foo;` — declares and exports module `foo`.
    Interface(String),
    /// `module foo;` (no export) — provides additional implementation for module `foo`.
    Implementation(String),
    /// No module declaration — ordinary translation unit.
    Regular,
}

/// A discovered source file annotated with its module role and imports.
#[derive(Debug, Clone)]
pub struct ScannedSource {
    pub source: SourceFile,
    pub role: ModuleRole,
    /// Named C++20 modules this file imports (header units skipped).
    pub imports: Vec<String>,
}

/// Result of analysing the module dependency graph.
pub struct ModuleBuildPlan {
    /// MIUs in topological order; each inner Vec is a batch that can compile in parallel.
    pub miu_batches: Vec<Vec<ScannedSource>>,
    /// MImplUs and ordinary TUs — compiled after all MIU batches finish.
    pub rest: Vec<ScannedSource>,
    /// Module name → BMI path. Populated incrementally as MIU batches complete.
    pub module_bmi_map: HashMap<String, PathBuf>,
}

// ── Scanning ──────────────────────────────────────────────────────────────────

/// Scan source files and annotate each with its module role and imports.
/// Only `cpp` lang-key files are inspected; all others get `ModuleRole::Regular`.
pub fn scan_sources(project_dir: &Path, sources: &[SourceFile]) -> Vec<ScannedSource> {
    sources.iter().map(|src| {
        if src.lang_key != "cpp" {
            return ScannedSource { source: src.clone(), role: ModuleRole::Regular, imports: vec![] };
        }
        scan_file(project_dir, src)
    }).collect()
}

/// Return `true` if any scanned source is a module interface unit.
pub fn has_modules(scanned: &[ScannedSource]) -> bool {
    scanned.iter().any(|s| matches!(s.role, ModuleRole::Interface(_)))
}

fn scan_file(project_dir: &Path, src: &SourceFile) -> ScannedSource {
    let path = project_dir.join(&src.path);
    let Ok(content) = fs::read_to_string(&path) else {
        return ScannedSource { source: src.clone(), role: ModuleRole::Regular, imports: vec![] };
    };

    let mut role = ModuleRole::Regular;
    let mut imports = Vec::new();

    for raw in content.lines().take(300) {
        let line = strip_line_comment(raw).trim();
        if line.is_empty() || line == "module;" { continue; } // global module fragment
        if line.starts_with('#') { continue; }                // preprocessor directives

        if let Some(name) = parse_export_module(line) {
            role = ModuleRole::Interface(name);
        } else if let Some(name) = parse_module_impl(line) {
            if role == ModuleRole::Regular {
                role = ModuleRole::Implementation(name);
            }
        } else if line.starts_with("import") {
            // Could be a named import, a header unit (`import <foo>`, `import "foo"`),
            // or a partition import — all are preamble-legal; only record named ones.
            if let Some(name) = parse_import(line) {
                imports.push(name);
            }
        } else {
            // Real code — module preamble is over.
            break;
        }
    }

    ScannedSource { source: src.clone(), role, imports }
}

fn strip_line_comment(line: &str) -> &str {
    line.find("//").map_or(line, |i| &line[..i])
}

fn parse_export_module(line: &str) -> Option<String> {
    let rest = line.strip_prefix("export")?.trim_start();
    let rest = rest.strip_prefix("module")?.trim_start();
    let name = rest.strip_suffix(';')?.trim();
    if name.is_empty() || name.contains(':') { return None; } // skip partitions for now
    Some(name.to_owned())
}

fn parse_module_impl(line: &str) -> Option<String> {
    if line.starts_with("export") { return None; }
    let rest = line.strip_prefix("module")?.trim_start();
    if rest == ";" || rest.is_empty() { return None; } // global module fragment
    let name = rest.strip_suffix(';')?.trim();
    if name.is_empty() || name.contains(':') { return None; }
    Some(name.to_owned())
}

fn parse_import(line: &str) -> Option<String> {
    let rest = line.strip_prefix("import")?.trim_start();
    // Skip header units and partition imports.
    if rest.starts_with('<') || rest.starts_with('"') || rest.starts_with(':') { return None; }
    let name = rest.strip_suffix(';')?.trim();
    if name.is_empty() { return None; }
    Some(name.to_owned())
}

// ── DAG + topo sort ───────────────────────────────────────────────────────────

/// Organise scanned sources into a phased build plan.
/// Returns `Err(DependencyCycle)` if the MIU dependency graph has a cycle.
pub fn plan_module_build(
    project_dir: &Path,
    profile: &str,
    scanned: Vec<ScannedSource>,
) -> Result<ModuleBuildPlan, FreightError> {
    let (mius, rest): (Vec<_>, Vec<_>) = scanned
        .into_iter()
        .partition(|s| matches!(s.role, ModuleRole::Interface(_)));

    let n = mius.len();
    if n == 0 {
        return Ok(ModuleBuildPlan { miu_batches: vec![], rest, module_bmi_map: HashMap::new() });
    }

    // Map module name → local index within `mius`.
    let name_to_local: HashMap<String, usize> = mius.iter().enumerate()
        .map(|(i, s)| {
            let name = match &s.role {
                ModuleRole::Interface(n) => n.clone(),
                _ => unreachable!(),
            };
            (name, i)
        })
        .collect();

    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n]; // adj[provider] → Vec<consumer>

    for (i, miu) in mius.iter().enumerate() {
        for import_name in &miu.imports {
            if let Some(&provider) = name_to_local.get(import_name.as_str()) {
                adj[provider].push(i);
                in_degree[i] += 1;
            }
        }
    }

    // Kahn's algorithm — produces batches of MIUs ready to compile in parallel.
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut miu_batches: Vec<Vec<ScannedSource>> = Vec::new();
    let mut processed = 0usize;

    while !queue.is_empty() {
        let batch_size = queue.len();
        let mut batch = Vec::with_capacity(batch_size);

        for _ in 0..batch_size {
            let idx = queue.pop_front().unwrap();
            batch.push(mius[idx].clone());
            for &dep_idx in &adj[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    queue.push_back(dep_idx);
                }
            }
        }

        processed += batch.len();
        miu_batches.push(batch);
    }

    if processed < n {
        return Err(FreightError::DependencyCycle(
            "cycle detected in C++20 module dependency graph".into(),
        ));
    }

    // Pre-populate the BMI map paths (actual files don't exist until compile time).
    let module_bmi_map = name_to_local.keys()
        .map(|name| (name.clone(), bmi_path(project_dir, profile, name)))
        .collect();

    Ok(ModuleBuildPlan { miu_batches, rest, module_bmi_map })
}

// ── BMI path ─────────────────────────────────────────────────────────────────

/// Canonical path for a module's Binary Module Interface (BMI).
/// `target/{profile}/modules/{name}.pcm`
pub fn bmi_path(project_dir: &Path, profile: &str, module_name: &str) -> PathBuf {
    project_dir
        .join("target")
        .join(profile)
        .join("modules")
        .join(format!("{module_name}.pcm"))
}

// ── Module-aware compile pipeline ────────────────────────────────────────────

/// Compile all sources using the module build plan.
///
/// Phase 1 — MIU batches in topo order (each batch is compiled in parallel).
/// Phase 2 — MImplUs + regular TUs in parallel with appropriate `-fmodule-file=` flags.
pub fn compile_module_sources(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    plan: &mut ModuleBuildPlan,
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    feature_defines: &[String],
    header_unit_flags: &[String],
    progress: &Progress,
) -> Result<CompileResult, FreightError> {
    let mut all_objects: Vec<PathBuf> = Vec::new();
    let mut total_compiled = 0usize;
    let mut total_skipped = 0usize;
    let mut compiled_sources: Vec<PathBuf> = Vec::new();

    let progress = progress.clone();

    // Phase 1: MIU batches — sequential between batches, parallel within each batch.
    for batch in &plan.miu_batches {
        let bmi_snapshot = plan.module_bmi_map.clone();
        let progress = progress.clone();

        let results: Result<Vec<(PathBuf, bool)>, FreightError> = batch
            .par_iter()
            .map(|scanned| {
                compile_miu(project_dir, manifest, backend, profile, scanned, include_dirs, detected, &bmi_snapshot, feature_defines, header_unit_flags, &progress)
            })
            .collect();

        for (scanned, (obj, compiled)) in batch.iter().zip(results?) {
            all_objects.push(obj);
            if compiled {
                total_compiled += 1;
                compiled_sources.push(scanned.source.path.clone());
            } else {
                total_skipped += 1;
            }
        }
    }

    // Phase 2: everything else — compiled in parallel now that all BMIs exist.
    let bmi_map = &plan.module_bmi_map;
    let results: Result<Vec<(PathBuf, bool)>, FreightError> = plan.rest
        .par_iter()
        .map(|scanned| {
            let mflags = import_flags(&scanned.imports, bmi_map);
            compile_non_miu(project_dir, manifest, backend, profile, scanned, include_dirs, detected, &mflags, feature_defines, header_unit_flags, &progress)
        })
        .collect();

    for (scanned, (obj, compiled)) in plan.rest.iter().zip(results?) {
        all_objects.push(obj);
        if compiled {
            total_compiled += 1;
            compiled_sources.push(scanned.source.path.clone());
        } else {
            total_skipped += 1;
        }
    }

    Ok(CompileResult { objects: all_objects, compiled_sources, compiled: total_compiled, skipped: total_skipped })
}

// ── MIU compilation ───────────────────────────────────────────────────────────

fn compile_miu(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    scanned: &ScannedSource,
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    bmi_map: &HashMap<String, PathBuf>,
    feature_defines: &[String],
    header_unit_flags: &[String],
    progress: &Progress,
) -> Result<(PathBuf, bool), FreightError> {
    let src_abs = project_dir.join(&scanned.source.path);
    let obj = object_path(project_dir, profile, &scanned.source.path);
    let dep = dep_file_path(project_dir, profile, &scanned.source.path);
    let module_name = match &scanned.role {
        ModuleRole::Interface(n) => n,
        _ => unreachable!(),
    };
    let bmi = bmi_path(project_dir, profile, module_name);

    if is_up_to_date(&src_abs, &obj, &dep) && bmi.exists() {
        progress(BuildEvent::Fresh { path: scanned.source.path.clone() });
        return Ok((obj, false));
    }

    let compiler = select_compiler(&scanned.source.lang_key, backend, detected, None)
        .ok_or_else(|| FreightError::NoCompilerForLang(scanned.source.lang_key.clone()))?;
    let settings = settings_for_lang(manifest, profile, &scanned.source.lang_key, include_dirs, project_dir, feature_defines);
    let compile_bin = resolve_compile_binary(compiler, &scanned.source.lang_key);

    fs::create_dir_all(obj.parent().unwrap())?;
    fs::create_dir_all(bmi.parent().unwrap())?;

    let dep_import_flags = import_flags(&scanned.imports, bmi_map);

    progress(BuildEvent::Compiling { path: scanned.source.path.clone() });

    match &compiler.template.modules {
        ModuleStyle::Gcc { compile_miu: miu_flag_tmpl, .. } => {
            // Single pass: produces both the object file and the BMI.
            let bmi_flag = miu_flag_tmpl.replace("{pcm_path}", &bmi.to_string_lossy());
            let mut mflags = split_flags(&bmi_flag);
            mflags.extend_from_slice(&dep_import_flags);
            mflags.extend_from_slice(header_unit_flags);
            compile_one(&src_abs, &obj, &dep, &compile_bin, compiler, &settings, &mflags)?;
        }
        ModuleStyle::Clang { precompile: precompile_flag, import_module: import_tmpl, .. } => {
            // Step 1: --precompile → BMI (.pcm); no object produced.
            precompile_clang(
                &src_abs, &bmi, &compile_bin, compiler, &settings,
                &dep_import_flags, precompile_flag,
            )?;
            // Step 2: compile to .o — pass the module's own BMI so clang knows the mapping.
            let own_flag = import_tmpl
                .replace("{name}", module_name)
                .replace("{pcm_path}", &bmi.to_string_lossy());
            let mut mflags = split_flags(&own_flag);
            mflags.extend_from_slice(&dep_import_flags);
            mflags.extend_from_slice(header_unit_flags);
            compile_one(&src_abs, &obj, &dep, &compile_bin, compiler, &settings, &mflags)?;
        }
        ModuleStyle::Unsupported => {
            return Err(FreightError::NoCompilerForLang(format!(
                "{} does not support C++20 modules", compiler.template.name
            )));
        }
    }

    Ok((obj, true))
}

/// Run the Clang `--precompile` step to produce a BMI (.pcm) from a module interface file.
/// This step does not produce an object file.
fn precompile_clang(
    source: &Path,
    pcm_out: &Path,
    compile_bin: &Path,
    compiler: &DetectedCompiler,
    settings: &BuildSettings,
    import_flags_slice: &[String],
    precompile_flag: &str,
) -> Result<(), FreightError> {
    let mut cmd = Command::new(compile_bin);
    cmd.args(compiler.template.assemble_flags(settings));
    cmd.args(split_flags(precompile_flag));
    cmd.args(import_flags_slice);
    cmd.arg(source);
    cmd.args(compiler.template.output_flag(pcm_out));

    let out = cmd.output().map_err(FreightError::Io)?;
    if out.status.success() { return Ok(()); }
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let diag = if stdout.is_empty() { stderr } else { format!("{stdout}\n{stderr}") };
    Err(FreightError::CompileFailed(
        source.to_string_lossy().into_owned(),
        format_compiler_diagnostics(source, &diag),
    ))
}

// ── Non-MIU compilation (MImplUs + regular TUs) ───────────────────────────────

fn compile_non_miu(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    scanned: &ScannedSource,
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    module_flags_slice: &[String],
    feature_defines: &[String],
    header_unit_flags: &[String],
    progress: &Progress,
) -> Result<(PathBuf, bool), FreightError> {
    let src_abs = project_dir.join(&scanned.source.path);
    let obj = object_path(project_dir, profile, &scanned.source.path);
    let dep = dep_file_path(project_dir, profile, &scanned.source.path);

    if is_up_to_date(&src_abs, &obj, &dep) {
        progress(BuildEvent::Fresh { path: scanned.source.path.clone() });
        return Ok((obj, false));
    }

    let compiler = select_compiler(&scanned.source.lang_key, backend, detected, None)
        .ok_or_else(|| FreightError::NoCompilerForLang(scanned.source.lang_key.clone()))?;
    let settings = settings_for_lang(manifest, profile, &scanned.source.lang_key, include_dirs, project_dir, feature_defines);
    let compile_bin = resolve_compile_binary(compiler, &scanned.source.lang_key);

    let mut all_flags: Vec<String> = module_flags_slice.to_vec();
    all_flags.extend_from_slice(header_unit_flags);

    fs::create_dir_all(obj.parent().unwrap())?;
    progress(BuildEvent::Compiling { path: scanned.source.path.clone() });
    compile_one(&src_abs, &obj, &dep, &compile_bin, compiler, &settings, &all_flags)?;
    Ok((obj, true))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build `-fmodule-file=<name>=<path>` flags for every import that has a known BMI.
fn import_flags(imports: &[String], bmi_map: &HashMap<String, PathBuf>) -> Vec<String> {
    imports.iter()
        .filter_map(|name| {
            bmi_map.get(name).map(|bmi| format!("-fmodule-file={}={}", name, bmi.display()))
        })
        .collect()
}

/// Split a space-separated flag string into individual tokens.
fn split_flags(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn src(path: &str) -> SourceFile {
        SourceFile { path: PathBuf::from(path), lang_key: "cpp".into() }
    }

    fn write(dir: &Path, path: &str, content: &str) {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    // ── Scanner ───────────────────────────────────────────────────────────────

    #[test]
    fn detects_module_interface() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/math.cppm", "export module math;\n\nint add(int a, int b) { return a + b; }\n");
        let scanned = scan_file(dir.path(), &src("src/math.cppm"));
        assert_eq!(scanned.role, ModuleRole::Interface("math".into()));
        assert!(scanned.imports.is_empty());
    }

    #[test]
    fn detects_module_implementation() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/math_impl.cpp", "module math;\n\nvoid helper() {}\n");
        let scanned = scan_file(dir.path(), &src("src/math_impl.cpp"));
        assert_eq!(scanned.role, ModuleRole::Implementation("math".into()));
    }

    #[test]
    fn detects_imports() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/main.cpp",
            "import math;\nimport geometry;\n\nint main() {}\n");
        let scanned = scan_file(dir.path(), &src("src/main.cpp"));
        assert_eq!(scanned.role, ModuleRole::Regular);
        assert!(scanned.imports.contains(&"math".to_string()));
        assert!(scanned.imports.contains(&"geometry".to_string()));
    }

    #[test]
    fn skips_header_unit_imports() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/main.cpp",
            "import <vector>;\nimport \"myheader.hpp\";\nimport mymodule;\n\nint main(){}\n");
        let scanned = scan_file(dir.path(), &src("src/main.cpp"));
        assert_eq!(scanned.imports, vec!["mymodule".to_string()]);
    }

    #[test]
    fn global_module_fragment_does_not_confuse_scanner() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/m.cppm",
            "module;\n#include <cstdio>\nexport module mylib;\n");
        let scanned = scan_file(dir.path(), &src("src/m.cppm"));
        assert_eq!(scanned.role, ModuleRole::Interface("mylib".into()));
    }

    #[test]
    fn regular_cpp_file_is_regular() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/util.cpp", "#include <string>\nvoid f() {}\n");
        let scanned = scan_file(dir.path(), &src("src/util.cpp"));
        assert_eq!(scanned.role, ModuleRole::Regular);
        assert!(scanned.imports.is_empty());
    }

    #[test]
    fn non_cpp_lang_key_is_always_regular() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/main.c", "export module foo;\n"); // nonsensical but should not matter
        let c_src = SourceFile { path: PathBuf::from("src/main.c"), lang_key: "c".into() };
        let scanned = scan_sources(dir.path(), &[c_src]);
        assert_eq!(scanned[0].role, ModuleRole::Regular);
    }

    // ── DAG / plan ────────────────────────────────────────────────────────────

    fn make_miu(name: &str, imports: &[&str]) -> ScannedSource {
        ScannedSource {
            source: SourceFile { path: PathBuf::from(format!("src/{name}.cppm")), lang_key: "cpp".into() },
            role: ModuleRole::Interface(name.to_owned()),
            imports: imports.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_regular(path: &str, imports: &[&str]) -> ScannedSource {
        ScannedSource {
            source: SourceFile { path: PathBuf::from(path), lang_key: "cpp".into() },
            role: ModuleRole::Regular,
            imports: imports.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn no_modules_gives_empty_batches() {
        let sources = vec![make_regular("src/main.cpp", &[])];
        let plan = plan_module_build(Path::new("/proj"), "dev", sources).unwrap();
        assert!(plan.miu_batches.is_empty());
        assert_eq!(plan.rest.len(), 1);
    }

    #[test]
    fn single_miu_produces_one_batch() {
        let sources = vec![make_miu("math", &[]), make_regular("src/main.cpp", &["math"])];
        let plan = plan_module_build(Path::new("/proj"), "dev", sources).unwrap();
        assert_eq!(plan.miu_batches.len(), 1);
        assert_eq!(plan.miu_batches[0].len(), 1);
        assert_eq!(plan.rest.len(), 1);
    }

    #[test]
    fn independent_mius_are_in_same_batch() {
        let sources = vec![
            make_miu("math", &[]),
            make_miu("geometry", &[]),
            make_regular("src/main.cpp", &["math", "geometry"]),
        ];
        let plan = plan_module_build(Path::new("/proj"), "dev", sources).unwrap();
        assert_eq!(plan.miu_batches.len(), 1);
        assert_eq!(plan.miu_batches[0].len(), 2);
    }

    #[test]
    fn chained_mius_produce_ordered_batches() {
        // geometry imports math → math must compile first.
        let sources = vec![
            make_miu("math", &[]),
            make_miu("geometry", &["math"]),
            make_regular("src/main.cpp", &["geometry"]),
        ];
        let plan = plan_module_build(Path::new("/proj"), "dev", sources).unwrap();
        assert_eq!(plan.miu_batches.len(), 2);
        let first_names: Vec<&str> = plan.miu_batches[0].iter()
            .map(|s| match &s.role { ModuleRole::Interface(n) => n.as_str(), _ => "" })
            .collect();
        assert_eq!(first_names, vec!["math"]);
    }

    #[test]
    fn cycle_returns_error() {
        let sources = vec![
            make_miu("a", &["b"]),
            make_miu("b", &["a"]),
        ];
        assert!(plan_module_build(Path::new("/proj"), "dev", sources).is_err());
    }

    #[test]
    fn bmi_path_has_correct_structure() {
        let p = bmi_path(Path::new("/project"), "dev", "math");
        assert_eq!(p, PathBuf::from("/project/target/dev/modules/math.pcm"));
    }

    #[test]
    fn import_flags_produces_module_file_flags() {
        let mut map = HashMap::new();
        map.insert("math".to_string(), PathBuf::from("/proj/target/dev/modules/math.pcm"));
        let flags = import_flags(&["math".to_string(), "unknown".to_string()], &map);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0], "-fmodule-file=math=/proj/target/dev/modules/math.pcm");
    }
}
