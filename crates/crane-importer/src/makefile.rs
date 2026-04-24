//! Makefile → [`ImportedProject`] using the `makefile-lossless` crate.
//!
//! Makefiles are ad-hoc: there is no single canonical shape. This importer
//! pulls out the pieces that map cleanly to crane (compiler variables, flag
//! variables, source lists, the primary link target) and flags everything else
//! as a note so the user can review.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use makefile_lossless::Makefile;

use crate::{ImportedBin, ImportedDep, ImportedProject};
use crane_core::error::CraneError;

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

    // makefile-lossless 0.3's raw_value() returns only the first physical line
    // of a multi-line variable, so we join backslash continuations ourselves
    // before parsing.
    let joined = join_continuations(text);

    // Parse with relaxed mode so minor syntax errors don't abort the import.
    let (mf, _errors) = Makefile::from_str_relaxed(&joined);

    // Collect variable definitions in file order, expanding references as we go.
    // This mirrors the original hand-rolled behaviour: later assignments win for
    // `=` / `:=` / `?=`, while `+=` appends.
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut rule_heads: Vec<String> = Vec::new();

    for var in mf.variable_definitions() {
        let name = match var.name() {
            Some(n) => n,
            None => continue,
        };
        let raw = var.raw_value().unwrap_or_default();
        let expanded = expand(&vars, raw.trim());
        let op = var.assignment_operator().unwrap_or_else(|| "=".to_string());
        if op == "+=" {
            let cur = vars.entry(name.clone()).or_default();
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(&expanded);
        } else {
            vars.insert(name.clone(), expanded);
        }
    }

    // Collect rule targets for the binary name heuristic.
    for rule in mf.rules() {
        for target in rule.targets() {
            if !target.is_empty() {
                rule_heads.push(target);
            }
        }
    }

    // Emit a note for conditionals — we don't attempt to evaluate them.
    let conditional_count = mf.conditionals().count();
    if conditional_count > 0 {
        project.push_note(format!(
            "{conditional_count} conditional block(s) (ifdef/ifeq/…) found — contents were imported unconditionally; review manually"
        ));
    }

    // ── Languages ──
    let has_cxxflags = vars.contains_key("CXXFLAGS") || vars.contains_key("CXX");
    let has_cflags = vars.contains_key("CFLAGS") || vars.contains_key("CC");
    let has_fflags = vars.contains_key("FFLAGS") || vars.contains_key("FC");

    if has_cxxflags {
        project.language_mut("cpp");
    }
    if has_cflags {
        project.language_mut("c");
    }
    if has_fflags {
        project.language_mut("fortran");
    }
    if project.languages.is_empty() {
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

    // ── System deps from LDLIBS / LDFLAGS / LIBS ──
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
    let entry =
        pick_entry_source(&sources).unwrap_or_else(|| default_entry_for_languages(&project));

    project.bins.push(ImportedBin {
        name: bin_name,
        src: entry,
    });

    let skipped: Vec<&String> = rule_heads
        .iter()
        .filter(|r| !is_phony_rule(r) && !r.contains('%') && !r.contains('$'))
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

/// Join backslash-newline continuations so that `makefile-lossless`'s
/// `raw_value()` — which only returns the first physical line — sees the
/// full value.
fn join_continuations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n') {
        if let Some(prefix) = line.strip_suffix('\\') {
            out.push_str(prefix);
            out.push(' ');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
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
            if rest.starts_with("c++") || rest.starts_with("gnu++") {
                let lang = project.language_mut("cpp");
                lang.std = Some(rest.replace("gnu++", "c++"));
            } else if rest.starts_with('c') {
                let lang = project.language_mut("c");
                lang.std = Some(rest.replace("gnu", "c"));
            }
        } else if tok == "-g" || tok.starts_with("-O") || tok == "-Wall" || tok == "-Wextra" {
            // Represented via crane's structured profile/warnings fields.
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
    let preferred = [
        "main.cpp", "main.cc", "main.cxx", "main.c", "src/main.cpp", "src/main.c",
    ];
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
