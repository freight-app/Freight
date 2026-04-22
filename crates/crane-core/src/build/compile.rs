use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use rayon::prelude::*;

use crate::error::CraneError;
use crate::manifest::types::{Backend, Manifest};
use crate::toolchain::template::BuildSettings;
use crate::toolchain::DetectedCompiler;
use super::discover::SourceFile;

// ── Public API ────────────────────────────────────────────────────────────────

pub struct CompileResult {
    /// All object files that exist after this call (compiled or already up-to-date).
    pub objects: Vec<PathBuf>,
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
    profile: &str,
    sources: &[SourceFile],
    include_dirs: &[PathBuf],
    detected: &[DetectedCompiler],
) -> Result<CompileResult, CraneError> {
    let results: Result<Vec<(PathBuf, bool)>, CraneError> = sources
        .par_iter()
        .map(|src| -> Result<(PathBuf, bool), CraneError> {
            let src_abs = project_dir.join(&src.path);
            let obj = object_path(project_dir, profile, &src.path);
            let dep = dep_file_path(project_dir, profile, &src.path);

            if is_up_to_date(&src_abs, &obj, &dep) {
                print_fresh(&src.path);
                return Ok((obj, false));
            }

            let compiler = select_compiler(&src.lang_key, &manifest.compiler.backend, detected)
                .ok_or_else(|| CraneError::NoCompilerForLang(src.lang_key.clone()))?;

            let settings = settings_for_lang(manifest, profile, &src.lang_key, include_dirs, project_dir);
            let compile_bin = resolve_compile_binary(compiler, &src.lang_key);

            fs::create_dir_all(obj.parent().expect("obj path always has a parent"))?;

            print_compiling(&src.path);
            compile_one(&src_abs, &obj, &dep, &compile_bin, compiler, &settings)?;

            Ok((obj, true))
        })
        .collect();

    let pairs = results?;
    let objects = pairs.iter().map(|(o, _)| o.clone()).collect();
    let compiled = pairs.iter().filter(|(_, c)| *c).count();
    let skipped = pairs.iter().filter(|(_, c)| !*c).count();

    Ok(CompileResult { objects, compiled, skipped })
}

// ── Compiler selection ────────────────────────────────────────────────────────

/// Pick the compiler to use for `lang_key` given the backend preference.
///
/// `backend = "auto"` → first detected compiler whose template declares `lang_key`.
/// Any other name     → the detected compiler whose template name matches exactly.
pub fn select_compiler<'a>(
    lang_key: &str,
    backend: &Backend,
    detected: &'a [DetectedCompiler],
) -> Option<&'a DetectedCompiler> {
    if backend.is_auto() {
        detected.iter().find(|d| d.template.linking.contains_key(lang_key))
    } else {
        let name = backend.name();
        detected.iter().find(|d| d.template.name == name)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Produce build settings for a specific lang_key, merging in discovered include dirs.
pub fn settings_for_lang(
    manifest: &Manifest,
    profile: &str,
    lang_key: &str,
    extra_include_dirs: &[PathBuf],
    project_dir: &Path,
) -> BuildSettings {
    let mut s = manifest.build_settings_for(profile);
    if s.standard.is_none() {
        s.standard = manifest.language.get(lang_key).and_then(|l| l.std.clone());
    }
    for dir in extra_include_dirs {
        s.include_paths.push(project_dir.join(dir));
    }
    s
}

/// `src/core/engine.cpp` → `{project}/target/{profile}/objs/src/core/engine.o`
pub fn object_path(project_dir: &Path, profile: &str, source_rel: &Path) -> PathBuf {
    let mut p = project_dir.join("target").join(profile).join("objs").join(source_rel);
    p.set_extension("o");
    p
}

/// Same as `object_path` but with `.d` extension for the Makefile dependency file.
pub fn dep_file_path(project_dir: &Path, profile: &str, source_rel: &Path) -> PathBuf {
    let mut p = project_dir.join("target").join(profile).join("objs").join(source_rel);
    p.set_extension("d");
    p
}

/// Return `true` if the object is newer than the source and all its included headers.
fn is_up_to_date(source: &Path, object: &Path, dep_file: &Path) -> bool {
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
    let Some(colon) = contents.find(':') else { return vec![] };
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
fn resolve_compile_binary(compiler: &DetectedCompiler, lang_key: &str) -> PathBuf {
    if let Some(cb) = compiler.template.linking.get(lang_key)
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
fn compile_one(
    source_abs: &Path,
    obj_path: &Path,
    dep_path: &Path,
    compile_bin: &Path,
    compiler: &DetectedCompiler,
    settings: &BuildSettings,
) -> Result<(), CraneError> {
    let mut cmd = Command::new(compile_bin);
    cmd.args(compiler.template.assemble_flags(settings));
    cmd.args(compiler.template.compile_only_flag());
    cmd.args(compiler.template.dep_file_flags(dep_path));
    cmd.arg(source_abs);
    cmd.args(compiler.template.output_flag(obj_path));

    let out = cmd.output().map_err(CraneError::Io)?;
    if out.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let diag = if stdout.is_empty() { stderr } else { format!("{stdout}\n{stderr}") };
    Err(CraneError::CompileFailed(
        source_abs.to_string_lossy().into_owned(),
        diag.trim().to_owned(),
    ))
}

// ── Progress output ───────────────────────────────────────────────────────────

fn print_compiling(path: &Path) {
    use owo_colors::OwoColorize;
    println!("  {} {}", "Compiling".bold().green(), path.display());
}

fn print_fresh(path: &Path) {
    use owo_colors::OwoColorize;
    println!("    {} {}", "Fresh".dimmed(), path.display());
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::CompilerTemplate;

    const TEMPLATES_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../compiler-templates");

    fn templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_templates(std::path::Path::new(TEMPLATES_DIR))
    }

    fn fake_detected(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
        templates.iter().map(|t| DetectedCompiler {
            template: t.clone(),
            version: "0.0.0".into(),
            path: PathBuf::from(format!("/usr/bin/{}", t.binary)),
        }).collect()
    }

    // ── select_compiler ───────────────────────────────────────────────────────

    #[test]
    fn auto_backend_picks_first_with_lang_key() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend::default();
        let found = select_compiler("cpp", &backend, &detected);
        assert!(found.is_some(), "should find a C++ compiler");
        assert!(found.unwrap().template.linking.contains_key("cpp"));
    }

    #[test]
    fn named_backend_matches_template_name() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend("gcc".into());
        let found = select_compiler("cpp", &backend, &detected);
        assert!(found.is_some());
        assert_eq!(found.unwrap().template.name, "gcc");
    }

    #[test]
    fn unknown_backend_returns_none() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend("nonexistent".into());
        assert!(select_compiler("cpp", &backend, &detected).is_none());
    }

    #[test]
    fn auto_backend_for_cuda_picks_nvcc() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let backend = Backend::default();
        let found = select_compiler("cuda", &backend, &detected);
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
        assert_eq!(obj, PathBuf::from("/project/target/debug/objs/src/core/engine.o"));
    }

    #[test]
    fn dep_file_path_has_d_extension() {
        let dep = dep_file_path(
            Path::new("/project"),
            "dev",
            Path::new("src/main.cpp"),
        );
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
        let s = settings_for_lang(&manifest, "dev", "cpp", &[], Path::new("/tmp"));
        assert_eq!(s.standard.as_deref(), Some("c++20"));

        let s2 = settings_for_lang(&manifest, "dev", "c", &[], Path::new("/tmp"));
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
        let s = settings_for_lang(&manifest, "dev", "cpp", &extra, Path::new("/project"));
        assert!(s.include_paths.iter().any(|p| p.ends_with("inc")));
    }

    // ── multi-language compiler selection ─────────────────────────────────────

    #[test]
    fn cpp_lang_key_finds_compiler_with_cpp_linking() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("cpp", &Backend::default(), &detected).unwrap();
        assert!(compiler.template.linking.contains_key("cpp"));
    }

    #[test]
    fn c_lang_key_finds_compiler_with_c_linking_and_compile_binary() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("c", &Backend::default(), &detected).unwrap();
        let c_info = compiler.template.linking.get("c").expect("should have linking.c");
        assert_eq!(c_info.abi, "c");
        assert!(c_info.compile_binary.is_some(),
            "C must declare compile_binary so it isn't compiled with g++/clang++");
    }

    #[test]
    fn c_files_use_different_binary_than_cpp_files() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("c", &Backend::default(), &detected).unwrap();
        let c_info = compiler.template.linking.get("c").unwrap();
        // The compile_binary for C must differ from the template's main linker binary.
        assert_ne!(
            c_info.compile_binary.as_deref().unwrap_or(&compiler.template.binary),
            compiler.template.binary.as_str(),
            "C compile binary should differ from the linker binary (e.g. gcc vs g++)"
        );
    }

    #[test]
    fn resolve_compile_binary_returns_override_for_c() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("c", &Backend::default(), &detected).unwrap();
        let bin = resolve_compile_binary(compiler, "c");
        // The resolved binary should NOT be g++ or clang++.
        let name = bin.file_name().unwrap().to_string_lossy();
        assert!(!name.ends_with("++"), "C should not compile with a C++ binary, got {name}");
    }

    #[test]
    fn resolve_compile_binary_returns_compiler_path_for_cpp() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let compiler = select_compiler("cpp", &Backend::default(), &detected).unwrap();
        let bin = resolve_compile_binary(compiler, "cpp");
        assert_eq!(bin, compiler.path, "C++ should compile with the template's main binary");
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

        assert!(!is_up_to_date(&src, &obj, &dep), "stale header should trigger recompile");
    }
}
