//! Deep migration of foreign build systems into freight manifests.
//!
//! Where the lightweight adoption path (`new::write_cmake_manifest`) delegates the
//! whole build back to CMake (`build = "cmake"`), this module *extracts the real
//! build data* — sources, defines, include dirs, language standard — from CMake's
//! [File API](https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html) and
//! renders a freight-**native** manifest from it.

pub mod cmake_fileapi;

pub use cmake_fileapi::{extract, CmakeModel, CmakeTarget, TargetKind};

/// Render a freight-native `freight.toml` from an extracted CMake model, for the
/// faithful case: a project that builds exactly one library (the common leaf-library
/// shape, e.g. fmt). Executable targets — typically the project's own tests and
/// examples — are ignored, since the migration deliverable is the library itself.
/// Returns `None` when there isn't exactly one library target, so the caller can
/// fall back to a `build = "cmake"` adoption.
pub fn render_native_manifest(name: &str, model: &CmakeModel) -> Option<String> {
    let libs: Vec<&CmakeTarget> = model
        .targets
        .iter()
        .filter(|t| matches!(t.kind, TargetKind::StaticLib | TargetKind::SharedLib))
        .collect();

    if libs.len() != 1 {
        return None;
    }
    let lib = libs[0];
    if lib.sources.is_empty() {
        return None;
    }

    let is_cpp = lib.language.as_deref() == Some("CXX");
    let lang_key = if is_cpp { "cpp" } else { "c" };
    let lib_type = match lib.kind {
        TargetKind::SharedLib => "shared",
        _ => "static",
    };

    let mut out = String::new();
    out.push_str(&format!(
        "# Migrated from CMake (native) by `freight init --migrate --native`.\n\
         [package]\nname        = \"{name}\"\nversion     = \"0.1.0\"\ndescription = \"\"\nlicense     = \"MIT\"\n\
         # Source list is authoritative (extracted from CMake) — no src/ auto-walk.\n\
         auto-discover = false\n\n",
    ));

    if let Some(std) = lib.std.as_ref() {
        let prefix = if is_cpp { "c++" } else { "c" };
        out.push_str(&format!("[language.{lang_key}]\nstd = \"{prefix}{std}\"\n\n"));
    }

    out.push_str(&format!("[lib]\ntype = \"{lib_type}\"\n"));
    out.push_str(&format!("srcs = [{}]\n", toml_str_list(&lib.sources)));

    if !lib.defines.is_empty() || !lib.includes.is_empty() {
        out.push_str("\n[compiler]\n");
        if !lib.includes.is_empty() {
            out.push_str(&format!("includes = [{}]\n", toml_str_list(&lib.includes)));
        }
        if !lib.defines.is_empty() {
            out.push_str(&format!("defines  = [{}]\n", toml_str_list(&lib.defines)));
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

    #[test]
    fn executable_targets_are_ignored() {
        // A single library plus test/example executables → still native (lib only).
        let mut exe = lib_target("tests");
        exe.kind = TargetKind::Executable;
        let model = CmakeModel {
            targets: vec![lib_target("fmt"), exe],
        };
        let toml = render_native_manifest("fmt", &model).expect("lib extracted, exe ignored");
        assert!(toml.contains("[lib]"));
    }

    #[test]
    fn multiple_libraries_fall_back() {
        let model = CmakeModel {
            targets: vec![lib_target("a"), lib_target("b")],
        };
        assert!(render_native_manifest("x", &model).is_none());
    }

    #[test]
    fn no_library_falls_back() {
        let model = CmakeModel { targets: vec![] };
        assert!(render_native_manifest("x", &model).is_none());
    }
}
