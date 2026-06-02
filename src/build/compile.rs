use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime};

use rayon::prelude::*;

use super::diagnostics::format_compiler_diagnostics;
use super::discover::SourceFile;
use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Backend, Manifest};
use crate::toolchain::template::BuildSettings;
use crate::toolchain::DetectedCompiler;

// ── Compiler cache wrapper (ccache / sccache) ─────────────────────────────────

static CACHE_WRAPPER: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Return the compiler cache wrapper binary (`sccache` > `ccache`), or `None`
/// when none is found or `FREIGHT_NO_CACHE=1` is set.
fn cache_wrapper() -> Option<&'static PathBuf> {
    CACHE_WRAPPER
        .get_or_init(|| {
            if std::env::var_os("FREIGHT_NO_CACHE").is_some() {
                return None;
            }
            for name in &["sccache", "ccache"] {
                if let Some(path) = which_tool(name) {
                    return Some(path);
                }
            }
            None
        })
        .as_ref()
}

fn which_tool(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Languages for which unity builds are supported (sources combined via `#include`).
pub const UNITY_SUPPORTED_LANGS: &[&str] = &["c", "cpp", "cuda", "hip", "opencl"];

fn unity_ext_for_lang(lang_key: &str) -> &'static str {
    match lang_key {
        "cpp" | "cuda" => "cpp",
        "hip" => "hip",
        "opencl" => "cl",
        _ => "c",
    }
}

pub struct CompileResult {
    /// All object files that exist after this call (compiled or already up-to-date).
    pub objects: Vec<PathBuf>,
    /// Source files that were actually recompiled during this invocation.
    pub compiled_sources: Vec<PathBuf>,
    pub compiled: usize,
    pub skipped: usize,
}

/// Compile every source file to a `.o` object under `target/{profile}/objs/`.
///
/// Files whose object is newer than both the source and all headers listed in its
/// `.d` dependency file are skipped. Compilation runs in parallel via rayon.
pub fn compile_sources(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    feature_defines: &[String],
    header_unit_flags: &[String],
    progress: &Progress,
) -> Result<CompileResult, FreightError> {
    let pf = primary_family(backend, detected);
    let lf = linker_family(manifest, backend, detected);
    let progress = progress.clone();

    let results: Result<Vec<(PathBuf, bool)>, FreightError> = sources
        .par_iter()
        .map(|src| -> Result<(PathBuf, bool), FreightError> {
            let src_abs = project_dir.join(&src.path);
            let obj = object_path(project_dir, profile, &src.path);
            let dep = dep_file_path(project_dir, profile, &src.path);

            if is_up_to_date(&src_abs, &obj, &dep) {
                progress(BuildEvent::Fresh {
                    path: src.path.clone(),
                });
                return Ok((obj, false));
            }

            let compiler = select_compiler(&src.lang_key, backend, detected, pf)
                .ok_or_else(|| FreightError::NoCompilerForLang(src.lang_key.clone()))?;

            let mut settings = settings_for_lang(
                manifest,
                profile,
                &src.lang_key,
                include_dirs,
                project_dir,
                feature_defines,
            );

            // Suppress LTO when this compiler's family differs from the linker's family.
            // Mixing GNU (gfortran/gcc) GIMPLE LTO IR with LLVM (clang++) LTO bitcode is
            // incompatible. The object still gets -O3; only cross-TU LTO is lost.
            if settings.lto {
                let cf = compiler.template.family.as_str();
                if let Some(linker_f) = lf {
                    if !cf.is_empty() && cf != linker_f {
                        settings.lto = false;
                    }
                }
            }

            // Whole-program builders (e.g. gnatmake for Ada) handle compile+bind+link
            // in a single invocation during the link step. Skip the separate compile
            // step and record the source path itself so link_executable receives it.
            if compiler
                .template
                .linking
                .get(src.lang_key.as_str())
                .map_or(false, |l| l.whole_program)
            {
                return Ok((src_abs, true));
            }

            let compile_bin = resolve_compile_binary(compiler, &src.lang_key);

            fs::create_dir_all(obj.parent().expect("obj path always has a parent"))?;

            progress(BuildEvent::Compiling {
                path: src.path.clone(),
            });
            let t0 = Instant::now();
            compile_one(
                &src_abs,
                &obj,
                &dep,
                &compile_bin,
                compiler,
                &settings,
                header_unit_flags,
            )?;
            if std::env::var_os("FREIGHT_TIME_PASSES").is_some() {
                progress(BuildEvent::Timing {
                    path: src.path.clone(),
                    ns: t0.elapsed().as_nanos() as u64,
                });
            }

            Ok((obj, true))
        })
        .collect();

    let pairs = results?;
    let objects = pairs.iter().map(|(o, _)| o.clone()).collect();
    let compiled_sources = sources
        .iter()
        .zip(pairs.iter())
        .filter_map(|(src, (_, compiled))| compiled.then(|| src.path.clone()))
        .collect();
    let compiled = pairs.iter().filter(|(_, c)| *c).count();
    let skipped = pairs.iter().filter(|(_, c)| !*c).count();

    Ok(CompileResult {
        objects,
        compiled_sources,
        compiled,
        skipped,
    })
}

/// Unity (jumbo) build: merge all sources of the same language into one TU via `#include`.
///
/// Languages in [`UNITY_SUPPORTED_LANGS`] are grouped and compiled as a single file each.
/// Other languages (Fortran, assembly, ISPC, …) fall back to individual compilation.
/// C++20 modules should not use this path — the caller must skip unity when modules are present.
pub fn compile_sources_unity(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    feature_defines: &[String],
    header_unit_flags: &[String],
    progress: &Progress,
) -> Result<CompileResult, FreightError> {
    use std::collections::HashMap;
    use std::fmt::Write as FmtWrite;

    let pf = primary_family(backend, detected);
    let progress_clone = progress.clone();

    // Split sources: unifiable langs vs. compile-individually langs.
    let (unity_srcs, regular_srcs): (Vec<SourceFile>, Vec<SourceFile>) = sources
        .iter()
        .cloned()
        .partition(|s| UNITY_SUPPORTED_LANGS.contains(&s.lang_key.as_str()));

    let reg_result = if !regular_srcs.is_empty() {
        compile_sources(
            project_dir,
            manifest,
            backend,
            profile,
            &regular_srcs,
            include_dirs,
            detected,
            feature_defines,
            header_unit_flags,
            progress,
        )?
    } else {
        CompileResult {
            objects: vec![],
            compiled_sources: vec![],
            compiled: 0,
            skipped: 0,
        }
    };

    if unity_srcs.is_empty() {
        return Ok(reg_result);
    }

    let unity_dir = project_dir.join("target").join(profile).join("unity");
    fs::create_dir_all(&unity_dir)?;

    let mut all_objects = reg_result.objects;
    let mut total_compiled = reg_result.compiled;
    let mut total_skipped = reg_result.skipped;

    // Group by language key.
    let mut groups: HashMap<String, Vec<SourceFile>> = HashMap::new();
    for src in unity_srcs {
        groups.entry(src.lang_key.clone()).or_default().push(src);
    }

    for (lang_key, lang_sources) in &groups {
        let ext = unity_ext_for_lang(lang_key);
        let unity_src = unity_dir.join(format!("{lang_key}_unity.{ext}"));
        let obj = unity_dir.join(format!("{lang_key}_unity.o"));
        let dep = unity_dir.join(format!("{lang_key}_unity.d"));

        // Regenerate the unity file if it's absent or any source is newer.
        let unity_mtime = mtime(&unity_src);
        let needs_regen = unity_mtime.is_none()
            || lang_sources.iter().any(|s| {
                mtime(&project_dir.join(&s.path))
                    .map_or(true, |sm| unity_mtime.map_or(true, |um| sm >= um))
            });
        if needs_regen {
            let mut content = String::new();
            for src in lang_sources {
                let _ = writeln!(
                    content,
                    "#include \"{}\"",
                    project_dir.join(&src.path).display()
                );
            }
            fs::write(&unity_src, &content)?;
        }

        // Up-to-date: object must exist and be newer than every constituent source.
        let obj_mtime = mtime(&obj);
        let up_to_date = obj_mtime.is_some()
            && !lang_sources.iter().any(|s| {
                mtime(&project_dir.join(&s.path))
                    .map_or(true, |sm| obj_mtime.map_or(true, |om| sm >= om))
            })
            && is_up_to_date(&unity_src, &obj, &dep);

        if up_to_date {
            for src in lang_sources {
                progress_clone(BuildEvent::Fresh {
                    path: src.path.clone(),
                });
            }
            total_skipped += lang_sources.len();
            all_objects.push(obj);
            continue;
        }

        let compiler = select_compiler(lang_key, backend, detected, pf)
            .ok_or_else(|| FreightError::NoCompilerForLang(lang_key.clone()))?;

        let settings = settings_for_lang(
            manifest,
            profile,
            lang_key,
            include_dirs,
            project_dir,
            feature_defines,
        );
        let compile_bin = resolve_compile_binary(compiler, lang_key);

        for src in lang_sources {
            progress_clone(BuildEvent::Compiling {
                path: src.path.clone(),
            });
        }
        compile_one(
            &unity_src,
            &obj,
            &dep,
            &compile_bin,
            compiler,
            &settings,
            header_unit_flags,
        )?;

        total_compiled += lang_sources.len();
        all_objects.push(obj);
    }

    Ok(CompileResult {
        objects: all_objects,
        compiled_sources: vec![],
        compiled: total_compiled,
        skipped: total_skipped,
    })
}

// ── Compiler selection ────────────────────────────────────────────────────────

/// Pick the compiler to use for `lang_key` given the backend preference.
///
/// `backend = "auto"` → first detected compiler whose template declares `lang_key`.
///   When `preferred_family` is `Some(f)` and non-empty, prefer a compiler in that
///   family; fall back to any compiler that handles `lang_key` when no family match
///   is found.
/// Any other name → treated as a family name first: find the compiler in that family
///   that handles `lang_key`. Falls back to an exact template-name match for standalone
///   compilers (e.g. `"nasm"`, `"msvc"`) that have no family.
pub fn select_compiler<'a>(
    lang_key: &str,
    backend: &Backend,
    detected: &'a [DetectedCompiler],
    preferred_family: Option<&str>,
) -> Option<&'a DetectedCompiler> {
    if backend.is_auto() {
        if let Some(family) = preferred_family.filter(|f| !f.is_empty()) {
            if let Some(c) = detected
                .iter()
                .find(|d| d.template.linking.contains_key(lang_key) && d.template.family == family)
            {
                return Some(c);
            }
        }
        detected
            .iter()
            .find(|d| d.template.linking.contains_key(lang_key))
    } else {
        let name = backend.name();
        // 1. Family member that directly handles this lang_key.
        if let Some(c) = detected.iter().find(|d| {
            !d.template.family.is_empty()
                && d.template.family == name
                && d.template.linking.contains_key(lang_key)
        }) {
            return Some(c);
        }

        // 2. Guest compiler (requires_toolchain non-empty) that handles lang_key
        //    and whose host requirements are all provided by the named family.
        let family_langs: std::collections::HashSet<&str> = detected
            .iter()
            .filter(|d| !d.template.family.is_empty() && d.template.family == name)
            .flat_map(|d| d.template.linking.keys().map(String::as_str))
            .collect();

        if !family_langs.is_empty() {
            if let Some(c) = detected.iter().find(|d| {
                !d.template.requires_toolchain.is_empty()
                    && d.template.linking.contains_key(lang_key)
                    && d.template
                        .requires_toolchain
                        .iter()
                        .all(|r| family_langs.contains(r.as_str()))
            }) {
                return Some(c);
            }
        }

        // 3. Exact template-name match (standalone compilers with no family)
        //    that also handles this lang_key.
        if let Some(c) = detected
            .iter()
            .find(|d| d.template.name == name && d.template.linking.contains_key(lang_key))
        {
            return Some(c);
        }

        // 4. Fallback: backend is recognised but doesn't compile this language.
        //    (e.g. backend="zig" for .cpp files → fall through to g++/clang++).
        //    Unknown backends (no detected compiler at all) return None so the
        //    caller can emit a proper "backend not found" error.
        let backend_exists = detected
            .iter()
            .any(|d| d.template.family == name || d.template.name == name);
        if backend_exists {
            return detected
                .iter()
                .find(|d| d.template.linking.contains_key(lang_key));
        }

        None
    }
}

/// Return the compiler family for the project's primary language (cpp → c → fortran → any),
/// used to bias secondary-language selection toward the same toolchain family.
pub fn primary_family<'a>(backend: &Backend, detected: &'a [DetectedCompiler]) -> Option<&'a str> {
    for lang in &["cpp", "c", "fortran"] {
        if let Some(c) = select_compiler(lang, backend, detected, None) {
            if !c.template.family.is_empty() {
                return Some(&c.template.family);
            }
        }
    }
    None
}

/// Return the family of the compiler that will drive the final link step.
///
/// Mirrors `link::select_linker`'s priority ordering (including backend preference)
/// but operates only on `detected` (no `templates` slice) for use inside `compile_sources`.
fn linker_family<'a>(
    manifest: &Manifest,
    backend: &Backend,
    detected: &'a [DetectedCompiler],
) -> Option<&'a str> {
    const PRIORITY: &[&str] = &[
        "cpp", "objcpp", "cuda", "hip", "sycl", "objc", "c", "fortran", "ada", "d", "opencl",
        "ispc",
    ];
    // Non-auto backend: prefer linkers from the requested family (same as select_linker).
    if !backend.is_auto() {
        let family = backend.name();
        for &lang in PRIORITY {
            if super::has_lang(manifest, lang, detected) {
                if let Some(d) = detected
                    .iter()
                    .find(|d| d.template.linking.contains_key(lang) && d.template.family == family)
                {
                    return Some(&d.template.family);
                }
            }
        }
    }
    for &lang in PRIORITY {
        if super::has_lang(manifest, lang, detected) {
            if let Some(d) = detected
                .iter()
                .find(|d| d.template.linking.contains_key(lang))
            {
                if !d.template.family.is_empty() {
                    return Some(&d.template.family);
                }
            }
        }
    }
    detected
        .first()
        .and_then(|d| (!d.template.family.is_empty()).then_some(d.template.family.as_str()))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Produce build settings for a specific lang_key, merging in discovered include dirs
/// and any active feature defines.
pub fn settings_for_lang(
    manifest: &Manifest,
    profile: &str,
    lang_key: &str,
    extra_include_dirs: &[PathBuf],
    project_dir: &Path,
    feature_defines: &[String],
) -> BuildSettings {
    let mut s = manifest.build_settings_for(profile);
    let lang = manifest.effective_language_settings(lang_key);
    if s.standard.is_none() {
        s.standard = lang.std;
    }
    if let Some(stdlib) = lang.stdlib {
        s.stdlib = stdlib;
    }
    for dir in extra_include_dirs {
        s.include_paths.push(project_dir.join(dir));
    }
    s.defines.extend_from_slice(feature_defines);
    // Flags injected by language_option handlers at build startup.
    s.extra_flags.extend_from_slice(&lang.injected_flags);
    s
}

/// `src/core/engine.cpp` → `{project}/target/{profile}/objs/src/core/engine.o`
pub fn object_path(project_dir: &Path, profile: &str, source_rel: &Path) -> PathBuf {
    let mut p = project_dir
        .join("target")
        .join(profile)
        .join("objs")
        .join(source_rel);
    p.set_extension("o");
    p
}

/// Same as `object_path` but with `.d` extension for the Makefile dependency file.
pub fn dep_file_path(project_dir: &Path, profile: &str, source_rel: &Path) -> PathBuf {
    let mut p = project_dir
        .join("target")
        .join(profile)
        .join("objs")
        .join(source_rel);
    p.set_extension("d");
    p
}

/// Return `true` if the object is newer than the source and all its included headers.
pub(crate) fn is_up_to_date(source: &Path, object: &Path, dep_file: &Path) -> bool {
    let obj_mtime = match mtime(object) {
        Some(t) => t,
        None => return false,
    };
    if mtime(source).map_or(true, |s| s >= obj_mtime) {
        return false;
    }
    // Check every header listed in the .d file.
    if dep_file.exists() {
        if let Ok(contents) = fs::read_to_string(dep_file) {
            for dep_path in parse_dep_file(&contents) {
                if mtime(Path::new(&dep_path)).map_or(false, |h| h >= obj_mtime) {
                    return false;
                }
            }
        }
    }
    true
}

/// Parse a Makefile dependency file and return all listed prerequisites.
///
/// Format: `target.o: src.cpp inc/foo.hpp \\\n  /usr/include/bar.h`
/// We skip the first token (the target itself) and return everything after the `:`.
fn parse_dep_file(contents: &str) -> Vec<String> {
    let Some(colon) = contents.find(':') else {
        return vec![];
    };
    contents[colon + 1..]
        .replace("\\\n", " ")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    path.metadata().ok()?.modified().ok()
}

/// Return the binary path to use when *compiling* (not linking) a source file.
///
/// If the language's linking section declares a `compile_binary`, we look for that
/// binary in the same directory as the detected compiler (e.g. `gcc` next to `g++`),
/// falling back to a bare name resolved via PATH. This lets `gcc.toml` use `g++` for
/// linking while still invoking `gcc` for `.c` source files.
pub(crate) fn resolve_compile_binary(compiler: &DetectedCompiler, lang_key: &str) -> PathBuf {
    if let Some(cb) = compiler
        .template
        .linking
        .get(lang_key)
        .and_then(|l| l.compile_binary.as_deref())
    {
        if let Some(parent) = compiler.path.parent() {
            let candidate = parent.join(cb);
            if candidate.exists() {
                return candidate;
            }
        }
        PathBuf::from(cb)
    } else {
        compiler.path.clone()
    }
}

/// Invoke the compiler for a single source file, generating a dep file alongside.
///
/// `module_flags` are injected after the assembled flags and before `-c` — used by the
/// module pipeline to pass `-fmodule-output=` and `-fmodule-file=` on a per-source basis.
pub(crate) fn compile_one(
    source_abs: &Path,
    obj_path: &Path,
    dep_path: &Path,
    compile_bin: &Path,
    compiler: &DetectedCompiler,
    settings: &BuildSettings,
    module_flags: &[String],
) -> Result<(), FreightError> {
    // Reject unsupported standards before invoking the compiler.
    let effective_std = settings
        .standard
        .as_deref()
        .or_else(|| compiler.template.defaults.get("std").map(String::as_str));
    if let Some(std) = effective_std {
        if let Some(msg) = compiler
            .template
            .check_standard_floor(std, &compiler.version)
        {
            return Err(FreightError::OptionError(msg));
        }
    }

    let dep_mode = compiler.template.dep_file_mode();

    let mut cmd = if let Some(wrapper) = cache_wrapper() {
        let mut c = Command::new(wrapper);
        c.arg(compile_bin);
        if let Some(sub) = compiler.template.subcommand.as_deref() {
            c.arg(sub);
        }
        c
    } else {
        let mut c = Command::new(compile_bin);
        if let Some(sub) = compiler.template.subcommand.as_deref() {
            c.arg(sub);
        }
        c
    };
    cmd.args(compiler.template.assemble_flags(settings));
    cmd.args(module_flags);
    cmd.args(compiler.template.compile_only_flag());
    // "file": -MMD -MF <path>; "stdout": compiler flag like /showIncludes; "none": nothing
    if dep_mode != "none" {
        cmd.args(compiler.template.dep_file_flags(dep_path));
    }
    cmd.arg(source_abs);
    cmd.args(compiler.template.output_flag(obj_path));

    if std::env::var_os("FREIGHT_VERBOSE").is_some() {
        print_cmd(&cmd);
    }
    let out = cmd.output().map_err(FreightError::Io)?;

    // For stdout dep mode: parse include lines from stdout before checking success
    if dep_mode == "stdout" && out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Err(e) = write_stdout_dep_file(dep_path, source_abs, &stdout) {
            eprintln!(
                "warning: could not write dep file {}: {e}",
                dep_path.display()
            );
        }
    }

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        // For stdout dep mode, stdout contains /showIncludes output, not error info
        let diag = if dep_mode == "stdout" {
            stderr
        } else {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            if stdout.is_empty() {
                stderr
            } else {
                format!("{stdout}\n{stderr}")
            }
        };
        return Err(FreightError::CompileFailed(
            source_abs.to_string_lossy().into_owned(),
            format_compiler_diagnostics(source_abs, &diag),
        ));
    }

    // Always forward compiler warnings/notes to stderr so the user sees them.
    // Compiler diagnostics on a successful exit are warnings — always relevant.
    // The full compiler command is only printed in --verbose mode (above).
    if !out.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }

    Ok(())
}

/// Parse MSVC `/showIncludes` stdout and write a synthetic Makefile dep file.
///
/// MSVC prints `Note: including file:  <path>` for every directly or transitively
/// included header. We collect these paths and write them in `.d` format so the
/// existing mtime dirty-check logic can use them unchanged.
fn write_stdout_dep_file(dep_path: &Path, source: &Path, stdout: &str) -> std::io::Result<()> {
    const PREFIX: &str = "Note: including file:";
    let includes: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let t = line.trim_start();
            if t.starts_with(PREFIX) {
                Some(t[PREFIX.len()..].trim())
            } else {
                None
            }
        })
        .filter(|p| !p.is_empty())
        .collect();

    if includes.is_empty() {
        return Ok(());
    }

    if let Some(parent) = dep_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Makefile dep format: `obj.o: source.cpp header1.h \\\n  header2.h\n`
    let obj = dep_path.with_extension("o");
    let mut content = format!("{}: {}", obj.display(), source.display());
    for inc in &includes {
        content.push_str(&format!(" \\\n  {inc}"));
    }
    content.push('\n');
    fs::write(dep_path, content)
}

// ── Debug command printer (kept for FREIGHT_VERBOSE) ─────────────────────────

pub(crate) fn print_cmd(cmd: &Command) {
    use owo_colors::OwoColorize;
    let prog = cmd.get_program().to_string_lossy();
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    eprintln!("     {} {} {}", "cmd".dimmed(), prog, args.join(" "));
}

// ── Assembly emission ─────────────────────────────────────────────────────────

/// Languages for which assembly emission (`-S`) is not meaningful or supported.
/// Pure assemblers (gas/nasm/yasm) already work with textual assembly source.
const ASM_EMIT_SKIP_LANGS: &[&str] = &["gas", "nasm", "yasm", "ispc"];

/// Emit textual assembly for every source in `sources` into `target/{profile}/asm/`.
///
/// Runs in parallel alongside the normal build. Uses the same compiler flags as
/// compilation, replacing `-c` with `-S`. Non-fatal: failures are surfaced as
/// [`BuildEvent::Warning`] rather than aborting the build.
pub fn emit_asm_sources(
    project_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
    feature_defines: &[String],
    progress: &Progress,
) -> Result<(), FreightError> {
    let pf = primary_family(backend, detected);
    let asm_dir = project_dir.join("target").join(profile).join("asm");
    fs::create_dir_all(&asm_dir)?;

    let eligible: Vec<&SourceFile> = sources
        .iter()
        .filter(|s| !ASM_EMIT_SKIP_LANGS.contains(&s.lang_key.as_str()))
        .collect();

    eligible.par_iter().for_each(|src| {
        let src_abs = project_dir.join(&src.path);

        // Preserve directory structure to avoid name collisions between files
        // with the same name in different subdirectories.
        let asm_rel = {
            let mut p = src.path.clone();
            p.set_extension("s");
            p
        };
        let asm_path = asm_dir.join(&asm_rel);

        if let Some(parent) = asm_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                progress(BuildEvent::Warning(format!(
                    "emit-asm: cannot create dir: {e}"
                )));
                return;
            }
        }

        let compiler = match select_compiler(&src.lang_key, backend, detected, pf) {
            Some(c) => c,
            None => return, // no compiler for this lang — skip silently
        };

        let settings = settings_for_lang(
            manifest,
            profile,
            &src.lang_key,
            include_dirs,
            project_dir,
            feature_defines,
        );
        let compile_bin = resolve_compile_binary(compiler, &src.lang_key);

        let mut cmd = if let Some(wrapper) = cache_wrapper() {
            let mut c = Command::new(wrapper);
            c.arg(&compile_bin);
            if let Some(sub) = compiler.template.subcommand.as_deref() {
                c.arg(sub);
            }
            c
        } else {
            let mut c = Command::new(&compile_bin);
            if let Some(sub) = compiler.template.subcommand.as_deref() {
                c.arg(sub);
            }
            c
        };
        cmd.args(compiler.template.assemble_flags(&settings));
        cmd.arg("-S");
        cmd.arg(&src_abs);
        cmd.arg("-o");
        cmd.arg(&asm_path);

        if std::env::var_os("FREIGHT_VERBOSE").is_some() {
            print_cmd(&cmd);
        }

        match cmd.output() {
            Ok(out) if out.status.success() => {
                progress(BuildEvent::EmittedAsm { path: asm_path });
            }
            Ok(out) => {
                let msg = String::from_utf8_lossy(&out.stderr).into_owned();
                progress(BuildEvent::Warning(format!(
                    "emit-asm failed for {}: {}",
                    src.path.display(),
                    msg.lines().next().unwrap_or("unknown error")
                )));
            }
            Err(e) => {
                progress(BuildEvent::Warning(format!("emit-asm: {e}")));
            }
        }
    });

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::CompilerTemplate;

    fn templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_all_templates()
    }

    fn fake_detected(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
        templates
            .iter()
            .map(|t| DetectedCompiler {
                template: t.clone(),
                version: "0.0.0".into(),
                path: PathBuf::from(format!("/usr/bin/{}", t.binary)),
                cpu_extensions: vec![],
            })
            .collect()
    }

    // ── select_compiler ───────────────────────────────────────────────────────

    #[test]
    fn auto_backend_picks_first_with_lang_key() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend::default();
        let found = select_compiler("cpp", &backend, &detected, None);
        assert!(found.is_some(), "should find a C++ compiler");
        assert!(found.unwrap().template.linking.contains_key("cpp"));
    }

    #[test]
    fn named_backend_matches_template_name() {
        let ts = templates();
        let detected = fake_detected(&ts);
        // "gnu" family → g++ for cpp language
        let backend = Backend("gnu".into());
        let found = select_compiler("cpp", &backend, &detected, None);
        assert!(found.is_some());
        assert_eq!(found.unwrap().template.name, "g++");
    }

    #[test]
    fn named_backend_family_picks_right_compiler_per_lang() {
        let ts = templates();
        let detected = fake_detected(&ts);
        // "gnu" family → g++ for cpp, gcc for c, gfortran for fortran
        let cpp = select_compiler("cpp", &Backend("gnu".into()), &detected, None);
        assert!(cpp.is_some(), "gnu backend should find a C++ compiler");
        assert_eq!(cpp.unwrap().template.name, "g++");

        let fortran = select_compiler("fortran", &Backend("gnu".into()), &detected, None);
        assert!(
            fortran.is_some(),
            "gnu backend should find a Fortran compiler"
        );
        assert_eq!(fortran.unwrap().template.name, "gfortran");

        // "llvm" family → clang++ for cpp
        let cpp_llvm = select_compiler("cpp", &Backend("llvm".into()), &detected, None);
        assert!(
            cpp_llvm.is_some(),
            "llvm backend should find a C++ compiler"
        );
        assert_eq!(cpp_llvm.unwrap().template.name, "clang++");
    }

    #[test]
    fn named_backend_family_picks_guest_for_extension_lang() {
        let ts = templates();
        let detected = fake_detected(&ts);
        // "gnu" family has no direct cuda member, but nvcc is a guest that requires cpp.
        // gnu provides cpp, so nvcc should be returned for cuda files.
        let cuda = select_compiler("cuda", &Backend("gnu".into()), &detected, None);
        assert!(
            cuda.is_some(),
            "gnu backend should pick nvcc for cuda files"
        );
        assert_eq!(cuda.unwrap().template.name, "nvcc");

        // Same for llvm
        let cuda_llvm = select_compiler("cuda", &Backend("llvm".into()), &detected, None);
        assert!(
            cuda_llvm.is_some(),
            "llvm backend should also pick nvcc for cuda files"
        );
        assert_eq!(cuda_llvm.unwrap().template.name, "nvcc");
    }

    #[test]
    fn named_backend_asm_guest_selected_for_any_family() {
        let ts = templates();
        let detected = fake_detected(&ts);
        // An asm guest (e.g. nasm, yasm — requires_toolchain = ["c"]) should be picked
        // whenever the active family satisfies the "c" requirement.
        for backend_name in &["gnu", "llvm"] {
            let found = select_compiler("asm", &Backend(backend_name.to_string()), &detected, None);
            assert!(
                found.is_some(),
                "{backend_name} backend should find an asm guest compiler"
            );
            let compiler = found.unwrap();
            assert!(
                compiler.template.linking.contains_key("asm"),
                "selected compiler must handle 'asm'"
            );
            assert!(
                compiler
                    .template
                    .requires_toolchain
                    .contains(&"c".to_string()),
                "selected compiler must require 'c' toolchain"
            );
        }
    }

    #[test]
    fn named_backend_family_no_match_for_unsupported_lang() {
        let ts = templates();
        let detected = fake_detected(&ts);
        // No family handles an entirely unknown language key.
        let found = select_compiler("haskell", &Backend("gnu".into()), &detected, None);
        assert!(
            found.is_none(),
            "gnu backend should not find a compiler for 'haskell'"
        );
    }

    #[test]
    fn unknown_backend_returns_none() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend("nonexistent".into());
        assert!(select_compiler("cpp", &backend, &detected, None).is_none());
    }

    #[test]
    fn zig_backend_uses_zig_cxx_for_cpp_files() {
        // backend="zig" must compile .cpp with zig c++ (same family),
        // not fall through to g++/clang++ from step 4.
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend("zig".into());
        let found = select_compiler("cpp", &backend, &detected, None);
        assert!(
            found.is_some(),
            "should find a C++ compiler for zig backend"
        );
        assert_eq!(
            found.unwrap().template.name,
            "zig-c++",
            "zig backend must compile C++ with zig c++, not g++/clang++"
        );
    }

    #[test]
    fn auto_backend_for_cuda_picks_nvcc() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend::default();
        let found = select_compiler("cuda", &backend, &detected, None);
        assert!(found.is_some(), "should find a CUDA compiler");
        assert_eq!(found.unwrap().template.name, "nvcc");
    }

    // ── object_path / dep_file_path ───────────────────────────────────────────

    #[test]
    fn object_path_maps_source_to_objs() {
        let obj = object_path(
            Path::new("/project"),
            "debug",
            Path::new("src/core/engine.cpp"),
        );
        assert_eq!(
            obj,
            PathBuf::from("/project/target/debug/objs/src/core/engine.o")
        );
    }

    #[test]
    fn dep_file_path_has_d_extension() {
        let dep = dep_file_path(Path::new("/project"), "dev", Path::new("src/main.cpp"));
        assert_eq!(dep, PathBuf::from("/project/target/dev/objs/src/main.d"));
    }

    // ── parse_dep_file ────────────────────────────────────────────────────────

    #[test]
    fn parse_dep_file_extracts_prerequisites() {
        let contents = "target/main.o: src/main.cpp inc/foo.hpp \\\n  /usr/include/iostream\n";
        let deps = parse_dep_file(contents);
        assert!(deps.contains(&"src/main.cpp".to_string()));
        assert!(deps.contains(&"inc/foo.hpp".to_string()));
        assert!(deps.contains(&"/usr/include/iostream".to_string()));
    }

    #[test]
    fn parse_dep_file_empty_returns_empty() {
        assert!(parse_dep_file("").is_empty());
        assert!(parse_dep_file("target.o:").is_empty());
    }

    // ── settings_for_lang ─────────────────────────────────────────────────────

    #[test]
    fn settings_picks_lang_specific_std_for_mixed_project() {
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
std = "c++20"
[language.c]
std = "c17"
[[bin]]
name = "p"
src  = "src/main.cpp"
"#;
        let manifest = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let s = settings_for_lang(&manifest, "dev", "cpp", &[], Path::new("/tmp"), &[]);
        assert_eq!(s.standard.as_deref(), Some("c++20"));

        let s2 = settings_for_lang(&manifest, "dev", "c", &[], Path::new("/tmp"), &[]);
        assert_eq!(s2.standard.as_deref(), Some("c17"));
    }

    #[test]
    fn settings_adds_include_dirs() {
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "p"
src  = "src/main.cpp"
"#;
        let manifest = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let extra = vec![PathBuf::from("inc")];
        let s = settings_for_lang(&manifest, "dev", "cpp", &extra, Path::new("/project"), &[]);
        assert!(s.include_paths.iter().any(|p| p.ends_with("inc")));
    }

    // ── multi-language compiler selection ─────────────────────────────────────

    #[test]
    fn cpp_lang_key_finds_compiler_with_cpp_linking() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("cpp", &Backend::default(), &detected, None).unwrap();
        assert!(compiler.template.linking.contains_key("cpp"));
    }

    #[test]
    fn c_lang_key_finds_compiler_with_c_linking_and_compile_binary() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("c", &Backend::default(), &detected, None).unwrap();
        let c_info = compiler
            .template
            .linking
            .get("c")
            .expect("should have linking.c");
        assert_eq!(c_info.abi, "c");
        assert!(
            c_info.compile_binary.is_some(),
            "C must declare compile_binary so it isn't compiled with g++/clang++"
        );
    }

    #[test]
    fn gcc_c_uses_different_binary_than_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        // gcc uses gcc as the C compile binary but g++ as the linker binary.
        let backend = Backend("gnu".into());
        let compiler = select_compiler("c", &backend, &detected, None).unwrap();
        let c_info = compiler.template.linking.get("c").unwrap();
        assert_ne!(
            c_info
                .compile_binary
                .as_deref()
                .unwrap_or(&compiler.template.binary),
            compiler.template.binary.as_str(),
            "gcc C compile binary (gcc) should differ from linker binary (g++)"
        );
    }

    #[test]
    fn resolve_compile_binary_returns_override_for_c() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("c", &Backend::default(), &detected, None).unwrap();
        let bin = resolve_compile_binary(compiler, "c");
        // The resolved binary should NOT be g++ or clang++.
        let name = bin.file_name().unwrap().to_string_lossy();
        assert!(
            !name.ends_with("++"),
            "C should not compile with a C++ binary, got {name}"
        );
    }

    #[test]
    fn resolve_compile_binary_returns_compiler_path_for_cpp() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("cpp", &Backend::default(), &detected, None).unwrap();
        let bin = resolve_compile_binary(compiler, "cpp");
        assert_eq!(
            bin, compiler.path,
            "C++ should compile with the template's main binary"
        );
    }

    // ── is_up_to_date ─────────────────────────────────────────────────────────

    #[test]
    fn missing_object_is_not_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("main.cpp");
        fs::write(&src, "").unwrap();
        let obj = dir.path().join("main.o");
        let dep = dir.path().join("main.d");
        assert!(!is_up_to_date(&src, &obj, &dep));
    }

    #[test]
    fn existing_object_newer_than_source_is_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("main.cpp");
        fs::write(&src, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let obj = dir.path().join("main.o");
        fs::write(&obj, "").unwrap();
        let dep = dir.path().join("main.d");
        assert!(is_up_to_date(&src, &obj, &dep));
    }

    #[test]
    fn stale_header_triggers_recompile() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("main.cpp");
        let obj = dir.path().join("main.o");
        let dep = dir.path().join("main.d");
        let hdr = dir.path().join("foo.hpp");

        fs::write(&src, "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&obj, "").unwrap();
        // Write dep file listing the header, then make header newer than obj.
        let dep_contents = format!("main.o: main.cpp {}\n", hdr.display());
        fs::write(&dep, dep_contents).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&hdr, "").unwrap();

        assert!(
            !is_up_to_date(&src, &obj, &dep),
            "stale header should trigger recompile"
        );
    }
}
