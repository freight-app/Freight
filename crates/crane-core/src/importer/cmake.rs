//! CMakeLists.txt → [`ImportedProject`].
//!
//! v1 scope: flat projects only — a single top-level `CMakeLists.txt`. Nested
//! `add_subdirectory(...)` calls are recorded as notes but not recursed into.
//! The parser is a lightweight hand-rolled tokeniser over the CMake command
//! syntax (`name(args)`), which is adequate for the commands we care about and
//! avoids taking on a CMake-specific crate dependency.

use std::fs;
use std::path::Path;

use regex::Regex;

use super::{
    ImportedBin, ImportedDep, ImportedLib, ImportedProject,
};
use crate::error::CraneError;

pub fn parse(project_dir: &Path) -> Result<ImportedProject, CraneError> {
    let path = project_dir.join("CMakeLists.txt");
    let text = fs::read_to_string(&path)
        .map_err(|e| CraneError::ImporterParse(format!("reading {}: {e}", path.display())))?;
    Ok(parse_text(&text))
}

pub(crate) fn parse_text(text: &str) -> ImportedProject {
    let mut project = ImportedProject::default();

    let stripped = strip_comments(text);
    let calls = tokenize_calls(&stripped);

    for Call { name, args, line } in calls {
        match name.as_str() {
            "project" => handle_project(&mut project, &args),
            "set" => handle_set(&mut project, &args),
            "add_executable" => handle_add_executable(&mut project, &args, line),
            "add_library" => handle_add_library(&mut project, &args, line),
            "target_link_libraries" => handle_target_link_libraries(&mut project, &args),
            "target_include_directories" | "include_directories" => {
                handle_include_dirs(&mut project, &args);
            }
            "find_package" => handle_find_package(&mut project, &args),
            "add_compile_definitions" => handle_compile_definitions(&mut project, &args),
            "add_definitions" => handle_add_definitions(&mut project, &args),
            "add_compile_options" | "target_compile_options" => {
                handle_compile_options(&mut project, &name, &args);
            }
            "add_subdirectory" => {
                let sub = args.first().cloned().unwrap_or_default();
                project.push_note(format!(
                    "add_subdirectory({sub}) at line {line}: subdirectory not imported"
                ));
            }
            _ => {
                // Ignore commands we don't care about (cmake_minimum_required,
                // message, install, etc.). Unknown targets with obvious intent
                // could be logged here in future; for v1 we keep output tidy.
            }
        }
    }

    project
}

// ── Tokeniser ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Call {
    name: String,
    args: Vec<String>,
    line: usize,
}

/// Strip CMake line comments (`# …` to end of line) but preserve line count so
/// error messages can cite the original line number.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut in_string = false;
        let mut end = line.len();
        let bytes = line.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'"' => in_string = !in_string,
                b'#' if !in_string => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

fn tokenize_calls(text: &str) -> Vec<Call> {
    // name(  … ), allowing newlines inside the parens
    let head = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    let mut calls = Vec::new();

    for m in head.captures_iter(text) {
        let whole = m.get(0).unwrap();
        let name = m.get(1).unwrap().as_str().to_string();
        let open = whole.end();

        // Walk forward, balancing parens.
        let bytes = text.as_bytes();
        let mut depth = 1usize;
        let mut i = open;
        let mut in_string = false;
        while i < bytes.len() && depth > 0 {
            let c = bytes[i];
            match c {
                b'"' => in_string = !in_string,
                b'(' if !in_string => depth += 1,
                b')' if !in_string => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                break;
            }
            i += 1;
        }
        if depth != 0 {
            // Unmatched paren — skip.
            continue;
        }
        let inner = &text[open..i];
        let args = split_args(inner);
        let line = text[..whole.start()].bytes().filter(|&b| b == b'\n').count() + 1;
        calls.push(Call { name, args, line });
    }
    calls
}

/// Split CMake argument text into individual tokens, respecting double-quoted
/// strings. Nested variable expansions (`${X}`) are preserved as-is and the
/// caller may choose to strip or dereference them.
fn split_args(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_string = false;
    for c in input.chars() {
        match c {
            '"' => {
                in_string = !in_string;
                // Drop the quote characters; they're structural.
            }
            ' ' | '\t' | '\n' | '\r' if !in_string => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ── Command handlers ──────────────────────────────────────────────────────────

fn handle_project(p: &mut ImportedProject, args: &[String]) {
    // project(<name> [VERSION x.y.z] [DESCRIPTION "…"] [LANGUAGES CXX C …])
    if args.is_empty() {
        return;
    }
    p.name = Some(args[0].clone());

    let mut i = 1;
    while i < args.len() {
        match args[i].to_ascii_uppercase().as_str() {
            "VERSION" if i + 1 < args.len() => {
                p.version = Some(args[i + 1].clone());
                i += 2;
            }
            "DESCRIPTION" if i + 1 < args.len() => {
                p.description = Some(args[i + 1].clone());
                i += 2;
            }
            "LANGUAGES" => {
                i += 1;
                while i < args.len() && !is_project_keyword(&args[i]) {
                    if let Some(key) = cmake_lang_to_key(&args[i]) {
                        p.language_mut(key);
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    // If no LANGUAGES clause, CMake defaults to C and C++.
    if p.languages.is_empty() {
        p.language_mut("c");
        p.language_mut("cpp");
    }
}

fn is_project_keyword(s: &str) -> bool {
    matches!(
        s.to_ascii_uppercase().as_str(),
        "VERSION" | "DESCRIPTION" | "LANGUAGES" | "HOMEPAGE_URL"
    )
}

fn cmake_lang_to_key(name: &str) -> Option<&'static str> {
    match name.to_ascii_uppercase().as_str() {
        "CXX" | "C++" => Some("cpp"),
        "C" => Some("c"),
        "Fortran" | "FORTRAN" => Some("fortran"),
        "CUDA" => Some("cuda"),
        "HIP" => Some("hip"),
        _ => None,
    }
}

fn handle_set(p: &mut ImportedProject, args: &[String]) {
    // set(VAR value [CACHE …])
    let Some((var, rest)) = args.split_first() else { return };
    let Some(val) = rest.first() else { return };
    match var.as_str() {
        "CMAKE_CXX_STANDARD" => {
            p.language_mut("cpp").std = Some(format!("c++{val}"));
        }
        "CMAKE_C_STANDARD" => {
            p.language_mut("c").std = Some(format!("c{val}"));
        }
        _ => {}
    }
}

fn handle_add_executable(p: &mut ImportedProject, args: &[String], _line: usize) {
    // add_executable(<name> [WIN32] [MACOSX_BUNDLE] [EXCLUDE_FROM_ALL] src…)
    let Some((name, rest)) = args.split_first() else { return };
    let srcs: Vec<&String> = rest
        .iter()
        .filter(|a| !is_exe_keyword(a) && !a.starts_with("${"))
        .collect();
    if let Some(entry) = srcs.first() {
        p.bins.push(ImportedBin {
            name: name.clone(),
            src: (*entry).clone(),
        });
    }
}

fn is_exe_keyword(s: &str) -> bool {
    matches!(
        s.to_ascii_uppercase().as_str(),
        "WIN32" | "MACOSX_BUNDLE" | "EXCLUDE_FROM_ALL"
    )
}

fn handle_add_library(p: &mut ImportedProject, args: &[String], _line: usize) {
    // add_library(<name> [STATIC|SHARED|MODULE|INTERFACE|OBJECT] src…)
    let Some((_name, rest)) = args.split_first() else { return };
    let (lib_type, srcs_start) = match rest.first().map(String::as_str) {
        Some("STATIC") => ("static", 1),
        Some("SHARED" | "MODULE") => ("shared", 1),
        Some("INTERFACE") => ("header-only", 1),
        _ => ("static", 0),
    };
    let srcs: Vec<&String> = rest[srcs_start..]
        .iter()
        .filter(|a| !a.starts_with("${"))
        .collect();

    let src_dir = srcs
        .first()
        .map(|s| parent_dir_or_self(s.as_str()))
        .unwrap_or_else(|| "src/".to_string());

    p.lib = Some(ImportedLib {
        lib_type: lib_type.to_string(),
        src: src_dir,
        include: None,
    });
}

fn parent_dir_or_self(src: &str) -> String {
    match src.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/"),
        _ => "src/".to_string(),
    }
}

fn handle_target_link_libraries(p: &mut ImportedProject, args: &[String]) {
    // target_link_libraries(<target> [PUBLIC|PRIVATE|INTERFACE] lib…)
    let Some((_target, rest)) = args.split_first() else { return };
    for a in rest {
        if matches!(
            a.to_ascii_uppercase().as_str(),
            "PUBLIC" | "PRIVATE" | "INTERFACE"
        ) {
            continue;
        }
        if a.starts_with("${") {
            continue;
        }
        // Strip a leading "lib" prefix cosmetically for the crane key, but
        // keep the linker name intact as the system value.
        let key = a.trim_start_matches("lib").to_string();
        p.dependencies
            .entry(if key.is_empty() { a.clone() } else { key })
            .or_insert_with(|| ImportedDep::System(a.clone()));
    }
}

fn handle_include_dirs(p: &mut ImportedProject, args: &[String]) {
    for a in args {
        // Scope / order keywords are CMake structure, not paths.
        if matches!(
            a.to_ascii_uppercase().as_str(),
            "PUBLIC" | "PRIVATE" | "INTERFACE" | "BEFORE" | "AFTER" | "SYSTEM"
        ) {
            continue;
        }
        if a.starts_with("${") {
            continue;
        }
        // target_include_directories() starts with a target name; a bare token
        // with no slash and no dot is almost certainly the target — skip. We
        // still accept `include` / `src` as conventional include dir names.
        let looks_like_path = a.contains('/') || a.contains('.') || a == "include" || a == "src";
        if !looks_like_path {
            continue;
        }
        let norm = if a.ends_with('/') { a.clone() } else { format!("{a}/") };
        if !p.compiler.include_paths.iter().any(|x| x == &norm) {
            p.compiler.include_paths.push(norm);
        }
    }
}

fn handle_find_package(p: &mut ImportedProject, args: &[String]) {
    // find_package(Name [REQUIRED] [COMPONENTS …])
    let Some(name) = args.first() else { return };
    let key = name.to_ascii_lowercase();
    p.dependencies
        .entry(key.clone())
        .or_insert_with(|| ImportedDep::System(key));
    p.push_note(format!(
        "find_package({name}) mapped to system dep — verify the linker name matches your system library"
    ));
}

fn handle_compile_definitions(p: &mut ImportedProject, args: &[String]) {
    for a in args {
        if a.starts_with("${") {
            continue;
        }
        let clean = a.trim_start_matches("-D").to_string();
        if !clean.is_empty() && !p.compiler.defines.contains(&clean) {
            p.compiler.defines.push(clean);
        }
    }
}

fn handle_add_definitions(p: &mut ImportedProject, args: &[String]) {
    for a in args {
        if let Some(rest) = a.strip_prefix("-D") {
            if !rest.is_empty() && !p.compiler.defines.contains(&rest.to_string()) {
                p.compiler.defines.push(rest.to_string());
            }
        } else if a.starts_with('-') && !p.compiler.flags.contains(a) {
            p.compiler.flags.push(a.clone());
        }
    }
}

fn handle_compile_options(p: &mut ImportedProject, cmd: &str, args: &[String]) {
    // target_compile_options: first arg is the target — skip it.
    let start = if cmd == "target_compile_options" { 1 } else { 0 };
    for a in &args[start..] {
        if matches!(
            a.to_ascii_uppercase().as_str(),
            "PUBLIC" | "PRIVATE" | "INTERFACE" | "BEFORE"
        ) {
            continue;
        }
        if a.starts_with("${") {
            continue;
        }
        if a.starts_with('-') && !p.compiler.flags.contains(a) {
            p.compiler.flags.push(a.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_with_version_and_languages() {
        let src = r#"
            cmake_minimum_required(VERSION 3.10)
            project(hello VERSION 1.2.3 LANGUAGES CXX)
        "#;
        let p = parse_text(src);
        assert_eq!(p.name.as_deref(), Some("hello"));
        assert_eq!(p.version.as_deref(), Some("1.2.3"));
        assert!(p.languages.contains_key("cpp"));
        assert!(!p.languages.contains_key("c"));
    }

    #[test]
    fn parses_cxx_standard_set() {
        let src = "project(foo)\nset(CMAKE_CXX_STANDARD 20)\n";
        let p = parse_text(src);
        assert_eq!(p.language_mut("cpp").std.as_deref(), Some("c++20"));
    }

    #[test]
    fn extracts_executable_and_library() {
        let src = r#"
            project(p)
            add_executable(myapp src/main.cpp src/util.cpp)
            add_library(mylib STATIC src/a.cpp src/b.cpp)
        "#;
        let p = parse_text(src);
        assert_eq!(p.bins.len(), 1);
        assert_eq!(p.bins[0].name, "myapp");
        assert_eq!(p.bins[0].src, "src/main.cpp");
        let lib = p.lib.expect("expected a lib");
        assert_eq!(lib.lib_type, "static");
        assert_eq!(lib.src, "src/");
    }

    #[test]
    fn interface_library_is_header_only() {
        let src = "project(p)\nadd_library(hdr INTERFACE)\n";
        let p = parse_text(src);
        assert_eq!(p.lib.unwrap().lib_type, "header-only");
    }

    #[test]
    fn find_package_becomes_system_dep() {
        let src = "project(p)\nfind_package(OpenSSL REQUIRED)\n";
        let p = parse_text(src);
        assert!(matches!(
            p.dependencies.get("openssl"),
            Some(ImportedDep::System(s)) if s == "openssl"
        ));
    }

    #[test]
    fn target_link_libraries_collects_linker_names() {
        let src = r#"
            project(p)
            add_executable(app main.c)
            target_link_libraries(app PRIVATE m pthread)
        "#;
        let p = parse_text(src);
        assert!(p.dependencies.contains_key("m"));
        assert!(p.dependencies.contains_key("pthread"));
    }

    #[test]
    fn add_subdirectory_becomes_note() {
        let src = "project(p)\nadd_subdirectory(vendor/zlib)\n";
        let p = parse_text(src);
        assert!(p.notes.iter().any(|n| n.contains("add_subdirectory(vendor/zlib)")));
    }

    #[test]
    fn include_directories_captured() {
        let src = "project(p)\ninclude_directories(include third_party/include)\n";
        let p = parse_text(src);
        assert!(p.compiler.include_paths.contains(&"include/".to_string()));
        assert!(p.compiler.include_paths.contains(&"third_party/include/".to_string()));
    }

    #[test]
    fn add_definitions_parses_defines_and_flags() {
        let src = r#"
            project(p)
            add_definitions(-DUSE_FOO -DN=4 -Wall)
        "#;
        let p = parse_text(src);
        assert!(p.compiler.defines.contains(&"USE_FOO".to_string()));
        assert!(p.compiler.defines.contains(&"N=4".to_string()));
        assert!(p.compiler.flags.contains(&"-Wall".to_string()));
    }

    #[test]
    fn comments_are_stripped() {
        let src = "project(hello) # trailing\n# full line\nset(CMAKE_CXX_STANDARD 17)\n";
        let p = parse_text(src);
        assert_eq!(p.name.as_deref(), Some("hello"));
        assert_eq!(p.language_mut("cpp").std.as_deref(), Some("c++17"));
    }
}
