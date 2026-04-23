//! Makefile → [`ImportedProject`].
//!
//! Makefiles are ad-hoc: there is no single canonical shape. This importer
//! pulls out the pieces that map cleanly to crane (compiler variables, flag
//! variables, source lists, the primary link target) and flags everything else
//! as a note so the user can review. Recipe bodies are ignored entirely —
//! trying to execute them would defeat the point of migrating off make.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::{ImportedBin, ImportedDep, ImportedProject};
use crate::error::CraneError;

pub fn parse(project_dir: &Path) -> Result<ImportedProject, CraneError> {
    let path = if project_dir.join("Makefile").is_file() {
        project_dir.join("Makefile")
    } else if project_dir.join("GNUmakefile").is_file() {
        project_dir.join("GNUmakefile")
    } else {
        return Err(CraneError::ImporterParse(format!(
            "no Makefile found in {}",
            project_dir.display()
        )));
    };

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

    let lines = join_continuations(text);
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut rule_heads: Vec<String> = Vec::new();

    for line in &lines {
        let trimmed = line.trim_start();
        // Skip comments and blank lines.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Skip recipe lines — they start with a tab in the original, which
        // `join_continuations` preserves at the start.
        if line.starts_with('\t') {
            continue;
        }
        // Skip directives we don't interpret.
        if starts_with_directive(trimmed) {
            project.push_note(format!("make directive not imported: `{}`", trimmed.trim_end()));
            continue;
        }

        if let Some((var, op, rhs)) = parse_assignment(trimmed) {
            let expanded = expand(&vars, rhs.trim());
            match op {
                "+=" => {
                    let cur = vars.entry(var.to_string()).or_default();
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(&expanded);
                }
                _ => {
                    vars.insert(var.to_string(), expanded);
                }
            }
            continue;
        }

        if let Some(colon) = trimmed.find(':') {
            let head = trimmed[..colon].trim().to_string();
            if !head.is_empty() {
                rule_heads.push(head);
            }
        }
    }

    // ── Languages ──
    let has_cxxflags = vars.contains_key("CXXFLAGS") || vars.contains_key("CXX");
    let has_cflags = vars.contains_key("CFLAGS") || vars.contains_key("CC");
    let has_fflags = vars.contains_key("FFLAGS") || vars.contains_key("FC");

    if has_cxxflags {
        project.language_mut("cpp");
    }
    if has_cflags && !has_cxxflags {
        project.language_mut("c");
    } else if has_cflags && has_cxxflags {
        project.language_mut("c");
    }
    if has_fflags {
        project.language_mut("fortran");
    }
    if project.languages.is_empty() {
        // Default guess: treat as C.
        project.language_mut("c");
    }

    // ── Flag extraction ──
    let mut all_flags = String::new();
    for key in ["CFLAGS", "CXXFLAGS", "FFLAGS", "CPPFLAGS"] {
        if let Some(v) = vars.get(key) {
            if !all_flags.is_empty() {
                all_flags.push(' ');
            }
            all_flags.push_str(v);
        }
    }
    extract_flags(&mut project, &all_flags);

    // ── System deps from LDLIBS / LDFLAGS ──
    let mut link_text = String::new();
    for key in ["LDLIBS", "LDFLAGS", "LIBS"] {
        if let Some(v) = vars.get(key) {
            if !link_text.is_empty() {
                link_text.push(' ');
            }
            link_text.push_str(v);
        }
    }
    for token in link_text.split_whitespace() {
        if let Some(name) = token.strip_prefix("-l") {
            if !name.is_empty() {
                project
                    .dependencies
                    .entry(name.to_string())
                    .or_insert_with(|| ImportedDep::System(name.to_string()));
            }
        }
    }

    // ── Binary target ──
    let bin_name = vars
        .get("TARGET")
        .or_else(|| vars.get("PROGRAM"))
        .or_else(|| vars.get("BIN"))
        .or_else(|| vars.get("EXE"))
        .map(|s| s.trim().to_string())
        .or_else(|| first_non_phony_rule(&rule_heads))
        .unwrap_or_else(|| default_name.to_string());

    let sources = collect_sources(&vars);
    let entry = pick_entry_source(&sources)
        .unwrap_or_else(|| default_entry_for_languages(&project));

    project.bins.push(ImportedBin {
        name: bin_name,
        src: entry,
    });

    // Recipes are ignored by design, but recording their targets helps the user.
    let skipped: Vec<&String> = rule_heads
        .iter()
        .filter(|r| !is_phony_rule(r) && !r.contains('$'))
        .collect();
    if skipped.len() > 1 {
        project.push_note(format!(
            "{} make rule(s) found; only the primary link target was imported — recipes are not translated",
            skipped.len()
        ));
    }

    project
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Fold backslash-newline continuations into a single logical line.
fn join_continuations(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.split_inclusive('\n') {
        let trimmed_nl = line.trim_end_matches('\n');
        if let Some(head) = trimmed_nl.strip_suffix('\\') {
            cur.push_str(head);
            cur.push(' ');
        } else {
            cur.push_str(trimmed_nl);
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn starts_with_directive(line: &str) -> bool {
    for d in &[
        "include ", "-include ", "sinclude ", "ifeq", "ifneq", "ifdef", "ifndef",
        "else", "endif", "define ", "endef", "override ", "export ", "unexport ",
        "vpath ", ".PHONY", ".SUFFIXES", ".DEFAULT",
    ] {
        if line.starts_with(d) || line.trim_end() == d.trim_end() {
            return true;
        }
    }
    false
}

/// Parse a variable assignment line. Returns (name, op, rhs).
fn parse_assignment(line: &str) -> Option<(&str, &str, &str)> {
    // Look for `:=`, `+=`, `?=`, `=` — in that order so `:=` isn't confused
    // with a rule colon.
    for op in &[":=", "+=", "?=", "="] {
        if let Some(idx) = line.find(op) {
            let var = line[..idx].trim();
            // Must be a bare identifier to be an assignment, not a rule like
            // `target: prereq =foo`.
            if !var.is_empty()
                && var
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                let rhs = &line[idx + op.len()..];
                return Some((var, op, rhs));
            }
        }
    }
    None
}

/// Expand `$(VAR)` / `${VAR}` references against previously-seen variables.
/// Unknown variables expand to the empty string — a pragmatic choice that
/// matches make's own default behaviour for undefined names.
fn expand(vars: &HashMap<String, String>, input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            let open = bytes[i + 1];
            let close = match open {
                b'(' => b')',
                b'{' => b'}',
                _ => {
                    out.push('$');
                    i += 1;
                    continue;
                }
            };
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == close) {
                let var = &input[i + 2..i + 2 + end];
                if let Some(v) = vars.get(var) {
                    out.push_str(v);
                }
                i += 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn extract_flags(project: &mut ImportedProject, text: &str) {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];

        // `-D NAME` and `-I PATH` with a separating space — bump the cursor.
        if tok == "-D" && i + 1 < tokens.len() {
            push_define(project, tokens[i + 1]);
            i += 2;
            continue;
        }
        if tok == "-I" && i + 1 < tokens.len() {
            push_include(project, tokens[i + 1]);
            i += 2;
            continue;
        }

        if let Some(rest) = tok.strip_prefix("-D") {
            push_define(project, rest);
        } else if let Some(rest) = tok.strip_prefix("-I") {
            push_include(project, rest);
        } else if let Some(rest) = tok.strip_prefix("-std=") {
            // Prefer mapping C-family stds onto whichever language section is
            // present; if both are present, CXX wins for c++ prefixes.
            if rest.starts_with("c++") || rest.starts_with("gnu++") {
                let lang = project.language_mut("cpp");
                lang.std = Some(rest.replace("gnu++", "c++"));
            } else if rest.starts_with('c') {
                let lang = project.language_mut("c");
                lang.std = Some(rest.replace("gnu", "c"));
            }
        } else if tok == "-g" || tok.starts_with("-O") || tok == "-Wall" || tok == "-Wextra" {
            // Recognised but represented via crane's structured profile /
            // warnings fields; don't echo as raw flags.
        } else if tok.starts_with('-') && !project.compiler.flags.contains(&tok.to_string()) {
            project.compiler.flags.push(tok.to_string());
        }
        i += 1;
    }
}

fn push_define(project: &mut ImportedProject, name: &str) {
    if name.is_empty() {
        return;
    }
    let s = name.to_string();
    if !project.compiler.defines.contains(&s) {
        project.compiler.defines.push(s);
    }
}

fn push_include(project: &mut ImportedProject, path: &str) {
    if path.is_empty() {
        return;
    }
    let norm = if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    };
    if !project.compiler.include_paths.iter().any(|p| p == &norm) {
        project.compiler.include_paths.push(norm);
    }
}

fn collect_sources(vars: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["SRCS", "SRC", "SOURCES", "OBJS"] {
        if let Some(v) = vars.get(key) {
            for tok in v.split_whitespace() {
                if tok.ends_with(".o") {
                    // Objects: treat `foo.o` as `foo.c` by default.
                    out.push(format!("{}.c", tok.trim_end_matches(".o")));
                } else {
                    out.push(tok.to_string());
                }
            }
        }
    }
    out
}

fn pick_entry_source(sources: &[String]) -> Option<String> {
    let preferred = ["main.cpp", "main.cc", "main.cxx", "main.c", "src/main.cpp", "src/main.c"];
    for p in &preferred {
        if let Some(s) = sources.iter().find(|s| s.ends_with(p)) {
            return Some(s.clone());
        }
    }
    sources.first().cloned()
}

fn default_entry_for_languages(project: &ImportedProject) -> String {
    if project.languages.contains_key("cpp") {
        "src/main.cpp".into()
    } else if project.languages.contains_key("fortran") {
        "src/main.f90".into()
    } else {
        "src/main.c".into()
    }
}

fn first_non_phony_rule(rules: &[String]) -> Option<String> {
    for r in rules {
        if is_phony_rule(r) {
            continue;
        }
        // Skip pattern rules and multi-target rules for the binary name guess.
        if r.contains('%') || r.contains(' ') || r.contains('$') {
            continue;
        }
        return Some(r.clone());
    }
    None
}

fn is_phony_rule(r: &str) -> bool {
    matches!(
        r,
        "all" | "clean" | "install" | "test" | "check" | "distclean" | "dist" | "help"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_basic_variables_and_target() {
        let src = "\
CC = gcc
CFLAGS = -Wall -std=c17 -DUSE_FOO -I include
TARGET = myapp
SRCS = src/main.c src/util.c
LDLIBS = -lm -lpthread

$(TARGET): $(SRCS)
\t$(CC) $(CFLAGS) -o $@ $(SRCS) $(LDLIBS)
";
        let mut p = parse_text(src, "fallback");
        assert_eq!(p.bins[0].name, "myapp");
        assert_eq!(p.bins[0].src, "src/main.c");
        assert!(p.languages.contains_key("c"));
        assert_eq!(p.language_mut("c").std.as_deref(), Some("c17"));
        assert!(p.compiler.defines.contains(&"USE_FOO".to_string()));
        assert!(p.compiler.include_paths.contains(&"include/".to_string()));
        assert!(p.dependencies.contains_key("m"));
        assert!(p.dependencies.contains_key("pthread"));
    }

    #[test]
    fn parses_cxxflags_as_cpp_language() {
        let src = "\
CXX = g++
CXXFLAGS = -std=c++20 -Wall
TARGET = app
SRCS = src/main.cpp
";
        let mut p = parse_text(src, "fallback");
        assert!(p.languages.contains_key("cpp"));
        assert_eq!(p.language_mut("cpp").std.as_deref(), Some("c++20"));
    }

    #[test]
    fn continuations_are_joined() {
        let src = "\
CFLAGS = -Wall \\
         -DFOO \\
         -DBAR
TARGET = x
";
        let p = parse_text(src, "fallback");
        assert!(p.compiler.defines.contains(&"FOO".to_string()));
        assert!(p.compiler.defines.contains(&"BAR".to_string()));
    }

    #[test]
    fn variable_expansion_inside_rhs() {
        let src = "\
BASE = include
CFLAGS = -I$(BASE)/core
TARGET = x
";
        let p = parse_text(src, "fallback");
        assert!(p.compiler.include_paths.contains(&"include/core/".to_string()));
    }

    #[test]
    fn fallbacks_to_dir_name_when_target_missing() {
        let src = "CFLAGS = -Wall\n";
        let p = parse_text(src, "myproj");
        assert_eq!(p.bins[0].name, "myproj");
    }
}
