//! CMakeLists.txt → [`ImportedProject`] using the `cmake-parser` crate.
//!
//! v1 scope: flat projects only — a single top-level `CMakeLists.txt`. Nested
//! `add_subdirectory(...)` calls are recorded as notes but not recursed into.
//!
//! The scripting commands (set, if, find_package, …) are handled via the
//! cmake-parser typed API; the project commands (add_executable, add_library,
//! target_link_libraries, …) use the crate's lossless tokeniser but fall back
//! to Debug-based token extraction for their inner enum types, which are kept
//! in private sub-modules inside cmake-parser.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use cmake_parser::command::scripting::{find_package::FindPackage, set::Set};
use cmake_parser::{parse_cmakelists, Command, Doc};

use crate::{ImportedBin, ImportedDep, ImportedLib, ImportedProject};
use crane_core::error::CraneError;

pub fn parse(project_dir: &Path) -> Result<ImportedProject, CraneError> {
    let path = project_dir.join("CMakeLists.txt");
    let text = fs::read_to_string(&path)
        .map_err(|e| CraneError::ImporterParse(format!("reading {}: {e}", path.display())))?;
    Ok(parse_text(&text))
}

pub(crate) fn parse_text(text: &str) -> ImportedProject {
    let mut project = ImportedProject::default();

    // cmake-parser 0.1.0-beta.1 does not handle inline comments such as
    // `project(hello) # note` — the entire line is silently dropped. Strip
    // them ourselves before handing the text to the parser.
    let cleaned = strip_inline_comments(text);

    let tokens = match parse_cmakelists(cleaned.as_bytes()) {
        Ok(t) => t,
        Err(_) => return project,
    };
    let doc = Doc::from(tokens);

    // User-defined variables seen via `set(VAR …)`. Used to expand `${VAR}` in
    // subsequent calls so that constructs like `add_executable(${TARGET} …)` or
    // `add_executable(app ${SRCS})` produce useful imports instead of being
    // silently dropped by the `${`-prefix filter downstream.
    let mut vars: HashMap<String, String> = HashMap::new();

    // Stack of active `if(…)` blocks so we can route platform-gated calls into
    // the right `[platform.X]` overlay and report the rest as notes.
    let mut if_stack: Vec<IfState> = Vec::new();

    for result in doc.to_commands_iter() {
        let cmd = match result {
            Ok(c) => c,
            Err(_) => continue, // skip unknown/unparseable commands
        };

        match cmd {
            Command::If(c) => {
                let cond_toks: Vec<String> =
                    c.condition.conditions.iter().map(|t| t.to_string()).collect();
                let state = classify_if(&cond_toks);
                if state.platform.is_none() {
                    let cond_str = cond_toks.join(" ");
                    project.push_note(format!(
                        "if({cond_str}): contents imported unconditionally — review for platform / option guards"
                    ));
                }
                if_stack.push(state);
                continue;
            }
            Command::EndIf(_) => {
                if_stack.pop();
                continue;
            }
            Command::ElseIf(c) => {
                if let Some(top) = if_stack.last_mut() {
                    if top.platform.is_some() {
                        let cond_str = c
                            .condition
                            .conditions
                            .iter()
                            .map(|t| t.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        project.push_note(format!(
                            "elseif({cond_str}) branch entered after {}: routed to base config",
                            top.kind_label(),
                        ));
                        top.platform = None;
                    }
                }
                continue;
            }
            Command::Else(_) => {
                if let Some(top) = if_stack.last_mut() {
                    if top.platform.is_some() {
                        project.push_note(format!(
                            "else branch entered after {}: routed to base config",
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

        match cmd {
            // ── project() ─────────────────────────────────────────────────
            Command::Project(p) => {
                let mut args = vec![p.project_name.to_string()];
                let d = format!("{:?}", p.details);
                args.extend(project_args_from_debug(&d));
                let args = expand_args(&args, &vars);
                handle_project(&mut project, &args);
            }

            // ── set() ─────────────────────────────────────────────────────
            Command::Set(s) => {
                let (var, raw_values) = match *s {
                    Set::Normal(n) => (
                        n.variable.to_string(),
                        n.value.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
                    ),
                    Set::Cache(c) => (
                        c.variable.to_string(),
                        c.value.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
                    ),
                };
                let mut args = vec![var];
                // Expand at assignment time so that chained sets like
                // `set(A x); set(B ${A}/y)` store the resolved value for B.
                args.extend(expand_args(&raw_values, &vars));
                handle_set(&mut project, &mut vars, &args);
            }

            // ── add_executable() ──────────────────────────────────────────
            Command::AddExecutable(a) => {
                let mut flat = vec![a.name.to_string()];
                // a.executable is of a private type; extract source tokens via Debug.
                flat.extend(debug_to_tokens(&format!("{:?}", a.executable)));
                let args = expand_args(&flat, &vars);
                handle_add_executable(&mut project, platform, &args);
            }

            // ── add_library() ─────────────────────────────────────────────
            Command::AddLibrary(a) => {
                let lib_debug = format!("{:?}", a.library);
                // Extract the library type keyword before reconstructing the flat arg list.
                let lib_type_kw = library_type_keyword(&lib_debug);
                let mut flat = vec![a.name.to_string()];
                if let Some(kw) = lib_type_kw {
                    flat.push(kw.to_string());
                }
                flat.extend(debug_to_tokens(&lib_debug));
                let args = expand_args(&flat, &vars);
                handle_add_library(&mut project, platform, &args);
            }

            // ── target_link_libraries() ───────────────────────────────────
            Command::TargetLinkLibraries(t) => {
                let flat = debug_to_tokens(&format!("{t:?}"));
                let args = expand_args(&flat, &vars);
                handle_target_link_libraries(&mut project, platform, &args);
            }

            // ── target_include_directories() ──────────────────────────────
            Command::TargetIncludeDirectories(t) => {
                let mut flat = vec![t.target.to_string()];
                flat.extend(debug_to_tokens(&format!("{:?}", t.directories)));
                let args = expand_args(&flat, &vars);
                handle_include_dirs(&mut project, platform, &args);
            }

            // ── include_directories() ─────────────────────────────────────
            Command::IncludeDirectories(i) => {
                let flat: Vec<String> = i.dirs.iter().map(|t| t.to_string()).collect();
                let args = expand_args(&flat, &vars);
                handle_include_dirs(&mut project, platform, &args);
            }

            // ── find_package() ────────────────────────────────────────────
            Command::FindPackage(f) => {
                let name = match *f {
                    FindPackage::Basic(b) => b.package_name.to_string(),
                    FindPackage::Full(f) => f.package_name.to_string(),
                };
                let args = expand_args(&[name], &vars);
                handle_find_package(&mut project, platform, &args);
            }

            // ── add_compile_definitions() ─────────────────────────────────
            Command::AddCompileDefinitions(a) => {
                let flat: Vec<String> =
                    a.compile_definitions.iter().map(|t| t.to_string()).collect();
                let args = expand_args(&flat, &vars);
                handle_compile_definitions(&mut project, platform, &args);
            }

            // ── target_compile_definitions() ─────────────────────────────
            Command::TargetCompileDefinitions(t) => {
                // Skip first token (target name), extract definition tokens.
                let debug = format!("{:?}", t.definitions);
                let flat = debug_to_tokens(&debug);
                let args = expand_args(&flat, &vars);
                handle_compile_definitions(&mut project, platform, &args);
            }

            // ── add_definitions() ─────────────────────────────────────────
            Command::AddDefinitions(a) => {
                let flat: Vec<String> = a.definitions.iter().map(|t| t.to_string()).collect();
                let args = expand_args(&flat, &vars);
                handle_add_definitions(&mut project, platform, &args);
            }

            // ── add_compile_options() ─────────────────────────────────────
            Command::AddCompileOptions(a) => {
                let flat: Vec<String> =
                    a.compile_options.iter().map(|t| t.to_string()).collect();
                let args = expand_args(&flat, &vars);
                handle_compile_options(&mut project, platform, false, &args);
            }

            // ── target_compile_options() ──────────────────────────────────
            Command::TargetCompileOptions(t) => {
                // target is public Token; options inner type is private.
                let mut flat = vec![t.target.to_string()];
                flat.extend(debug_to_tokens(&format!("{:?}", t.options)));
                let args = expand_args(&flat, &vars);
                handle_compile_options(&mut project, platform, true, &args);
            }

            // ── target_compile_features() ─────────────────────────────────
            Command::TargetCompileFeatures(t) => {
                // Extract cxx_std_XX / c_std_XX feature tokens to set language
                // standards — the rest of the feature list is silently ignored.
                for tok in debug_to_tokens(&format!("{:?}", t.features)) {
                    if let Some(ver) = tok.strip_prefix("cxx_std_") {
                        project.language_mut("cpp").std = Some(format!("c++{ver}"));
                    } else if let Some(ver) = tok.strip_prefix("c_std_") {
                        project.language_mut("c").std = Some(format!("c{ver}"));
                    }
                }
            }

            // ── configure_file() ──────────────────────────────────────────
            Command::ConfigureFile(_) => {
                project.push_note(
                    "configure_file(): not imported — recreate in a build.crane script"
                        .to_string(),
                );
            }

            // ── add_subdirectory() ────────────────────────────────────────
            Command::AddSubdirectory(s) => {
                let sub = extract_add_subdirectory_source(&s);
                project.push_note(format!(
                    "add_subdirectory({sub}): subdirectory not imported"
                ));
            }

            _ => {}
        }
    }

    project
}

// ── Conditional handling ─────────────────────────────────────────────────────

#[derive(Debug)]
struct IfState {
    platform: Option<String>,
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

fn classify_if(args: &[String]) -> IfState {
    let raw: Vec<String> = args.iter().cloned().collect();

    if args.len() == 1 {
        if let Some(plat) = bare_platform_token(&args[0]) {
            return IfState { platform: Some(plat.to_string()), raw };
        }
    }

    if args.len() == 3
        && args[0].eq_ignore_ascii_case("CMAKE_SYSTEM_NAME")
        && args[1].eq_ignore_ascii_case("STREQUAL")
    {
        if let Some(plat) = system_name_token(&args[2]) {
            return IfState { platform: Some(plat.to_string()), raw };
        }
    }

    IfState { platform: None, raw }
}

fn bare_platform_token(tok: &str) -> Option<&'static str> {
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

// ── Debug extraction helpers ──────────────────────────────────────────────────

/// Extract all `Token(value)` strings from a `Debug`-formatted cmake-parser struct.
///
/// cmake-parser's inner enum types (e.g. `Executable`, `Library`, `Directory`) live in
/// private sub-modules and cannot be imported. Their Debug implementations consistently
/// format every CMake token value as `Token(value)` (unquoted) or `Token("value")`
/// (quoted), so we can recover the original argument list without naming the types.
fn debug_to_tokens(debug_str: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut s = debug_str;
    while let Some(idx) = s.find("Token(") {
        s = &s[idx + 6..]; // skip "Token("
        let (tok, rest) = if s.starts_with('"') {
            // Quoted token: Token("some value")
            let inner = &s[1..];
            match inner.find('"') {
                Some(end) => (inner[..end].to_string(), &inner[end + 1..]),
                None => break,
            }
        } else {
            // Unquoted token: Token(foo/bar.cpp)
            // Tokens are path-like strings that won't contain ')' themselves.
            match s.find(')') {
                Some(end) => (s[..end].to_string(), &s[end + 1..]),
                None => break,
            }
        };
        s = rest;
        if !tok.is_empty() {
            tokens.push(tok);
        }
    }
    tokens
}

/// Reconstruct the keyword-bearing portion of `project()` arguments from the
/// Debug representation of `Option<ProjectDetails>`.
///
/// Returns the args that should come *after* the project name:
/// `["VERSION", "1.2.3", "DESCRIPTION", "...", "LANGUAGES", "CXX", "C"]`
fn project_args_from_debug(details_debug: &str) -> Vec<String> {
    let mut args = Vec::new();

    if details_debug.contains("General(") {
        // GeneralProjectDetails has named fields; extract each one by key.
        if let Some(after) = details_debug.split("version: Some(Token(").nth(1) {
            if let Some(end) = after.find(')') {
                let v = after[..end].trim_matches('"');
                if !v.is_empty() {
                    args.push("VERSION".to_string());
                    args.push(v.to_string());
                }
            }
        }
        if let Some(after) = details_debug.split("description: Some(Token(").nth(1) {
            if let Some(end) = after.find(')') {
                let d = after[..end].trim_matches('"');
                if !d.is_empty() {
                    args.push("DESCRIPTION".to_string());
                    args.push(d.to_string());
                }
            }
        }
        if let Some(after) = details_debug.split("languages: Some([").nth(1) {
            let lang_section = after.split("])").next().unwrap_or("");
            let langs = debug_to_tokens(lang_section);
            if !langs.is_empty() {
                args.push("LANGUAGES".to_string());
                args.extend(langs);
            }
        }
    } else if details_debug.contains("Short(") {
        // Old-style: project(name LANG1 LANG2) — treat all tokens as language list.
        let toks = debug_to_tokens(details_debug);
        if !toks.is_empty() {
            args.push("LANGUAGES".to_string());
            args.extend(toks);
        }
    }

    args
}

/// Infer the CMake library type keyword from the Debug representation of a
/// cmake-parser `Library` enum value.
fn library_type_keyword(lib_debug: &str) -> Option<&'static str> {
    // The Debug output of the library enum variants contains variant names like:
    //   "Normal(NormalLibrary { library_type: Some(Shared), ... })"
    //   "Normal(NormalLibrary { library_type: Some(Static), ... })"
    //   "Interface(InterfaceLibrary { ... })"
    //   "Object(ObjectLibrary { ... })"
    if lib_debug.contains("library_type: Some(Shared)")
        || lib_debug.contains("library_type: Some(Module)")
    {
        Some("SHARED")
    } else if lib_debug.contains("library_type: Some(Static)") {
        Some("STATIC")
    } else if lib_debug.starts_with("Interface(") {
        Some("INTERFACE")
    } else if lib_debug.starts_with("Object(") {
        Some("OBJECT")
    } else {
        None
    }
}

/// Extract the `source_dir` from an `AddSubdirectory` node.
///
/// `AddSubdirectory::source_dir` is private in cmake-parser, so we fall back
/// to the Debug representation, which is stable within a crate version.
fn extract_add_subdirectory_source(
    s: &cmake_parser::command::project::AddSubdirectory<'_>,
) -> String {
    // Debug output looks like:
    //   AddSubdirectory { source_dir: Token(vendor/zlib), binary_dir: None, ... }
    // or with a quoted path:
    //   AddSubdirectory { source_dir: Token("some path"), binary_dir: None, ... }
    let debug = format!("{s:?}");
    if let Some((_, rest)) = debug.split_once("source_dir: Token(") {
        if let Some(stripped) = rest.strip_prefix('"') {
            return stripped.split('"').next().unwrap_or("").to_string();
        } else {
            return rest.split(')').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

// ── Comment stripping ─────────────────────────────────────────────────────────

/// Strip inline `#` comments from CMake source before passing to cmake-parser.
///
/// cmake-parser 0.1.0-beta.1 silently drops any command line that contains a
/// trailing `# comment`, so we remove them ourselves. Full-line comments
/// (`# ...` with optional leading whitespace) are replaced by blank lines to
/// preserve line numbers for other diagnostics.
fn strip_inline_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let bytes = line.as_bytes();
        let mut in_quote = false;
        let mut comment_pos = None;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => in_quote = !in_quote,
                b'\\' if in_quote && i + 1 < bytes.len() => i += 1,
                b'#' if !in_quote => {
                    comment_pos = Some(i);
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        if let Some(pos) = comment_pos {
            out.push_str(line[..pos].trim_end());
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

// ── Variable expansion ────────────────────────────────────────────────────────

/// Expand `${VAR}` references in each argument using `vars`. If the expansion
/// of a single argument yields whitespace-separated tokens (the common case for
/// `set(SRCS a.cpp b.cpp); add_executable(app ${SRCS})`), the result is split
/// so each token becomes its own entry. Unknown vars are left as-is so the
/// downstream `${`-prefix filters in handlers continue to drop them.
fn expand_args(args: &[String], vars: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let expanded = expand_str(a, vars);
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
        let c = input[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

// ── Command handlers ──────────────────────────────────────────────────────────

fn handle_project(p: &mut ImportedProject, args: &[String]) {
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
        "FORTRAN" => Some("fortran"),
        "CUDA" => Some("cuda"),
        "HIP" => Some("hip"),
        _ => None,
    }
}

fn handle_set(p: &mut ImportedProject, vars: &mut HashMap<String, String>, args: &[String]) {
    let Some((var, values)) = args.split_first() else { return };

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

    vars.insert(var.clone(), values.join(" "));
}

fn handle_add_executable(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
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
                "add_executable({name}) was inside if({plat}) — emitted at top level; remove or guard manually for non-{plat} builds"
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

fn handle_add_library(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
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

    p.libs.push(ImportedLib {
        name: name.clone(),
        lib_type: lib_type.to_string(),
        src: src_dir,
        include: None,
    });

    if let Some(plat) = platform {
        p.push_note(format!(
            "add_library({name}) was inside if({plat}) — emitted at top level; review for non-{plat} builds"
        ));
    }
}

fn parent_dir_or_self(src: &str) -> String {
    match src.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/"),
        _ => "src/".to_string(),
    }
}

fn handle_target_link_libraries(
    p: &mut ImportedProject,
    platform: Option<&str>,
    args: &[String],
) {
    let Some((_target, rest)) = args.split_first() else { return };
    for a in rest {
        if matches!(
            a.to_ascii_uppercase().as_str(),
            "PUBLIC" | "PRIVATE" | "INTERFACE"
        ) {
            continue;
        }
        // Skip variable references and generator expressions.
        if a.starts_with("${") || a.starts_with("$<") {
            continue;
        }
        let (dep_key, linker_name) = normalize_link_token(a);
        p.add_dep(platform, dep_key, ImportedDep::System(linker_name));
    }
}

/// Normalize a CMake link token to `(dep_key, linker_name)`.
///
/// CMake imported targets use `Namespace::Component` syntax. We map these to
/// the actual linker flag name (`-l{linker_name}`) that the system library
/// requires. For plain library names (e.g. `m`, `pthread`, `libz`), we strip
/// the `lib` prefix and lowercase.
fn normalize_link_token(a: &str) -> (String, String) {
    if let Some((_ns, component)) = a.split_once("::") {
        let linker = imported_target_linker_name(a, component);
        (linker.clone(), linker)
    } else {
        // Strip leading "lib" prefix (e.g. "libz" → "z") then lowercase.
        let stripped = a.strip_prefix("lib").unwrap_or(a);
        let name = if stripped.is_empty() { a } else { stripped };
        let lower = name.to_ascii_lowercase();
        (lower.clone(), lower)
    }
}

/// Map a CMake imported target to its actual system linker name (`-l{name}`).
///
/// For well-known packages the mapping is hardcoded; everything else falls back
/// to lowercasing the component after `::`. Boost components get the
/// `boost_` prefix because Boost libraries are installed as `libboost_<name>`.
fn imported_target_linker_name(full: &str, component: &str) -> String {
    match full {
        // Threading
        "Threads::Threads" => "pthread".to_string(),
        // Compression
        "ZLIB::ZLIB" | "zlib::zlib" => "z".to_string(),
        "BZip2::BZip2" => "bz2".to_string(),
        "LibLZMA::LibLZMA" => "lzma".to_string(),
        "zstd::libzstd_shared" | "zstd::libzstd_static" => "zstd".to_string(),
        // TLS / crypto
        "OpenSSL::SSL" => "ssl".to_string(),
        "OpenSSL::Crypto" => "crypto".to_string(),
        // Image formats
        "PNG::PNG" => "png".to_string(),
        "JPEG::JPEG" => "jpeg".to_string(),
        "TIFF::TIFF" => "tiff".to_string(),
        // Network
        "CURL::libcurl" => "curl".to_string(),
        // XML / JSON
        "LibXml2::LibXml2" => "xml2".to_string(),
        // Math
        "BLAS::BLAS" => "blas".to_string(),
        "LAPACK::LAPACK" => "lapack".to_string(),
        // Boost: Boost::filesystem → boost_filesystem
        _ if full.starts_with("Boost::") => {
            format!("boost_{}", component.to_ascii_lowercase())
        }
        // Generic fallback: lowercase the component name
        _ => component.to_ascii_lowercase(),
    }
}

fn handle_include_dirs(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
    for a in args {
        if matches!(
            a.to_ascii_uppercase().as_str(),
            "PUBLIC" | "PRIVATE" | "INTERFACE" | "BEFORE" | "AFTER" | "SYSTEM"
        ) {
            continue;
        }
        if a.starts_with("${") {
            continue;
        }
        let looks_like_path = a.contains('/') || a.contains('.') || a == "include" || a == "src";
        if !looks_like_path {
            continue;
        }
        let norm = if a.ends_with('/') { a.clone() } else { format!("{a}/") };
        p.add_include_path(platform, norm);
    }
}

fn handle_find_package(p: &mut ImportedProject, platform: Option<&str>, args: &[String]) {
    let Some(name) = args.first() else { return };
    let key = name.to_ascii_lowercase();
    p.add_dep(platform, key.clone(), ImportedDep::System(key));
    p.push_note(format!(
        "find_package({name}) mapped to system dep — verify the linker name matches your system library"
    ));
}

fn handle_compile_definitions(
    p: &mut ImportedProject,
    platform: Option<&str>,
    args: &[String],
) {
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

fn handle_compile_options(
    p: &mut ImportedProject,
    platform: Option<&str>,
    is_target_variant: bool,
    args: &[String],
) {
    let start = if is_target_variant { 1 } else { 0 };
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
        let lib = p.libs.first().expect("expected a lib");
        assert_eq!(lib.lib_type, "static");
        assert_eq!(lib.src, "src/");
    }

    #[test]
    fn interface_library_is_header_only() {
        let src = "project(p)\nadd_library(hdr INTERFACE)\n";
        let p = parse_text(src);
        assert_eq!(p.libs.first().unwrap().lib_type, "header-only");
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
    fn imported_targets_are_normalized_to_linker_names() {
        let src = r#"
            project(p)
            find_package(OpenSSL REQUIRED)
            find_package(Threads REQUIRED)
            find_package(ZLIB REQUIRED)
            add_executable(app main.cpp)
            target_link_libraries(app PRIVATE OpenSSL::SSL OpenSSL::Crypto Threads::Threads ZLIB::ZLIB Boost::filesystem)
        "#;
        let p = parse_text(src);
        assert!(matches!(p.dependencies.get("ssl"), Some(ImportedDep::System(s)) if s == "ssl"));
        assert!(matches!(p.dependencies.get("crypto"), Some(ImportedDep::System(s)) if s == "crypto"));
        assert!(matches!(p.dependencies.get("pthread"), Some(ImportedDep::System(s)) if s == "pthread"));
        assert!(matches!(p.dependencies.get("z"), Some(ImportedDep::System(s)) if s == "z"));
        assert!(matches!(p.dependencies.get("boost_filesystem"), Some(ImportedDep::System(s)) if s == "boost_filesystem"));
    }

    #[test]
    fn multiple_add_library_produces_workspace_members() {
        let src = r#"
            project(p)
            add_library(mylib STATIC src/a.cpp)
            add_library(another SHARED src/b.cpp)
        "#;
        let p = parse_text(src);
        assert_eq!(p.libs.len(), 2);
        assert_eq!(p.libs[0].name, "mylib");
        assert_eq!(p.libs[1].name, "another");
        assert_eq!(p.libs[1].lib_type, "shared");
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
        // normalize_link_token lowercases plain lib names; "ON" → "on"
        assert!(p.dependencies.contains_key("on"));
        assert!(!p.dependencies.contains_key("bool"));
        assert!(!p.dependencies.contains_key("force"));
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
        assert_eq!(p.bins.len(), 1);
        assert!(p.notes.iter().any(|n| n.contains("unconditionally")));
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
        assert!(!p.dependencies.contains_key("ws2_32"));
        assert!(!p.compiler.defines.contains(&"WIN_BUILD".to_string()));
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
        assert!(p.platforms["freebsd"].dependencies.contains_key("execinfo"));
    }

    #[test]
    fn target_compile_features_sets_language_standard() {
        let src = r#"
            project(p)
            add_executable(app src/main.cpp)
            target_compile_features(app PRIVATE cxx_std_20)
        "#;
        let p = parse_text(src);
        assert_eq!(p.languages.get("cpp").and_then(|l| l.std.as_deref()), Some("c++20"));
    }

    #[test]
    fn configure_file_emits_note() {
        let src = "project(p)\nconfigure_file(config.h.in config.h)\n";
        let p = parse_text(src);
        assert!(p.notes.iter().any(|n| n.contains("configure_file")));
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
        assert_eq!(p.bins.len(), 1);
        assert_eq!(p.bins[0].name, "winapp");
        assert!(p.notes.iter().any(|n|
            n.contains("add_executable(winapp)") && n.contains("if(windows)")
        ));
    }
}
