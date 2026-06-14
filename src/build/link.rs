use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::types::{Backend, Dependency, LibType, Manifest};
use crate::toolchain::template::BuildSettings;
use crate::toolchain::{CompilerTemplate, DetectedCompiler};
use crate::vendor::parse_triple;

use super::compile::object_path;

// ── Up-to-date check ──────────────────────────────────────────────────────────

/// Returns true if `output` exists and is newer than every file in `inputs`.
fn output_is_fresh(output: &Path, inputs: &[&Path]) -> bool {
    let Ok(out_meta) = std::fs::metadata(output) else {
        return false;
    };
    let Ok(out_mtime) = out_meta.modified() else {
        return false;
    };
    inputs.iter().all(|inp| {
        std::fs::metadata(inp)
            .and_then(|m| m.modified())
            .map(|t| t <= out_mtime)
            .unwrap_or(false)
    })
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Priority order for linker selection: the first language in this list that is
/// active in the project wins the link step.
const LINK_PRIORITY: &[&str] = &[
    "cpp", "objcpp", "cuda", "hip", "sycl", "objc", "c", "fortran", "ada", "d", "zig", "opencl",
    "ispc",
];

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
/// `dep_libs` are pre-built `.a` archives from `target/deps/` that are linked in
/// before any system libraries.
pub fn link_targets(
    _project_dir: &Path,
    target_dir: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    objects: &[PathBuf],
    detected: &[DetectedCompiler],
    templates: &[CompilerTemplate],
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
    progress: &Progress,
) -> Result<LinkResult, FreightError> {
    let mut outputs: Vec<PathBuf> = Vec::new();
    let target_os = link_target_os(manifest);

    for bin in &manifest.bins {
        let out = target_dir
            .join(profile)
            .join(executable_name(&bin.name, &target_os));
        let linker = select_linker(manifest, backend, detected, templates)
            .ok_or_else(|| FreightError::CompilerNotFound("no suitable linker found".into()))?;

        // Exclude other bins' entry-point objects so each binary only has one main().
        let other_entry_objs: HashSet<PathBuf> = manifest
            .bins
            .iter()
            .filter(|b| b.src != bin.src)
            .map(|b| object_path(target_dir, profile, Path::new(&b.src)))
            .collect();
        let bin_objects: Vec<PathBuf> = objects
            .iter()
            .filter(|o| !other_entry_objs.contains(*o))
            .cloned()
            .collect();

        // Whole-program builders (e.g. gnatmake) compile + bind + link in one shot.
        // Emit a Compiling event here so the CLI shows progress for Ada projects
        // that otherwise produce no individual Compiling events during the compile step.
        if linker.template.linking.values().any(|l| l.whole_program) {
            // The "objects" list contains absolute source paths for whole-program langs.
            for src in bin_objects
                .iter()
                .filter(|p| p.extension().and_then(|e| e.to_str()) != Some("o"))
            {
                progress(BuildEvent::Compiling { path: src.clone() });
            }
        }
        let linked = link_executable(
            &bin_objects,
            &out,
            linker,
            manifest,
            profile,
            dep_libs,
            extra_link_flags,
        )?;
        if linked {
            progress(BuildEvent::Linking {
                name: bin.name.clone(),
            });
        }
        if link_settings(manifest, profile).strip {
            strip_output(&out, linker)?;
        }
        outputs.push(out);
    }

    if let Some(lib) = &manifest.lib {
        // Prebuilt libs (link is set) have no objects to archive or link.
        if lib.link.is_none() {
            match lib.lib_type {
                LibType::Static => {
                    let out = target_dir
                        .join(profile)
                        .join(format!("lib{}.a", manifest.package.name));
                    let linker =
                        select_linker(manifest, backend, detected, templates).ok_or_else(|| {
                            FreightError::CompilerNotFound("no suitable linker found".into())
                        })?;
                    if link_static(&out, objects, linker.template.ar_binary())? {
                        progress(BuildEvent::Archiving {
                            name: format!("lib{}.a", manifest.package.name),
                        });
                    }
                    outputs.push(out);
                }
                LibType::Shared => {
                    let lib_name = shared_lib_name(&manifest.package.name, &target_os);
                    let out = target_dir.join(profile).join(&lib_name);
                    let linker =
                        select_linker(manifest, backend, detected, templates).ok_or_else(|| {
                            FreightError::CompilerNotFound("no suitable linker found".into())
                        })?;
                    if link_shared(
                        objects,
                        &out,
                        linker,
                        manifest,
                        profile,
                        dep_libs,
                        extra_link_flags,
                    )? {
                        progress(BuildEvent::Linking {
                            name: lib_name.clone(),
                        });
                    }
                    if link_settings(manifest, profile).strip {
                        strip_output(&out, linker)?;
                    }
                    outputs.push(out);
                }
                LibType::Header => {}
            }
        }
    }

    Ok(LinkResult { outputs })
}

/// Link a test binary from the given objects (test `.o` + lib objects from the project).
pub fn link_test_binary(
    objects: &[PathBuf],
    out: &Path,
    manifest: &Manifest,
    backend: &Backend,
    profile: &str,
    detected: &[DetectedCompiler],
    templates: &[CompilerTemplate],
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<(), FreightError> {
    let linker = select_linker(manifest, backend, detected, templates)
        .ok_or_else(|| FreightError::CompilerNotFound("no suitable linker found".into()))?;
    link_executable(
        objects,
        out,
        linker,
        manifest,
        profile,
        dep_libs,
        extra_link_flags,
    )
    .map(|_| ())
}

/// Archive a set of object files into a static library.
/// `ar_bin` is the archiver binary to use (from `CompilerTemplate::ar_binary()`).
pub fn link_static_lib(objects: &[PathBuf], out: &Path, ar_bin: &str) -> Result<(), FreightError> {
    link_static(out, objects, ar_bin).map(|_| ())
}

/// Pick the compiler binary that drives the final link step.
///
/// Priority order:
/// 1. If any language template declares a non-empty `linker` ABI (e.g. CUDA→`"c++"`),
///    use the detected compiler that produces that ABI.
/// 2. When `backend` is non-auto, prefer linkers from that family (e.g. `g++` for `gnu`).
/// 3. Among active languages, prefer the most link-capable one.
/// 4. Fall back to the first detected compiler.
pub fn select_linker<'a>(
    manifest: &Manifest,
    backend: &Backend,
    detected: &'a [DetectedCompiler],
    _templates: &[CompilerTemplate],
) -> Option<&'a DetectedCompiler> {
    for lang_key in manifest.language.keys() {
        for d in detected {
            if let Some(l) = d.template.linking.get(lang_key.as_str()) {
                if l.linker.is_empty() {
                    continue;
                }
                let found = detected
                    .iter()
                    .find(|dd| dd.template.linking.values().any(|li| li.abi == l.linker));
                if found.is_some() {
                    return found;
                }
            }
        }
    }

    // Non-auto backend: prefer a linker from the requested family first.
    if !backend.is_auto() {
        let family = backend.name();
        // First: prefer a linker whose own lang_key matches the backend name (e.g. zig_native
        // for backend="zig"). This ensures zig build-exe is preferred over zig c++ for
        // zig+cpp projects, because zig build-exe handles startup correctly in both cases.
        if let Some(own) = detected.iter().find(|d| {
            d.template.linking.contains_key(family)
                && (d.template.family == family || d.template.name == family)
        }) {
            return Some(own);
        }
        for &lang in LINK_PRIORITY {
            if super::has_lang(manifest, lang, detected) {
                let found = detected.iter().find(|d| {
                    d.template.linking.contains_key(lang)
                        && (d.template.family == family || d.template.name == family)
                });
                if found.is_some() {
                    return found;
                }
            }
        }
    }

    for &lang in LINK_PRIORITY {
        if super::has_lang(manifest, lang, detected) {
            let found = detected
                .iter()
                .find(|d| d.template.linking.contains_key(lang));
            if found.is_some() {
                return found;
            }
        }
    }

    detected.first()
}

// ── Link commands ─────────────────────────────────────────────────────────────

/// Returns `true` if linking was performed, `false` if the output was already fresh.
fn link_executable(
    objects: &[PathBuf],
    out: &Path,
    linker: &DetectedCompiler,
    manifest: &Manifest,
    profile: &str,
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<bool, FreightError> {
    // Skip link if binary is newer than all inputs.
    let all_inputs: Vec<&Path> = objects
        .iter()
        .map(PathBuf::as_path)
        .chain(dep_libs.iter().map(PathBuf::as_path))
        .collect();
    if output_is_fresh(out, &all_inputs) {
        return Ok(false);
    }

    // Whole-program builders (e.g. gnatmake for Ada) receive source file paths instead
    // of object files and handle compile + bind + link themselves.
    let is_whole_program = linker.template.linking.values().any(|l| l.whole_program);

    let mut cmd = Command::new(&linker.path);

    if is_whole_program {
        // Run from the obj dir so gnatmake's bind phase writes b~*.adb/o there too,
        // not into the project root. The source paths must be absolute.
        let obj_dir = out.parent().map(|p| p.join("objs")).unwrap_or_default();
        let _ = std::fs::create_dir_all(&obj_dir);
        cmd.current_dir(&obj_dir);
        cmd.args(
            linker
                .template
                .assemble_link_flags(&link_settings(manifest, profile)),
        );
        cmd.args(objects);
        cmd.args(linker.template.output_bin_flag(out));
        run_cmd(cmd, out)?;
        return Ok(true);
    } else {
        let link_sub = linker
            .template
            .link_subcommand
            .as_deref()
            .or(linker.template.subcommand.as_deref());
        if let Some(sub) = link_sub {
            cmd.arg(sub);
        }
        cmd.args(
            linker
                .template
                .assemble_link_flags(&link_settings(manifest, profile)),
        );
        cmd.args(objects);
        cmd.args(dep_libs);
        for flag in collect_system_lib_flags(manifest, &linker.template) {
            cmd.arg(flag);
        }
        cmd.args(extra_link_flags);
    }
    cmd.args(linker.template.output_bin_flag(out));
    run_cmd(cmd, out)?;
    Ok(true)
}

/// Returns `true` if archiving was performed, `false` if the output was already fresh.
fn link_static(out: &Path, objects: &[PathBuf], ar_bin: &str) -> Result<bool, FreightError> {
    let inputs: Vec<&Path> = objects.iter().map(PathBuf::as_path).collect();
    if output_is_fresh(out, &inputs) {
        return Ok(false);
    }
    let mut cmd = Command::new(ar_bin);
    cmd.arg("rcs").arg(out).args(objects);
    run_cmd(cmd, out)?;
    Ok(true)
}

fn shared_lib_name(name: &str, target_os: &str) -> String {
    match target_os {
        "macos" => format!("lib{name}.dylib"),
        "windows" => format!("{name}.dll"),
        _ => format!("lib{name}.so"),
    }
}

/// The on-disk executable file name for `name` on `target_os` (adds `.exe` on
/// Windows). Shared with the install/package path.
pub(crate) fn executable_name(name: &str, target_os: &str) -> String {
    if target_os == "windows" && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn link_target_os(manifest: &Manifest) -> String {
    manifest
        .compiler
        .target
        .as_deref()
        .map(parse_triple)
        .map(|(_, os)| os)
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

/// Returns `true` if linking was performed, `false` if the output was already fresh.
fn link_shared(
    objects: &[PathBuf],
    out: &Path,
    linker: &DetectedCompiler,
    manifest: &Manifest,
    profile: &str,
    dep_libs: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<bool, FreightError> {
    let all_inputs: Vec<&Path> = objects
        .iter()
        .map(PathBuf::as_path)
        .chain(dep_libs.iter().map(PathBuf::as_path))
        .collect();
    if output_is_fresh(out, &all_inputs) {
        return Ok(false);
    }
    let target_os = link_target_os(manifest);
    let shared_flag = if target_os == "macos" {
        "-dynamiclib"
    } else {
        "-shared"
    };
    let mut cmd = Command::new(&linker.path);
    let link_sub = linker
        .template
        .link_subcommand
        .as_deref()
        .or(linker.template.subcommand.as_deref());
    if let Some(sub) = link_sub {
        cmd.arg(sub);
    }
    cmd.args(
        linker
            .template
            .assemble_link_flags(&link_settings(manifest, profile)),
    );
    cmd.arg(shared_flag);
    cmd.args(objects);
    cmd.args(dep_libs);
    for flag in collect_system_lib_flags(manifest, &linker.template) {
        cmd.arg(flag);
    }
    cmd.args(extra_link_flags);
    cmd.args(linker.template.output_bin_flag(out));
    run_cmd(cmd, out)?;
    Ok(true)
}

fn strip_output(out: &Path, linker: &DetectedCompiler) -> Result<(), FreightError> {
    let Some(strip_bin) = linker.template.strip_binary() else {
        return Ok(());
    };
    let status = Command::new(strip_bin)
        .arg(out)
        .status()
        .map_err(|e| FreightError::CompilerNotFound(format!("{strip_bin} not found: {e}")))?;
    if !status.success() {
        eprintln!(
            "warning: strip of '{}' failed (exit {})",
            out.display(),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn run_cmd(mut cmd: Command, out: &Path) -> Result<(), FreightError> {
    if std::env::var_os("FREIGHT_VERBOSE").is_some() {
        super::compile::print_cmd(&cmd);
    }
    let result = cmd.output().map_err(FreightError::Io)?;
    if result.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let diag = if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    };
    Err(FreightError::CompileFailed(
        out.to_string_lossy().into_owned(),
        diag.trim().to_owned(),
    ))
}

// ── Dependency helpers ────────────────────────────────────────────────────────

/// Collect link flags for platform package dependencies (`windows`, `linux`, `macos`, …).
///
/// Each feature of a platform dep maps to a link flag:
/// - macOS: leading-uppercase feature → `-framework <Name>`; otherwise → `-l<name>`
/// - All others: → toolchain `system_lib_flag(name)` (GCC/Clang: `-l<name>`, MSVC: `<name>.lib`)
fn collect_system_lib_flags(manifest: &Manifest, linker: &CompilerTemplate) -> Vec<String> {
    use crate::manifest::types::is_platform_dep;
    let effective = manifest.effective_dependencies();
    let is_macos = std::env::consts::OS == "macos";
    effective
        .iter()
        .chain(manifest.dev_dependencies.iter())
        .filter(|(name, dep)| is_platform_dep(name) && matches!(dep, Dependency::Detailed(_)))
        .flat_map(|(_, dep)| {
            let Dependency::Detailed(d) = dep else {
                return vec![];
            };
            d.features
                .iter()
                .map(|feat| {
                    if is_macos && feat.chars().next().is_some_and(|c| c.is_uppercase()) {
                        format!("-framework {feat}")
                    } else {
                        linker.system_lib_flag(feat)
                    }
                })
                .collect::<Vec<_>>()
        })
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::CompilerTemplate;
    use std::path::PathBuf;

    fn templates() -> Vec<CompilerTemplate> {
        crate::toolchain::load_all_templates()
    }

    fn gcc() -> CompilerTemplate {
        templates().into_iter().find(|t| t.name == "g++").unwrap()
    }
    fn msvc() -> CompilerTemplate {
        templates().into_iter().find(|t| t.name == "msvc").unwrap()
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
        let linker = select_linker(&m, &Backend::default(), &detected, &ts).unwrap();
        assert!(linker.template.linking.contains_key("cpp"));
    }

    #[test]
    fn cuda_project_picks_nvcc_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("cuda");
        let linker = select_linker(&m, &Backend::default(), &detected, &ts).unwrap();
        assert!(
            linker.template.linking.values().any(|l| l.abi == "cuda"),
            "CUDA should use nvcc as linker, got: {}",
            linker.template.name
        );
    }

    #[test]
    fn hip_project_picks_cpp_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("hip");
        let linker = select_linker(&m, &Backend::default(), &detected, &ts).unwrap();
        assert!(linker.template.linking.values().any(|l| l.abi == "c++"));
    }

    #[test]
    fn c_project_picks_c_or_cpp_linker() {
        let ts = templates();
        let detected = fake_detected(&ts);
        let m = manifest("c");
        let linker = select_linker(&m, &Backend::default(), &detected, &ts).unwrap();
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
        let linker = select_linker(&m, &Backend::default(), &detected, &ts).unwrap();
        // cpp has higher PRIORITY than c → linker must handle cpp
        assert!(
            linker.template.linking.contains_key("cpp"),
            "mixed C/C++ project should use a C++ linker, got: {}",
            linker.template.name
        );
    }

    #[test]
    fn named_backend_matches_linker_by_template_name_not_just_family() {
        // When backend = "ldc2", select_linker must pick ldc2 (family "llvm"),
        // not dmd (family ""), even though "ldc2" != "llvm".
        let ts = templates();
        let detected = fake_detected(&ts);
        let manifest_src = r#"
[package]
name    = "p"
version = "0.1.0"
[language.d]
[[bin]]
name = "p"
src  = "src/main.d"
[compiler]
backend = "ldc2"
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let backend = m.compiler.backend.clone();
        let linker = select_linker(&m, &backend, &detected, &ts).unwrap();
        assert_eq!(
            linker.template.name, "ldc2",
            "ldc2 backend must use ldc2 as the linker, not '{}'",
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
linux = { features = ["pthread", "dl"] }
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let flags = collect_system_lib_flags(&m, &gcc());
        assert!(flags.contains(&"-lpthread".to_string()));
        assert!(flags.contains(&"-ldl".to_string()));
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
windows = { features = ["ws2_32", "crypt32"] }
"#;
        let m = crate::manifest::load_manifest_str(manifest_src).unwrap();
        let flags = collect_system_lib_flags(&m, &msvc());
        assert!(
            flags.contains(&"ws2_32.lib".to_string()),
            "MSVC should use {{name}}.lib, got: {flags:?}"
        );
        assert!(flags.contains(&"crypt32.lib".to_string()));
        assert!(
            !flags.iter().any(|f| f.starts_with("-l")),
            "MSVC must not emit -l flags"
        );
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
        assert!(s.lto, "LTO should be preserved for link step");
        assert!(s.strip, "strip should be preserved for link step");
    }
}
