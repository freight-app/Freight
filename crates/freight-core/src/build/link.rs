use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::manifest::types::{Dependency, LibType, Manifest};
use crate::toolchain::template::BuildSettings;
use crate::toolchain::{CompilerTemplate, DetectedCompiler};

use super::compile::object_path;

// ── Public API ────────────────────────────────────────────────────────────────

pub struct LinkResult {
    /// All produced output files (binaries and/or libraries).
    pub outputs: Vec<PathBuf>,
}

/// Link compiled objects into every target declared in the manifest.
///
/// - Each `[[bin]]` → executable in `target/{profile}/{name}`
/// - `[lib] type = "static"` → `target/{profile}/lib{name}.a` (via `ar`)
/// - `[lib] type = "shared"` → `target/{profile}/lib{name}.so` (Linux), `.dylib` (macOS), `.dll` (Windows)
/// - `[lib] type = "header-only"` → nothing to link
///
/// `dep_libs` are pre-built `.a` archives from `.deps/` that are linked in
/// before any system libraries.
pub fn link_targets(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    objects: &[PathBuf],
    detected: &[DetectedCompiler],
    templates: &[CompilerTemplate],
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<LinkResult, FreightError> {
    let mut outputs: Vec<PathBuf> = Vec::new();

    for bin in &manifest.bins {
        let out = project_dir.join("target").join(profile).join(&bin.name);
        let linker = select_linker(manifest, detected, templates)
            .ok_or_else(|| FreightError::CompilerNotFound("no suitable linker found".into()))?;

        // Exclude other bins' entry-point objects so each binary only has one main().
        let other_entry_objs: HashSet<PathBuf> = manifest.bins.iter()
            .filter(|b| b.src != bin.src)
            .map(|b| object_path(project_dir, profile, Path::new(&b.src)))
            .collect();
        let bin_objects: Vec<PathBuf> = objects.iter()
            .filter(|o| !other_entry_objs.contains(*o))
            .cloned()
            .collect();

        print_linking(&bin.name);
        link_executable(&bin_objects, &out, linker, manifest, profile, dep_libs, extra_link_flags)?;
        outputs.push(out);
    }

    if let Some(lib) = &manifest.lib {
        match lib.lib_type {
            LibType::Static => {
                let out = project_dir.join("target").join(profile)
                    .join(format!("lib{}.a", manifest.package.name));
                let linker = select_linker(manifest, detected, templates)
                    .ok_or_else(|| FreightError::CompilerNotFound("no suitable linker found".into()))?;
                print_archiving(out.file_name().unwrap_or_default().to_str().unwrap_or("lib"));
                link_static(&out, objects, linker.template.ar_binary())?;
                outputs.push(out);
            }
            LibType::Shared => {
                let lib_name = shared_lib_name(&manifest.package.name);
                let out = project_dir.join("target").join(profile).join(&lib_name);
                let linker = select_linker(manifest, detected, templates)
                    .ok_or_else(|| FreightError::CompilerNotFound("no suitable linker found".into()))?;
                print_linking(out.file_name().unwrap_or_default().to_str().unwrap_or("lib"));
                link_shared(objects, &out, linker, manifest, profile, dep_libs, extra_link_flags)?;
                outputs.push(out);
            }
            LibType::HeaderOnly => {}
        }
    }

    Ok(LinkResult { outputs })
}

/// Link a test binary from the given objects (test `.o` + lib objects from the project).
pub fn link_test_binary(
    objects: &[PathBuf],
    out: &Path,
    manifest: &Manifest,
    profile: &str,
    detected: &[DetectedCompiler],
    templates: &[CompilerTemplate],
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<(), FreightError> {
    let linker = select_linker(manifest, detected, templates)
        .ok_or_else(|| FreightError::CompilerNotFound("no suitable linker found".into()))?;
    link_executable(objects, out, linker, manifest, profile, dep_libs, extra_link_flags)
}

/// Archive a set of object files into a static library.
/// `ar_bin` is the archiver binary to use (from `CompilerTemplate::ar_binary()`).
pub fn link_static_lib(objects: &[PathBuf], out: &Path, ar_bin: &str) -> Result<(), FreightError> {
    link_static(out, objects, ar_bin)
}

/// Pick the compiler binary that drives the final link step.
///
/// Priority order:
/// 1. If any language template declares a non-empty `linker` ABI (e.g. CUDA→`"c++"`),
///    use the detected compiler that produces that ABI.
/// 2. Among active languages, prefer the most link-capable one.
/// 3. Fall back to the first detected compiler.
pub fn select_linker<'a>(
    manifest: &Manifest,
    detected: &'a [DetectedCompiler],
    templates: &[CompilerTemplate],
) -> Option<&'a DetectedCompiler> {
    for (lang_key, _) in &manifest.language {
        for template in templates {
            if let Some(l) = template.linking.get(lang_key.as_str()) {
                if l.linker.is_empty() { continue; }
                let found = detected.iter()
                    .find(|d| d.template.linking.values().any(|li| li.abi == l.linker));
                if found.is_some() { return found; }
            }
        }
    }

    const PRIORITY: &[&str] = &[
        "cpp", "cuda", "hip", "sycl", "c", "fortran", "ada", "d", "opencl", "ispc",
    ];
    for &lang in PRIORITY {
        if manifest.language.contains_key(lang) {
            let found = detected.iter()
                .find(|d| d.template.linking.contains_key(lang));
            if found.is_some() { return found; }
        }
    }

    detected.first()
}

// ── Link commands ─────────────────────────────────────────────────────────────

fn link_executable(
    objects: &[PathBuf],
    out: &Path,
    linker: &DetectedCompiler,
    manifest: &Manifest,
    profile: &str,
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<(), FreightError> {
    let mut cmd = Command::new(&linker.path);
    cmd.args(linker.template.assemble_link_flags(&link_settings(manifest, profile)));
    cmd.args(objects);
    cmd.args(dep_libs);
    for flag in collect_system_lib_flags(manifest, &linker.template) {
        cmd.arg(flag);
    }
    cmd.args(extra_link_flags);
    cmd.args(linker.template.output_bin_flag(out));
    run_cmd(cmd, out)
}

fn link_static(out: &Path, objects: &[PathBuf], ar_bin: &str) -> Result<(), FreightError> {
    let mut cmd = Command::new(ar_bin);
    cmd.arg("rcs").arg(out).args(objects);
    run_cmd(cmd, out)
}

fn shared_lib_name(name: &str) -> String {
    match std::env::consts::OS {
        "macos" => format!("lib{name}.dylib"),
        "windows" => format!("{name}.dll"),
        _ => format!("lib{name}.so"),
    }
}

fn link_shared(
    objects: &[PathBuf],
    out: &Path,
    linker: &DetectedCompiler,
    manifest: &Manifest,
    profile: &str,
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<(), FreightError> {
    let shared_flag = if std::env::consts::OS == "macos" { "-dynamiclib" } else { "-shared" };
    let mut cmd = Command::new(&linker.path);
    cmd.args(linker.template.assemble_link_flags(&link_settings(manifest, profile)));
    cmd.arg(shared_flag);
    cmd.args(objects);
    cmd.args(dep_libs);
    for flag in collect_system_lib_flags(manifest, &linker.template) {
        cmd.arg(flag);
    }
    cmd.args(extra_link_flags);
    cmd.args(linker.template.output_bin_flag(out));
    run_cmd(cmd, out)
}

fn run_cmd(mut cmd: Command, out: &Path) -> Result<(), FreightError> {
    if std::env::var_os("FREIGHT_VERBOSE").is_some() {
        super::compile::print_cmd(&cmd);
    }
    let result = cmd.output().map_err(FreightError::Io)?;
    if result.status.success() { return Ok(()); }
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let diag = if stdout.is_empty() { stderr } else { format!("{stdout}\n{stderr}") };
    Err(FreightError::CompileFailed(
        out.to_string_lossy().into_owned(),
        diag.trim().to_owned(),
    ))
}

// ── Dependency helpers ────────────────────────────────────────────────────────

/// Collect system library link flags for every `system = "..."` dependency.
///
/// The flag format is determined by `linker.system_lib_flag(name)` — GCC/Clang produce
/// `-l{name}`, MSVC produces `{name}.lib`.
///
/// Deps with `pkg_config = "..."` are skipped here — pkg-config already provides the
/// correct link flags via `raw_link_flags` from `build_foreign_deps`.
fn collect_system_lib_flags(manifest: &Manifest, linker: &CompilerTemplate) -> Vec<String> {
    let effective = manifest.effective_dependencies();
    effective.values()
        .chain(manifest.dev_dependencies.values())
        .filter_map(|dep| {
            if let Dependency::Detailed(d) = dep {
                if d.pkg_config.is_some() { return None; }
                d.system.as_deref()
            } else {
                None
            }
        })
        .map(|name| linker.system_lib_flag(name))
        .collect()
}


/// Strip link-irrelevant fields from BuildSettings before passing to the linker.
/// The linker doesn't want -std=, -Wall, -D, or -I flags.
pub fn link_settings(manifest: &Manifest, profile: &str) -> BuildSettings {
    let mut s = manifest.build_settings_for(profile);
    s.standard = None;
    s.warnings = "none".to_string();
    s.defines.clear();
    s.include_paths.clear();
    s
}

// ── Progress output ───────────────────────────────────────────────────────────

fn print_linking(name: &str) {
    use owo_colors::OwoColorize;
    println!("   {} {name}", "Linking".bold().cyan());
}

fn print_archiving(name: &str) {
    use owo_colors::OwoColorize;
    println!(" {} {name}", "Archiving".bold().cyan());
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use crate::toolchain::CompilerTemplate;

    const TEMPLATES_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../toolchains");
    const GCC_RHAI: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../toolchains/gcc.rhai"));
    const MSVC_RHAI: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../toolchains/msvc.rhai"));

    fn templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_templates(std::path::Path::new(TEMPLATES_DIR))
    }

    fn gcc() -> CompilerTemplate { CompilerTemplate::from_rhai(GCC_RHAI).unwrap() }
    fn msvc() -> CompilerTemplate { CompilerTemplate::from_rhai(MSVC_RHAI).unwrap() }

    fn fake_detected(templates: &[CompilerTemplate]) -> Vec<DetectedCompiler> {
        templates.iter().map(|t| DetectedCompiler {
            template: t.clone(),
            version: "0.0.0".into(),
            path: PathBuf::from(format!("/usr/bin/{}", t.binary)),
        }).collect()
    }

    fn manifest(lang_key: &str) -> crate::manifest::types::Manifest {
        let src = format!(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n\
             [language.{lang_key}]\n\
             [[bin]]\nname=\"p\"\nsrc=\"src/main.cpp\"\n"
        );
        crate::manifest::load_manifest_str(&src).unwrap()
    }

    // ── select_linker ─────────────────────────────────────────────────────────

    #[test]
    fn cpp_project_picks_cpp_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("cpp");
        let linker = select_linker(&m, &detected, &ts).unwrap();
        assert!(linker.template.linking.contains_key("cpp"));
    }

    #[test]
    fn cuda_project_picks_cpp_linker_via_required_linker_field() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("cuda");
        let linker = select_linker(&m, &detected, &ts).unwrap();
        assert!(linker.template.linking.values().any(|l| l.abi == "c++"),
            "CUDA should use C++ linker, got: {}", linker.template.name);
    }

    #[test]
    fn hip_project_picks_cpp_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("hip");
        let linker = select_linker(&m, &detected, &ts).unwrap();
        assert!(linker.template.linking.values().any(|l| l.abi == "c++"));
    }

    #[test]
    fn c_project_picks_c_or_cpp_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("c");
        let linker = select_linker(&m, &detected, &ts).unwrap();
        assert!(!linker.template.name.is_empty());
    }

    #[test]
    fn mixed_c_cpp_project_linker_is_cpp() {
        let ts = templates();
        let detected = fake_detected(&ts);
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
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let linker = select_linker(&m, &detected, &ts).unwrap();
        // cpp has higher PRIORITY than c → linker must handle cpp
        assert!(
            linker.template.linking.contains_key("cpp"),
            "mixed C/C++ project should use a C++ linker, got: {}",
            linker.template.name
        );
    }

    // ── collect_system_lib_flags ──────────────────────────────────────────────

    #[test]
    fn system_dep_produces_dash_l_flag() {
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "p"
src  = "src/main.cpp"
[dependencies]
OpenBLAS = { system = "openblas" }
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let flags = collect_system_lib_flags(&m, &gcc());
        assert!(flags.contains(&"-lopenblas".to_string()));
    }

    #[test]
    fn no_system_deps_returns_empty() {
        let m = manifest("cpp");
        assert!(collect_system_lib_flags(&m, &gcc()).is_empty());
    }

    #[test]
    fn msvc_system_dep_uses_dot_lib_format() {
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "p"
src  = "src/main.cpp"
[dependencies]
OpenSSL = { system = "ssl" }
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let flags = collect_system_lib_flags(&m, &msvc());
        assert!(flags.contains(&"ssl.lib".to_string()), "MSVC should use {{name}}.lib, got: {flags:?}");
        assert!(!flags.iter().any(|f| f.starts_with("-l")), "MSVC must not emit -l flags");
    }

    // ── link_settings ─────────────────────────────────────────────────────────

    #[test]
    fn link_settings_clears_std_warnings_defines_includes() {
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
std = "c++20"
[[bin]]
name = "p"
src  = "src/main.cpp"
[compiler]
warnings = "all"
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let s = link_settings(&m, "dev");
        assert!(s.standard.is_none(), "no -std= at link time");
        assert_eq!(s.warnings, "none", "no warnings at link time");
        assert!(s.defines.is_empty());
        assert!(s.include_paths.is_empty());
    }

    #[test]
    fn link_settings_preserves_lto_and_strip() {
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.cpp]
[[bin]]
name = "p"
src  = "src/main.cpp"
[profile.release]
lto   = true
strip = true
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let s = link_settings(&m, "release");
        assert!(s.lto,   "LTO should be preserved for link step");
        assert!(s.strip, "strip should be preserved for link step");
    }
}
