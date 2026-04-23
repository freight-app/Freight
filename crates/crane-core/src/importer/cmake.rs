//! CMakeLists.txt → [`ImportedProject`].
//!
//! v1 scope: flat projects only — a single top-level `CMakeLists.txt`. Nested
//! `add_subdirectory(...)` calls are recorded as notes but not recursed into.
//! The parser is a lightweight hand-rolled tokeniser over the CMake command
//! syntax (`name(args)`), which is adequate for the commands we care about and
//! avoids taking on a CMake-specific crate dependency.

use std::collections::HashMap;
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

    // User-defined variables seen via `set(VAR …)`. Used to expand `${VAR}` in
    // subsequent calls so that constructs like `add_executable(${TARGET} …)` or
    // `add_executable(app ${SRCS})` produce useful imports instead of being
    // silently dropped by the `${`-prefix filter downstream.
    let mut vars: HashMap<String, String> = HashMap::new();

    // Stack of active `if(…)` blocks so we can route platform-gated calls into
    // the right `[platform.X]` overlay and report the rest as notes.
    let mut if_stack: Vec<IfState> = Vec::new();

    for Call { name, args, line } in calls {
        match name.as_str() {
            "if" => {
                let state = classify_if(&args, line);
                if state.platform.is_none() {
                    project.push_note(format!(
                        "if(...) at line {line}: contents imported unconditionally — review for platform / option guards"
                    ));
                }
                if_stack.push(state);
                continue;
            }
            "endif" => {
                if_stack.pop();
                continue;
            }
            "elseif" | "else" => {
                // Best-effort: any branch other than the original `if(...)`
                // gets demoted to "no platform" so we don't misroute things.
                // The user is expected to review the resulting crane.toml.
                if let Some(top) = if_stack.last_mut() {
                    if top.platform.is_some() {
                        project.push_note(format!(
                            "{} branch at line {line} entered after {}: routed to base config",
                            name,
                            top.kind_label(),
                        ));
                        top.platform = None;
                    }
                }
                continue;
            }
            _ => {}
        }

        // Innermost active platform wins, mirroring nested-if semantics.
        let platform = if_stack.iter().rev().find_map(|s| s.platform.as_deref());

        let args = expand_args(&args, &vars);

        match name.as_str() {
            "project" => handle_project(&mut project, &args),
            "set" => handle_set(&mut project, &mut vars, &args),
            "add_executable" => handle_add_executable(&mut project, platform, &args, line),
            "add_library" => handle_add_library(&mut project, platform, &args, line),
            "target_link_libraries" => handle_target_link_libraries(&mut project, platform, &args),
            "target_include_directories" | "include_directories" => {
                handle_include_dirs(&mut project, platform, &args);
            }
            "find_package" => handle_find_package(&mut project, platform, &args),
            "add_compile_definitions" => handle_compile_definitions(&mut project, platform, &args),
            "add_definitions" => handle_add_definitions(&mut project, platform, &args),
            "add_compile_options" | "target_compile_options" => {
                handle_compile_options(&mut project, platform, &name, &args);
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

// ── Conditional handling ─────────────────────────────────────────────────────

#[derive(Debug)]
struct IfState {
    /// Crane platform name (`linux`, `windows`, `macos`, `unix`, …) when this
    /// `if(...)` was recognised as a platform guard, otherwise `None`.
    platform: Option<String>,
    /// Original token list, used for diagnostic output.
    raw: Vec<String>,
}

impl IfState {
    fn kind_label(&self) -> String {
        if let Some(p) = &self.platform {
            format!("platform-gated if({}) → {p}", self.raw.join(" "))
        } else {
            format!("if({})", self.raw.join(" "))
        }
    }
}

/// Recognise CMake's well-known platform predicates. Returns the matching
/// crane platform key when the if-condition is a single platform check; in
/// every other case (compound conditions, options, undefined behaviour) the
/// caller falls back to the unconditional-import path.
fn classify_if(args: &[String], _line: usize) -> IfState {
    let raw: Vec<String> = args.iter().cloned().collect();

    // CMake `if(WIN32)`, `if(LINUX)`, etc. — a single bare platform token.
    if args.len() == 1 {
        if let Some(plat) = bare_platform_token(&args[0]) {
            return IfState { platform: Some(plat.to_string()), raw };
        }
    }

    // CMake `if(CMAKE_SYSTEM_NAME STREQUAL "Linux")` — three-token form.
    if args.len() == 3 && args[0].eq_ignore_ascii_case("CMAKE_SYSTEM_NAME")
        && args[1].eq_ignore_ascii_case("STREQUAL")
    {
        if let Some(plat) = system_name_token(&args[2]) {
            return IfState { platform: Some(plat.to_string()), raw };
        }
    }

    IfState { platform: None, raw }
}

fn bare_platform_token(tok: &str) -> Option<&'static str> {
    // Match the predicate variables CMake sets per platform / toolchain.
    // MSVC and MINGW are toolchain markers but always imply Windows in
    // practice, so they map to the windows overlay.
    match tok.to_ascii_uppercase().as_str() {
        "WIN32" | "MSVC" | "MINGW" | "WINDOWS" | "CYGWIN" => Some("windows"),
        "APPLE" => Some("macos"),
        "UNIX" => Some("unix"),
        "LINUX" => Some("linux"),
        "BSD" => Some("bsd"),
        "FREEBSD" => Some("freebsd"),
        "OPENBSD" => Some("openbsd"),
        "NETBSD" => Some("netbsd"),
        "ANDROID" => Some("android"),
        "IOS" => Some("ios"),
        _ => None,
    }
}

fn system_name_token(tok: &str) -> Option<&'static str> {
    let v = tok.trim_matches('"').to_ascii_lowercase();
    match v.as_str() {
        "linux" => Some("linux"),
        "windows" => Some("windows"),
        "darwin" | "macos" => Some("macos"),
        "freebsd" => Some("freebsd"),
        "openbsd" => Some("openbsd"),
        "netbsd" => Some("netbsd"),
        "android" => Some("android"),
        "ios" => Some("ios"),
        _ => None,
    }
}

/// Expand `${VAR}` references in each argument using `vars`. If the expansion
/// of a single argument yields whitespace-separated tokens (the common case for
/// `set(SRCS a.cpp b.cpp); add_executable(app ${SRCS})`), the result is split
/// so each token becomes its own entry. Unknown vars are left as-is so the
/// downstream `${`-prefix filters in handlers continue to drop them.
fn expand_args(args: &[String], vars: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let expanded = expand_str(a, vars);
        // No whitespace introduced: keep as a single token, even if empty —
        // dropping empty tokens matches the original split_args behaviour.
        if !expanded.contains(char::is_whitespace) {
            if !expanded.is_empty() {
                out.push(expanded);
            }
            continue;
        }
        for tok in expanded.split_whitespace() {
            out.push(tok.to_string());
        }
    }
    out
}

fn expand_str(input: &str, vars: &HashMap<String, String>) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end_rel) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let name = &input[i + 2..i + 2 + end_rel];
                let advance = 2 + end_rel + 1;
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&input[i..i + advance]),
                }
                i += advance;
                continue;
            }
        }
        // ASCII-safe to push by byte since `${` boundaries are ASCII; for
        // multi-byte chars we still walk one byte at a time but only push them
        // through the char path.
        let c = input[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
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

fn handle_set(p: &mut ImportedProject, vars: &mut HashMap<String, String>, args: &[String]) {
    // set(VAR value… [CACHE TYPE "doc" [FORCE]] [PARENT_SCOPE])
    let Some((var, rest)) = args.split_first() else { return };

    // Collect values up to the first CMake `set()` keyword.
    let value_end = rest
        .iter()
        .position(|t| matches!(t.as_str(), "CACHE" | "PARENT_SCOPE" | "FORCE"))
        .unwrap_or(rest.len());
    let values = &rest[..value_end];

    if let Some(first) = values.first() {
        match var.as_str() {
            "CMAKE_CXX_STANDARD" => {
                p.language_mut("cpp").std = Some(format!("c++{first}"));
            }
            "CMAKE_C_STANDARD" => {
                p.language_mut("c").std = Some(format!("c{first}"));
            }
            _ => {}
        }
    }

    // Always record the variable so subsequent ${VAR} references can resolve.
    // Multi-token values are joined with spaces; expand_args splits them back
    // out into separate args at use sites.
    vars.insert(var.clone(), values.join(" "));
}

fn handle_add_executable(p: &mut ImportedProject, platform: Option<&str>, args: &[String], line: usize) {
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
        if let Some(plat) = platform {
            p.push_note(format!(
                "add_executable({name}) at line {line} was inside if({plat}) — emitted at top level; remove or guard manually for non-{plat} builds"
            ));
        }
    }
}

fn is_exe_keyword(s: &str) -> bool {
    matches!(
        s.to_ascii_uppercase().as_str(),
        "WIN32" | "MACOSX_BUNDLE" | "EXCLUDE_FROM_ALL"
    )
}

fn handle_add_library(p: &mut ImportedProject, platform: Option<&str>, args: &[String], line: usize) {
    // add_library(<name> [STATIC|SHARED|MODULE|INTERFACE|OBJECT] src…)
    let Some((name, rest)) = args.split_first() else { return };
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

    if let Some(plat) = platform {
        p.push_note(format!(
            "add_library({name}) at line {line} was inside if({plat}) — emitted at top level; review for non-{plat} builds"
        ));
    }
}

fn parent_dir_or_self(src: &str) -> String {
    match src.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/"),
        _ => "src/".to_string(),
    }
}

fn handle_target_link_libraries(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
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
        let dep_key = if key.is_empty() { a.clone() } else { key };
        p.add_dep(platform, dep_key, ImportedDep::System(a.clone()));
    }
}

fn handle_include_dirs(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
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
        p.add_include_path(platform, norm);
    }
}

fn handle_find_package(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
    // find_package(Name [REQUIRED] [COMPONENTS …])
    let Some(name) = args.first() else { return };
    let key = name.to_ascii_lowercase();
    p.add_dep(platform, key.clone(), ImportedDep::System(key));
    p.push_note(format!(
        "find_package({name}) mapped to system dep — verify the linker name matches your system library"
    ));
}

fn handle_compile_definitions(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
    for a in args {
        if a.starts_with("${") {
            continue;
        }
        let clean = a.trim_start_matches("-D").to_string();
        if !clean.is_empty() {
            p.add_define(platform, clean);
        }
    }
}

fn handle_add_definitions(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
    for a in args {
        if let Some(rest) = a.strip_prefix("-D") {
            if !rest.is_empty() {
                p.add_define(platform, rest.to_string());
            }
        } else if a.starts_with('-') {
            p.add_flag(platform, a.clone());
        }
    }
}

fn handle_compile_options(p: &mut ImportedProject, platform: Option<&str>, cmd: &str, args: &[String]) {
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
        if a.starts_with('-') {
            p.add_flag(platform, a.clone());
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
        assert_eq!(p.languages.get("cpp").and_then(|l| l.std.as_deref()), Some("c++20"));
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
        assert_eq!(p.languages.get("cpp").and_then(|l| l.std.as_deref()), Some("c++17"));
    }

    #[test]
    fn variable_expansion_resolves_target_name() {
        let src = r#"
            project(p)
            set(TARGET_NAME app)
            add_executable(${TARGET_NAME} src/main.cpp)
        "#;
        let p = parse_text(src);
        assert_eq!(p.bins.len(), 1);
        assert_eq!(p.bins[0].name, "app");
        assert_eq!(p.bins[0].src, "src/main.cpp");
    }

    #[test]
    fn variable_expansion_splits_source_lists() {
        let src = r#"
            project(p)
            set(SRCS src/main.cpp src/util.cpp)
            add_executable(app ${SRCS})
        "#;
        let p = parse_text(src);
        assert_eq!(p.bins.len(), 1);
        assert_eq!(p.bins[0].name, "app");
        assert_eq!(p.bins[0].src, "src/main.cpp");
    }

    #[test]
    fn variable_expansion_chains_through_other_sets() {
        let src = r#"
            project(p)
            set(BASE include)
            set(DIRS ${BASE}/core ${BASE}/util)
            include_directories(${DIRS})
        "#;
        let p = parse_text(src);
        assert!(p.compiler.include_paths.contains(&"include/core/".to_string()));
        assert!(p.compiler.include_paths.contains(&"include/util/".to_string()));
    }

    #[test]
    fn unknown_vars_are_dropped_by_filter() {
        // ${UNDEFINED} stays literal; the existing `${`-prefix filter in
        // handle_target_link_libraries skips it so we don't emit a bogus dep.
        let src = r#"
            project(p)
            add_executable(app main.c)
            target_link_libraries(app PRIVATE ${UNDEFINED} m)
        "#;
        let p = parse_text(src);
        assert!(p.dependencies.contains_key("m"));
        assert_eq!(p.dependencies.len(), 1);
    }

    #[test]
    fn cache_keyword_terminates_value_collection() {
        let src = r#"
            project(p)
            set(MY_OPT ON CACHE BOOL "Enable foo" FORCE)
            add_executable(app main.c)
            target_link_libraries(app PRIVATE ${MY_OPT})
        "#;
        let p = parse_text(src);
        // ${MY_OPT} should expand to "ON", not "ON CACHE BOOL …".
        // (And "ON" itself becomes a system dep, which is the user's problem
        // to review, but at least it's not "BOOL" / "FORCE" garbage.)
        assert!(p.dependencies.contains_key("ON"));
        assert!(!p.dependencies.contains_key("BOOL"));
        assert!(!p.dependencies.contains_key("FORCE"));
    }

    #[test]
    fn multiple_executables_produce_multiple_bins() {
        let src = r#"
            project(p)
            add_executable(app1 src/main1.cpp)
            add_executable(app2 src/main2.cpp)
        "#;
        let p = parse_text(src);
        assert_eq!(p.bins.len(), 2);
        assert_eq!(p.bins[0].name, "app1");
        assert_eq!(p.bins[1].name, "app2");
    }

    #[test]
    fn unrecognized_if_blocks_are_flagged_as_notes() {
        let src = r#"
            project(p)
            if(SOME_USER_OPTION)
              add_executable(app src/main.cpp)
            endif()
        "#;
        let p = parse_text(src);
        // Contents of the if-block are still imported (we can't evaluate
        // arbitrary options), but a note records that the import was unconditional.
        assert_eq!(p.bins.len(), 1);
        assert!(p.notes.iter().any(|n| n.contains("if(...)") && n.contains("unconditionally")));
    }

    #[test]
    fn win32_if_routes_into_windows_platform_overlay() {
        let src = r#"
            project(p)
            add_executable(app src/main.c)
            if(WIN32)
              target_link_libraries(app PRIVATE ws2_32)
              add_definitions(-DWIN_BUILD)
              include_directories(third_party/win-include)
            endif()
        "#;
        let p = parse_text(src);
        let win = p.platforms.get("windows").expect("expected windows overlay");
        assert!(win.dependencies.contains_key("ws2_32"));
        assert!(win.defines.contains(&"WIN_BUILD".to_string()));
        assert!(win.include_paths.contains(&"third_party/win-include/".to_string()));
        // Base config should NOT have these.
        assert!(!p.dependencies.contains_key("ws2_32"));
        assert!(!p.compiler.defines.contains(&"WIN_BUILD".to_string()));
        // Recognised platform guards do NOT emit the "unconditionally" note.
        assert!(!p.notes.iter().any(|n| n.contains("unconditionally")));
    }

    #[test]
    fn unix_if_routes_into_unix_overlay() {
        let src = r#"
            project(p)
            add_executable(app src/main.c)
            if(UNIX)
              target_link_libraries(app PRIVATE pthread dl)
            endif()
        "#;
        let p = parse_text(src);
        let unix = p.platforms.get("unix").expect("expected unix overlay");
        assert!(unix.dependencies.contains_key("pthread"));
        assert!(unix.dependencies.contains_key("dl"));
    }

    #[test]
    fn apple_if_routes_into_macos_overlay() {
        let src = r#"
            project(p)
            add_executable(app src/main.c)
            if(APPLE)
              target_link_libraries(app PRIVATE c++abi)
            endif()
        "#;
        let p = parse_text(src);
        assert!(p.platforms.contains_key("macos"));
    }

    #[test]
    fn cmake_system_name_strequal_routes_correctly() {
        let src = r#"
            project(p)
            add_executable(app src/main.c)
            if(CMAKE_SYSTEM_NAME STREQUAL "FreeBSD")
              target_link_libraries(app PRIVATE execinfo)
            endif()
        "#;
        let p = parse_text(src);
        assert!(p.platforms.contains_key("freebsd"));
        assert!(
            p.platforms["freebsd"].dependencies.contains_key("execinfo")
        );
    }

    #[test]
    fn binaries_inside_platform_if_get_a_review_note() {
        let src = r#"
            project(p)
            if(WIN32)
              add_executable(winapp src/win.cpp)
            endif()
        "#;
        let p = parse_text(src);
        // Bin is still emitted (it can't be platform-overlayed in v1) but
        // a note tells the user to review.
        assert_eq!(p.bins.len(), 1);
        assert_eq!(p.bins[0].name, "winapp");
        assert!(p.notes.iter().any(|n|
            n.contains("add_executable(winapp)") && n.contains("if(windows)")
        ));
    }
}
