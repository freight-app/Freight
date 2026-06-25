//! Deep migration of foreign build systems into freight manifests.
//!
//! Where the lightweight adoption path (`new::write_cmake_manifest`) delegates the
//! whole build back to CMake (`build = "cmake"`), this module *extracts the real
//! build data* — sources, defines, include dirs, language standard — from CMake's
//! [File API](https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html) and
//! renders a freight-**native** manifest from it.

pub mod cmake_fileapi;

use std::collections::BTreeSet;
use std::path::Path;

pub use cmake_fileapi::{extract, CmakeModel, CmakeTarget, TargetKind};

/// The set of source files freight's `src/` walk would compile, as project-relative
/// paths: every file under `src/` whose extension matches one the model's targets
/// actually compile (e.g. `{"cc"}` for fmt). A native migration uses this so it can
/// rely on the zero-config walk and list only the *differences* — `!` negations for
/// files CMake doesn't build (a module unit), and plain additions for sources outside
/// `src/`.
pub fn walk_source_set(project_dir: &Path, model: &CmakeModel) -> BTreeSet<String> {
    let exts: BTreeSet<String> = model
        .targets
        .iter()
        .flat_map(|t| t.sources.iter())
        .filter_map(|s| {
            Path::new(s)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
        })
        .collect();
    let src_dir = project_dir.join("src");
    let mut out = BTreeSet::new();
    if exts.is_empty() || !src_dir.is_dir() {
        return out;
    }
    for entry in walkdir::WalkDir::new(&src_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let matches = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| exts.contains(&e.to_ascii_lowercase()))
            .unwrap_or(false);
        if matches {
            if let Ok(rel) = path.strip_prefix(project_dir) {
                out.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

/// Render a freight-native `freight.toml` from an extracted CMake model, as a single
/// freight package: up to one library (`[lib]`) plus any number of executables
/// (`[[bin]]`, which auto-link the library). Defines / include dirs are unioned
/// across targets into `[compiler]`, and the standard is taken from the library (or
/// the first executable). Returns `None` — so the caller falls back to a
/// `build = "cmake"` adoption — when the shape can't be represented faithfully in one
/// package: more than one library (needs a workspace), or any executable built from
/// more than one source (`[[bin]]` carries a single entry source).
pub fn render_native_manifest(
    name: &str,
    model: &CmakeModel,
    walk_set: &BTreeSet<String>,
) -> Option<String> {
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

    // Collapse static/shared variants of the *same* library (identical source set)
    // into one — many projects build both, e.g. zlib's `zlib` + `zlibstatic`. A
    // static variant is preferred as the representative. Genuinely distinct
    // libraries (different sources) remain separate → workspace territory → fall back.
    let mut src_sets: Vec<BTreeSet<String>> = Vec::new();
    let mut reps: Vec<&CmakeTarget> = Vec::new();
    for l in &libs {
        let key: BTreeSet<String> = l.sources.iter().cloned().collect();
        match src_sets.iter().position(|s| *s == key) {
            Some(i) => {
                if l.kind == TargetKind::StaticLib {
                    reps[i] = l;
                }
            }
            None => {
                src_sets.push(key);
                reps.push(l);
            }
        }
    }

    // > 1 distinct library → workspace territory → fall back.
    if reps.len() > 1 {
        return None;
    }
    let lib = reps.first().copied();

    // When a library is present, executables are its tools / examples / tests — not
    // the migration deliverable — so they're ignored (real-world example tools like
    // zlib's minigzip break native builds and aren't wanted). A *pure application*
    // (no library) migrates its single-source executables instead.
    let emit_exes: Vec<&CmakeTarget> = if lib.is_some() {
        Vec::new()
    } else {
        exes.clone()
    };
    if lib.is_none() && (emit_exes.is_empty() || emit_exes.iter().any(|e| e.sources.len() != 1)) {
        return None;
    }

    let primary = lib.or_else(|| emit_exes.first().copied())?;
    let is_cpp = model.targets.iter().any(|t| t.language.as_deref() == Some("CXX"));
    let lang_key = if is_cpp { "cpp" } else { "c" };

    // Targets whose flags/sources we actually emit (the library, or the app's exes).
    let emitted: Vec<&CmakeTarget> = lib.into_iter().chain(emit_exes.iter().copied()).collect();

    // Every source actually compiled by an emitted target. Files the `src/` walk
    // would pick up but that no emitted target compiles (e.g. a module unit, or an
    // ignored tool's source) are "extras" to negate with `!`, so we keep the
    // zero-config walk and only spell out the diff.
    let compiled: BTreeSet<String> = emitted
        .iter()
        .flat_map(|t| t.sources.iter().cloned())
        .collect();
    let extras: Vec<String> = walk_set.difference(&compiled).cloned().collect();
    // Negations have to live in `[lib].srcs`; with no library there's nowhere to put
    // them, so fall back to the cmake self-build for that (rare) shape.
    if lib.is_none() && !extras.is_empty() {
        return None;
    }

    // Union defines + includes across the emitted targets.
    let mut defines: Vec<String> = Vec::new();
    let mut includes: Vec<String> = Vec::new();
    for t in &emitted {
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
         [package]\nname        = \"{name}\"\nversion     = \"0.1.0\"\ndescription = \"\"\nlicense     = \"MIT\"\n\n",
    ));

    if let Some(std) = primary.std.as_ref() {
        let prefix = if is_cpp { "c++" } else { "c" };
        out.push_str(&format!("[language.{lang_key}]\nstd = \"{prefix}{std}\"\n\n"));
    }

    if let Some(lib) = lib {
        let lib_type = match lib.kind {
            TargetKind::SharedLib => "shared",
            _ => "static",
        };
        out.push_str(&format!("[lib]\ntype = \"{lib_type}\"\n"));
        // srcs lists only the differences from the walk: library sources the walk
        // wouldn't find (outside `src/`) as plain entries, plus `!`-negated extras.
        let mut srcs: Vec<String> = lib
            .sources
            .iter()
            .filter(|s| !walk_set.contains(*s))
            .cloned()
            .collect();
        srcs.extend(extras.iter().map(|e| format!("!{e}")));
        if !srcs.is_empty() {
            out.push_str(&format!("srcs = [{}]\n", toml_str_list(&srcs)));
        }
        out.push('\n');
    }

    for exe in &emit_exes {
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

    fn walk(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn walk_matches_so_no_srcs_listed() {
        // The walk would compile exactly the library's sources → no `srcs` at all.
        let model = CmakeModel {
            targets: vec![lib_target("fmt")],
        };
        let w = walk(&["src/format.cc", "src/os.cc"]);
        let toml = render_native_manifest("fmt", &model, &w).expect("single lib → native");
        assert!(toml.contains("[lib]"));
        assert!(toml.contains("type = \"static\""));
        assert!(!toml.contains("srcs ="), "walk matches → no srcs:\n{toml}");
        assert!(!toml.contains("auto-discover"), "no flag needed:\n{toml}");
        assert!(toml.contains("std = \"c++17\""));
        assert!(toml.contains("includes = [\"include\"]"));
        assert!(toml.contains("defines  = [\"FMT_LOCALE\"]"));
        crate::manifest::load_manifest_str(&toml).expect("renders valid toml");
    }

    #[test]
    fn extra_walked_file_is_negated() {
        // The walk also finds src/fmt.cc (a module unit no target compiles) → negate.
        let model = CmakeModel {
            targets: vec![lib_target("fmt")],
        };
        let w = walk(&["src/format.cc", "src/os.cc", "src/fmt.cc"]);
        let toml = render_native_manifest("fmt", &model, &w).expect("native");
        assert!(toml.contains("srcs = [\"!src/fmt.cc\"]"), "{toml}");
        crate::manifest::load_manifest_str(&toml).expect("valid toml");
    }

    #[test]
    fn library_source_outside_walk_is_listed_plainly() {
        // A library source the walk wouldn't find (outside src/) is added explicitly.
        let mut lib = lib_target("x");
        lib.sources = vec!["vendor/extra.c".into()];
        lib.language = Some("C".into());
        let model = CmakeModel { targets: vec![lib] };
        let toml = render_native_manifest("x", &model, &BTreeSet::new()).expect("native");
        assert!(toml.contains("srcs = [\"vendor/extra.c\"]"), "{toml}");
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
    fn library_present_so_executables_are_ignored() {
        // A library + an example/tool executable → migrate the library only.
        let model = CmakeModel {
            targets: vec![lib_target("fmt"), exe_target("demo", "app/main.cc")],
        };
        let w = walk(&["src/format.cc", "src/os.cc"]);
        let toml = render_native_manifest("fmt", &model, &w).expect("lib → native");
        assert!(toml.contains("[lib]"));
        assert!(!toml.contains("[[bin]]"), "exe should be ignored when a lib exists:\n{toml}");
    }

    #[test]
    fn pure_application_emits_bin_only() {
        let model = CmakeModel {
            targets: vec![exe_target("app", "src/main.c")],
        };
        let w = walk(&["src/main.c"]);
        let toml = render_native_manifest("app", &model, &w).expect("app → native bin");
        assert!(!toml.contains("[lib]"));
        assert!(toml.contains("[[bin]]\nname = \"app\"\nsrc  = \"src/main.c\""));
    }

    #[test]
    fn pure_app_with_extra_walked_file_falls_back() {
        // No library to host the negation for the stray src/ file → fall back.
        let model = CmakeModel {
            targets: vec![exe_target("app", "src/main.c")],
        };
        let w = walk(&["src/main.c", "src/orphan.c"]);
        assert!(render_native_manifest("app", &model, &w).is_none());
    }

    #[test]
    fn multi_source_executable_falls_back() {
        let mut exe = exe_target("app", "src/main.c");
        exe.sources.push("src/extra.c".into());
        let model = CmakeModel { targets: vec![exe] };
        assert!(render_native_manifest("x", &model, &BTreeSet::new()).is_none());
    }

    #[test]
    fn static_and_shared_variants_collapse_to_one() {
        // Same sources, two targets (e.g. zlib + zlibstatic) → one native [lib].
        let mut shared = lib_target("z");
        shared.kind = TargetKind::SharedLib;
        let mut stat = lib_target("zstatic");
        stat.kind = TargetKind::StaticLib;
        let model = CmakeModel {
            targets: vec![shared, stat],
        };
        let w = walk(&["src/format.cc", "src/os.cc"]);
        let toml = render_native_manifest("z", &model, &w).expect("variants collapse → native");
        assert!(toml.contains("[lib]"));
        // The static variant is the representative.
        assert!(toml.contains("type = \"static\""), "{toml}");
    }

    #[test]
    fn genuinely_distinct_libraries_fall_back() {
        let mut a = lib_target("a");
        a.sources = vec!["src/a.c".into()];
        let mut b = lib_target("b");
        b.sources = vec!["src/b.c".into()];
        let model = CmakeModel { targets: vec![a, b] };
        assert!(render_native_manifest("x", &model, &BTreeSet::new()).is_none());
    }

    #[test]
    fn no_targets_falls_back() {
        let model = CmakeModel { targets: vec![] };
        assert!(render_native_manifest("x", &model, &BTreeSet::new()).is_none());
    }
}
