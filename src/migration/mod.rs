//! Deep migration of foreign build systems into freight manifests.
//!
//! Where the lightweight adoption path (`new::write_cmake_manifest`) delegates the
//! whole build back to CMake (`build = "cmake"`), this module *extracts the real
//! build data* — sources, defines, include dirs, language standard — from CMake's
//! [File API](https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html) and
//! renders a freight-**native** manifest from it.

pub mod cmake_fileapi;

pub use cmake_fileapi::{extract, CmakeModel, CmakeTarget, TargetKind};

/// Render a freight-native `freight.toml` from an extracted CMake model, as a single
/// freight package: up to one library (`[lib]`) plus any number of executables
/// (`[[bin]]`, which auto-link the library). Defines / include dirs are unioned
/// across targets into `[compiler]`, and the standard is taken from the library (or
/// the first executable). Returns `None` — so the caller falls back to a
/// `build = "cmake"` adoption — when the shape can't be represented faithfully in one
/// package: more than one library (needs a workspace), or any executable built from
/// more than one source (`[[bin]]` carries a single entry source).
pub fn render_native_manifest(name: &str, model: &CmakeModel) -> Option<String> {
    let libs: Vec<&CmakeTarget> = model
        .targets
        .iter()
        .filter(|t| matches!(t.kind, TargetKind::StaticLib | TargetKind::SharedLib))
        .collect();
    let exes: Vec<&CmakeTarget> = model
        .targets
        .iter()
        .filter(|t| matches!(t.kind, TargetKind::Executable))
        .collect();

    // > 1 library → workspace territory; a multi-source executable can't be a
    // single `[[bin]]`; nothing to build → fall back in all three cases.
    if libs.len() > 1 || exes.iter().any(|e| e.sources.len() != 1) {
        return None;
    }
    if libs.is_empty() && exes.is_empty() {
        return None;
    }

    let lib = libs.first().copied();
    // Language/standard come from the library, else the first executable.
    let primary = lib.or_else(|| exes.first().copied())?;
    let is_cpp = model.targets.iter().any(|t| t.language.as_deref() == Some("CXX"));
    let lang_key = if is_cpp { "cpp" } else { "c" };

    // Union defines + includes across every emitted target.
    let mut defines: Vec<String> = Vec::new();
    let mut includes: Vec<String> = Vec::new();
    for t in libs.iter().chain(exes.iter()) {
        for d in &t.defines {
            if !defines.contains(d) {
                defines.push(d.clone());
            }
        }
        for i in &t.includes {
            if !includes.contains(i) {
                includes.push(i.clone());
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "# Migrated from CMake (native) by `freight init --migrate --native`.\n\
         [package]\nname        = \"{name}\"\nversion     = \"0.1.0\"\ndescription = \"\"\nlicense     = \"MIT\"\n\
         # Source list is authoritative (extracted from CMake) — no src/ auto-walk.\n\
         auto-discover = false\n\n",
    ));

    if let Some(std) = primary.std.as_ref() {
        let prefix = if is_cpp { "c++" } else { "c" };
        out.push_str(&format!("[language.{lang_key}]\nstd = \"{prefix}{std}\"\n\n"));
    }

    if let Some(lib) = lib {
        if lib.sources.is_empty() {
            return None;
        }
        let lib_type = match lib.kind {
            TargetKind::SharedLib => "shared",
            _ => "static",
        };
        out.push_str(&format!("[lib]\ntype = \"{lib_type}\"\n"));
        out.push_str(&format!("srcs = [{}]\n\n", toml_str_list(&lib.sources)));
    }

    for exe in &exes {
        out.push_str(&format!(
            "[[bin]]\nname = \"{}\"\nsrc  = \"{}\"\n\n",
            exe.name,
            exe.sources[0].replace('\\', "/"),
        ));
    }

    if !defines.is_empty() || !includes.is_empty() {
        out.push_str("[compiler]\n");
        if !includes.is_empty() {
            out.push_str(&format!("includes = [{}]\n", toml_str_list(&includes)));
        }
        if !defines.is_empty() {
            out.push_str(&format!("defines  = [{}]\n", toml_str_list(&defines)));
        }
    }

    Some(out)
}

/// Render a list of strings as a TOML inline array body: `"a", "b"`.
fn toml_str_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('\\', "/")))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib_target(name: &str) -> CmakeTarget {
        CmakeTarget {
            name: name.into(),
            kind: TargetKind::StaticLib,
            sources: vec!["src/format.cc".into(), "src/os.cc".into()],
            defines: vec!["FMT_LOCALE".into()],
            includes: vec!["include".into()],
            std: Some("17".into()),
            language: Some("CXX".into()),
        }
    }

    #[test]
    fn single_library_renders_native() {
        let model = CmakeModel {
            targets: vec![lib_target("fmt")],
        };
        let toml = render_native_manifest("fmt", &model).expect("single lib → native");
        assert!(toml.contains("[lib]"));
        assert!(toml.contains("type = \"static\""));
        assert!(toml.contains("srcs = [\"src/format.cc\", \"src/os.cc\"]"));
        assert!(toml.contains("std = \"c++17\""));
        assert!(toml.contains("includes = [\"include\"]"));
        assert!(toml.contains("defines  = [\"FMT_LOCALE\"]"));
        // It parses as a valid manifest.
        crate::manifest::load_manifest_str(&toml).expect("renders valid toml");
    }

    fn exe_target(name: &str, src: &str) -> CmakeTarget {
        CmakeTarget {
            name: name.into(),
            kind: TargetKind::Executable,
            sources: vec![src.into()],
            defines: vec![],
            includes: vec![],
            std: Some("17".into()),
            language: Some("CXX".into()),
        }
    }

    #[test]
    fn library_plus_single_source_exe_emits_lib_and_bin() {
        let model = CmakeModel {
            targets: vec![lib_target("fmt"), exe_target("demo", "app/main.cc")],
        };
        let toml = render_native_manifest("fmt", &model).expect("lib + bin → native");
        assert!(toml.contains("[lib]"));
        assert!(toml.contains("[[bin]]\nname = \"demo\"\nsrc  = \"app/main.cc\""));
    }

    #[test]
    fn pure_application_emits_bin_only() {
        let model = CmakeModel {
            targets: vec![exe_target("app", "src/main.c")],
        };
        let toml = render_native_manifest("app", &model).expect("app → native bin");
        assert!(!toml.contains("[lib]"));
        assert!(toml.contains("[[bin]]\nname = \"app\"\nsrc  = \"src/main.c\""));
    }

    #[test]
    fn multi_source_executable_falls_back() {
        let mut exe = exe_target("app", "src/main.c");
        exe.sources.push("src/extra.c".into());
        let model = CmakeModel { targets: vec![exe] };
        assert!(render_native_manifest("x", &model).is_none());
    }

    #[test]
    fn multiple_libraries_fall_back() {
        let model = CmakeModel {
            targets: vec![lib_target("a"), lib_target("b")],
        };
        assert!(render_native_manifest("x", &model).is_none());
    }

    #[test]
    fn no_targets_falls_back() {
        let model = CmakeModel { targets: vec![] };
        assert!(render_native_manifest("x", &model).is_none());
    }
}
