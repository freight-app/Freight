//! meson.build → [`ImportedProject`].
//!
//! Meson's build DSL is a real (dynamic) scripting language with conditionals,
//! loops and custom functions. A full parser would be a large dependency for
//! very little win — in practice new projects arrive with fairly vanilla
//! `project()` / `executable()` / `library()` / `dependency()` calls. This
//! importer scans for those call sites with a small regex-based tokenizer and
//! records anything else it doesn't recognise as a `# CRANE:` note.

use std::fs;
use std::path::Path;

use regex::Regex;

use crate::{ImportedBin, ImportedDep, ImportedLib, ImportedProject};
use crane_core::error::CraneError;

pub fn parse(project_dir: &Path) -> Result<ImportedProject, CraneError> {
    let path = project_dir.join("meson.build");
    let text = fs::read_to_string(&path)
        .map_err(|e| CraneError::ImporterParse(format!("reading {}: {e}", path.display())))?;

    let dir_name = project_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("imported")
        .to_string();

    Ok(parse_text(&text, &dir_name))
}

pub(crate) fn parse_text(text: &str, default_name: &str) -> ImportedProject {
    let mut project = ImportedProject::default();
    project.name = Some(default_name.to_string());

    let stripped = strip_comments(text);
    let calls = collect_calls(&stripped);

    for c in &calls {
        match c.name.as_str() {
            "project" => handle_project(&mut project, &c.positional, &c.named),
            "executable" => handle_executable(&mut project, &c.positional),
            "library" => handle_library(&mut project, &c.positional, "static"),
            "static_library" => handle_library(&mut project, &c.positional, "static"),
            "shared_library" => handle_library(&mut project, &c.positional, "shared"),
            "both_libraries" => handle_library(&mut project, &c.positional, "static"),
            "dependency" => handle_dependency(&mut project, &c.positional),
            "include_directories" => handle_include_dirs(&mut project, &c.positional),
            "add_project_arguments" | "add_global_arguments" => {
                handle_add_arguments(&mut project, &c.positional);
            }
            "subdir" => {
                if let Some(dir) = c.positional.first() {
                    project.push_note(format!(
                        "subdir('{dir}'): subdirectory not imported — recurse manually"
                    ));
                }
            }
            _ => {}
        }
    }

    project
}

// ── Tokeniser ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct MesonCall {
    name: String,
    positional: Vec<String>,
    /// Named args as `(key, value)` pairs; values are already unquoted.
    named: Vec<(String, String)>,
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut in_string: Option<char> = None;
        let mut end = line.len();
        for (i, c) in line.char_indices() {
            match c {
                '\'' | '"' => {
                    if let Some(q) = in_string {
                        if q == c {
                            in_string = None;
                        }
                    } else {
                        in_string = Some(c);
                    }
                }
                '#' if in_string.is_none() => {
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

fn collect_calls(text: &str) -> Vec<MesonCall> {
    let head = Regex::new(r"(?m)(?:^|[^A-Za-z0-9_.])([a-z_][a-z_0-9]*)\s*\(").unwrap();
    let mut out = Vec::new();
    for caps in head.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let name = caps.get(1).unwrap().as_str().to_string();

        // We only care about a small allow-list — bail early to keep output clean.
        if !is_interesting(&name) {
            continue;
        }

        // Locate the `(` that opened this call.
        let open = text[whole.start()..whole.end()].rfind('(').unwrap() + whole.start() + 1;

        let Some(close) = match_paren(text, open - 1) else { continue };
        let inner = &text[open..close];
        let (positional, named) = split_call_args(inner);
        out.push(MesonCall {
            name,
            positional,
            named,
        });
    }
    out
}

fn is_interesting(name: &str) -> bool {
    matches!(
        name,
        "project"
            | "executable"
            | "library"
            | "static_library"
            | "shared_library"
            | "both_libraries"
            | "dependency"
            | "include_directories"
            | "add_project_arguments"
            | "add_global_arguments"
            | "subdir"
    )
}

/// Given an index pointing at `(`, return the index of the matching `)`.
fn match_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 1usize;
    let mut i = open + 1;
    let mut in_string: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match (c, in_string) {
            (b'\'', None) => in_string = Some(b'\''),
            (b'"', None) => in_string = Some(b'"'),
            (q, Some(sq)) if q == sq => in_string = None,
            (b'(', None) => depth += 1,
            (b')', None) => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split call arguments on top-level commas, respecting quoted strings and
/// nested brackets / parens / braces. Produces `(positional, named)`.
fn split_call_args(input: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut positional = Vec::new();
    let mut named = Vec::new();

    for raw in split_top_level_commas(input) {
        let arg = raw.trim();
        if arg.is_empty() {
            continue;
        }
        // Named arg? Look for `key :` at the top level.
        if let Some((k, v)) = split_named(arg) {
            named.push((k.to_string(), strip_quotes(v.trim()).to_string()));
        } else if let Some(list) = strip_list_brackets(arg) {
            // Expand list literals into positional tokens.
            for item in split_top_level_commas(list) {
                let t = strip_quotes(item.trim()).to_string();
                if !t.is_empty() {
                    positional.push(t);
                }
            }
        } else {
            positional.push(strip_quotes(arg).to_string());
        }
    }

    (positional, named)
}

fn split_top_level_commas(input: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut start = 0;
    for (i, c) in input.char_indices() {
        match c {
            '\'' | '"' => {
                if let Some(q) = in_string {
                    if q == c {
                        in_string = None;
                    }
                } else {
                    in_string = Some(c);
                }
            }
            '(' | '[' | '{' if in_string.is_none() => depth += 1,
            ')' | ']' | '}' if in_string.is_none() => depth -= 1,
            ',' if in_string.is_none() && depth == 0 => {
                out.push(&input[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start <= input.len() {
        out.push(&input[start..]);
    }
    out
}

fn split_named(arg: &str) -> Option<(&str, &str)> {
    // Find the first `:` at the top level, that isn't `::`.
    let mut in_string: Option<char> = None;
    for (i, c) in arg.char_indices() {
        match c {
            '\'' | '"' => {
                if let Some(q) = in_string {
                    if q == c {
                        in_string = None;
                    }
                } else {
                    in_string = Some(c);
                }
            }
            ':' if in_string.is_none() => {
                let key = arg[..i].trim();
                if key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !key.is_empty()
                {
                    return Some((key, &arg[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_list_brackets(arg: &str) -> Option<&str> {
    let t = arg.trim();
    if t.starts_with('[') && t.ends_with(']') {
        Some(&t[1..t.len() - 1])
    } else {
        None
    }
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    if (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
        || (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
    {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

// ── Command handlers ──────────────────────────────────────────────────────────

fn handle_project(
    p: &mut ImportedProject,
    positional: &[String],
    named: &[(String, String)],
) {
    if let Some(name) = positional.first() {
        p.name = Some(name.clone());
    }
    // Remaining positional args are languages (strings or, less commonly, a list
    // that was already flattened).
    for lang in positional.iter().skip(1) {
        if let Some(key) = meson_lang_to_key(lang) {
            p.language_mut(key);
        }
    }

    for (k, v) in named {
        match k.as_str() {
            "version" => p.version = Some(v.clone()),
            "default_options" => {
                // "cpp_std=c++20" / "c_std=c17" come in via default_options,
                // typically wrapped in a list literal: ['cpp_std=c++20', ...].
                let inner = strip_list_brackets(v).unwrap_or(v.as_str());
                for opt in split_top_level_commas(inner) {
                    let opt = strip_quotes(opt.trim());
                    if let Some((lhs, rhs)) = opt.split_once('=') {
                        let rhs = strip_quotes(rhs.trim());
                        match lhs.trim() {
                            "cpp_std" => p.language_mut("cpp").std = Some(rhs.to_string()),
                            "c_std" => p.language_mut("c").std = Some(rhs.to_string()),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if p.languages.is_empty() {
        // Meson requires at least one language in project(); default to C++ if
        // we couldn't figure it out.
        p.language_mut("cpp");
    }
}

fn meson_lang_to_key(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "cpp" | "c++" => Some("cpp"),
        "c" => Some("c"),
        "fortran" => Some("fortran"),
        "cuda" => Some("cuda"),
        _ => None,
    }
}

fn handle_executable(p: &mut ImportedProject, positional: &[String]) {
    let Some(name) = positional.first() else { return };
    let src = positional
        .iter()
        .skip(1)
        .find(|s| looks_like_source(s))
        .cloned()
        .unwrap_or_else(|| "src/main.cpp".to_string());
    p.bins.push(ImportedBin {
        name: name.clone(),
        src,
    });
}

fn handle_library(p: &mut ImportedProject, positional: &[String], default_type: &str) {
    let Some(name) = positional.first() else { return };
    let srcs: Vec<&String> = positional.iter().skip(1).filter(|s| looks_like_source(s)).collect();
    let src_dir = srcs
        .first()
        .map(|s| parent_dir_or_self(s))
        .unwrap_or_else(|| "src/".to_string());

    if p.libs.is_empty() {
        p.libs.push(ImportedLib {
            name: name.clone(),
            lib_type: default_type.to_string(),
            src: src_dir,
            include: None,
        });
    } else {
        p.push_note(format!(
            "library('{name}') — multiple libraries found; consider splitting into a workspace (only the first library is emitted as [lib])"
        ));
    }
}

fn parent_dir_or_self(src: &str) -> String {
    match src.rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => format!("{parent}/"),
        _ => "src/".to_string(),
    }
}

fn looks_like_source(s: &str) -> bool {
    [
        ".cpp", ".cc", ".cxx", ".c++", ".c", ".f90", ".f95", ".for", ".cu",
    ]
    .iter()
    .any(|ext| s.ends_with(ext))
}

fn handle_dependency(p: &mut ImportedProject, positional: &[String]) {
    let Some(name) = positional.first() else { return };
    let key = name.to_ascii_lowercase();
    p.dependencies
        .entry(key.clone())
        .or_insert_with(|| ImportedDep::System(key));
}

fn handle_include_dirs(p: &mut ImportedProject, positional: &[String]) {
    for raw in positional {
        if raw.is_empty() {
            continue;
        }
        let norm = if raw.ends_with('/') {
            raw.clone()
        } else {
            format!("{raw}/")
        };
        if !p.compiler.include_paths.iter().any(|x| x == &norm) {
            p.compiler.include_paths.push(norm);
        }
    }
}

fn handle_add_arguments(p: &mut ImportedProject, positional: &[String]) {
    for tok in positional {
        if let Some(rest) = tok.strip_prefix("-D") {
            if !rest.is_empty() && !p.compiler.defines.contains(&rest.to_string()) {
                p.compiler.defines.push(rest.to_string());
            }
        } else if tok.starts_with('-') && !p.compiler.flags.contains(tok) {
            p.compiler.flags.push(tok.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_name_version_language() {
        let src = "project('hello', 'cpp', version: '1.2.3')\n";
        let p = parse_text(src, "fallback");
        assert_eq!(p.name.as_deref(), Some("hello"));
        assert_eq!(p.version.as_deref(), Some("1.2.3"));
        assert!(p.languages.contains_key("cpp"));
    }

    #[test]
    fn project_language_list_is_expanded() {
        let src = "project('mix', ['c', 'cpp'])\n";
        let p = parse_text(src, "fallback");
        assert!(p.languages.contains_key("c"));
        assert!(p.languages.contains_key("cpp"));
    }

    #[test]
    fn default_options_pulls_out_stds() {
        let src = "project('x', 'cpp', default_options: ['cpp_std=c++20', 'c_std=c17'])\n";
        let p = parse_text(src, "fallback");
        assert_eq!(p.languages.get("cpp").and_then(|l| l.std.as_deref()), Some("c++20"));
        assert_eq!(p.languages.get("c").and_then(|l| l.std.as_deref()), Some("c17"));
    }

    #[test]
    fn executable_and_dependency() {
        let src = "\
project('x', 'cpp')
exe = executable('app', 'src/main.cpp', 'src/util.cpp')
ssl = dependency('openssl')
";
        let p = parse_text(src, "fallback");
        assert_eq!(p.bins.len(), 1);
        assert_eq!(p.bins[0].name, "app");
        assert_eq!(p.bins[0].src, "src/main.cpp");
        assert!(matches!(
            p.dependencies.get("openssl"),
            Some(ImportedDep::System(s)) if s == "openssl"
        ));
    }

    #[test]
    fn shared_library_marks_lib_type() {
        let src = "\
project('x', 'cpp')
shared_library('foo', 'src/a.cpp', 'src/b.cpp')
";
        let p = parse_text(src, "fallback");
        let lib = p.libs.first().expect("expected a lib");
        assert_eq!(lib.lib_type, "shared");
        assert_eq!(lib.src, "src/");
    }

    #[test]
    fn include_directories_and_defines() {
        let src = "\
project('x', 'cpp')
include_directories('include', 'third_party/include')
add_project_arguments('-DFOO', '-DBAR=2', '-Wall', language: 'cpp')
";
        let p = parse_text(src, "fallback");
        assert!(p.compiler.include_paths.contains(&"include/".to_string()));
        assert!(p.compiler.include_paths.contains(&"third_party/include/".to_string()));
        assert!(p.compiler.defines.contains(&"FOO".to_string()));
        assert!(p.compiler.defines.contains(&"BAR=2".to_string()));
        assert!(p.compiler.flags.contains(&"-Wall".to_string()));
    }

    #[test]
    fn multiple_executables_produce_multiple_bins() {
        let src = "\
project('p', 'cpp')
executable('app1', 'src/main1.cpp')
executable('app2', 'src/main2.cpp')
";
        let p = parse_text(src, "fallback");
        assert_eq!(p.bins.len(), 2);
        assert_eq!(p.bins[0].name, "app1");
        assert_eq!(p.bins[1].name, "app2");
    }

    #[test]
    fn comments_are_stripped() {
        let src = "\
# top comment
project('x', 'cpp')  # side comment
";
        let p = parse_text(src, "fallback");
        assert_eq!(p.name.as_deref(), Some("x"));
    }
}
