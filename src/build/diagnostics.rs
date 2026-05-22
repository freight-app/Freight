use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

const MAX_DIAGNOSTICS: usize = 8;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct CompilerDiagnostic {
    path: String,
    line: usize,
    column: usize,
    kind: String,
    code: Option<String>,
    message: String,
}

/// Parse common compiler diagnostic formats and render a concise, clickable
/// summary with source snippets.
///
/// Supported primary formats:
/// - GCC/Clang: `file:line:col: error: message`
/// - MSVC: `file(line,col): error C2143: message`
pub(crate) fn format_compiler_diagnostics(default_source: &Path, raw: &str) -> String {
    let diagnostics = parse_diagnostics(raw);
    if diagnostics.is_empty() {
        return raw.trim().to_owned();
    }

    let mut out = String::new();
    let shown = diagnostics.len().min(MAX_DIAGNOSTICS);
    out.push_str(&format!(
        "compiler diagnostics (showing {shown} of {}):",
        diagnostics.len()
    ));

    for diag in diagnostics.iter().take(MAX_DIAGNOSTICS) {
        out.push_str("\n\n");
        let code = diag
            .code
            .as_deref()
            .map(|c| format!(" {c}"))
            .unwrap_or_default();
        let message = if diag.message.is_empty() {
            String::new()
        } else {
            format!(": {}", diag.message)
        };
        out.push_str(&format!(
            "{}:{}:{}: {}{}{}",
            diag.path, diag.line, diag.column, diag.kind, code, message,
        ));

        if let Some(snippet) = source_snippet(default_source, &diag.path, diag.line, diag.column) {
            out.push('\n');
            out.push_str(&snippet);
        }
    }

    if diagnostics.len() > MAX_DIAGNOSTICS {
        out.push_str(&format!(
            "\n\n... {} more diagnostics omitted; rerun with --verbose for compiler output.",
            diagnostics.len() - MAX_DIAGNOSTICS
        ));
    }

    out
}

fn parse_diagnostics(raw: &str) -> Vec<CompilerDiagnostic> {
    let mut seen = HashSet::new();
    let mut parsed = Vec::new();

    for line in raw.lines() {
        let diag = parse_msvc_line(line).or_else(|| parse_gcc_clang_line(line));
        if let Some(diag) = diag {
            if seen.insert(diag.clone()) {
                parsed.push(diag);
            }
        }
    }

    parsed
}

fn parse_gcc_clang_line(line: &str) -> Option<CompilerDiagnostic> {
    const KINDS: &[(&str, &str)] = &[
        (": fatal error: ", "fatal error"),
        (": error: ", "error"),
        (": warning: ", "warning"),
        (": note: ", "note"),
    ];

    for (marker, kind) in KINDS {
        let Some((location, message)) = line.split_once(marker) else {
            continue;
        };
        let (path, line_no, column) = split_gcc_location(location)?;
        return Some(CompilerDiagnostic {
            path: path.to_owned(),
            line: line_no,
            column,
            kind: (*kind).to_owned(),
            code: None,
            message: message.trim().to_owned(),
        });
    }

    None
}

fn split_gcc_location(location: &str) -> Option<(&str, usize, usize)> {
    let (before_col, col) = location.rsplit_once(':')?;
    let column = col.parse().ok()?;
    let (path, line_no) = before_col.rsplit_once(':')?;
    let line = line_no.parse().ok()?;
    if path.is_empty() || line == 0 || column == 0 {
        return None;
    }
    Some((path, line, column))
}

fn parse_msvc_line(line: &str) -> Option<CompilerDiagnostic> {
    static MSVC_RE: OnceLock<Regex> = OnceLock::new();
    let re = MSVC_RE.get_or_init(|| {
        Regex::new(r"^(?P<path>.+)\((?P<line>\d+)(?:,(?P<col>\d+))?\):\s+(?P<kind>fatal error|error|warning|note)(?:\s+(?P<code>[A-Z]+\d+))?:\s*(?P<msg>.*)$")
            .expect("valid MSVC diagnostic regex")
    });
    let caps = re.captures(line.trim())?;
    let path = caps.name("path")?.as_str().trim();
    let line_no = caps.name("line")?.as_str().parse().ok()?;
    let column = caps
        .name("col")
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(1);
    if path.is_empty() || line_no == 0 || column == 0 {
        return None;
    }

    Some(CompilerDiagnostic {
        path: path.to_owned(),
        line: line_no,
        column,
        kind: caps.name("kind")?.as_str().to_owned(),
        code: caps.name("code").map(|m| m.as_str().to_owned()),
        message: caps
            .name("msg")
            .map(|m| m.as_str().trim().to_owned())
            .unwrap_or_default(),
    })
}

fn source_snippet(
    default_source: &Path,
    diagnostic_path: &str,
    line: usize,
    column: usize,
) -> Option<String> {
    let path = resolve_snippet_path(default_source, diagnostic_path);
    let source = fs::read_to_string(path).ok()?;
    let line_text = source.lines().nth(line.saturating_sub(1))?;
    let line_no_width = line.to_string().len().max(2);
    let caret_col = visual_column(line_text, column);
    Some(format!(
        "{line:>line_no_width$} | {line_text}\n{blank:>line_no_width$} | {caret:>caret_col$}^",
        blank = "",
        caret = "",
    ))
}

fn resolve_snippet_path(default_source: &Path, diagnostic_path: &str) -> PathBuf {
    let direct = PathBuf::from(diagnostic_path);
    if direct.is_file() {
        return direct;
    }

    if default_source.is_file() {
        return default_source.to_path_buf();
    }

    direct
}

fn visual_column(line_text: &str, one_based_column: usize) -> usize {
    let mut width = 1;
    for ch in line_text.chars().take(one_based_column.saturating_sub(1)) {
        width += if ch == '\t' { 4 } else { 1 };
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_gcc_clang_diagnostic_with_snippet() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("main.cpp");
        fs::write(&src, "int main() {\n  nope\n}\n").unwrap();
        let raw = format!(
            "{}:2:3: error: use of undeclared identifier 'nope'\n  nope\n  ^",
            src.display()
        );

        let formatted = format_compiler_diagnostics(&src, &raw);

        assert!(formatted.contains("compiler diagnostics (showing 1 of 1):"));
        assert!(formatted.contains(&format!(
            "{}:2:3: error: use of undeclared identifier 'nope'",
            src.display()
        )));
        assert!(formatted.contains("2 |   nope"));
        assert!(formatted.contains("  |    ^"));
        assert!(!formatted.contains("use of undeclared identifier 'nope'\n  nope\n  ^"));
    }

    #[test]
    fn formats_msvc_diagnostic_with_code_as_clickable_reference() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("main.cpp");
        fs::write(&src, "int main() {\n  return\n}\n").unwrap();
        let raw = format!(
            "{}(2,9): error C2143: syntax error: missing ';' before '}}'",
            src.display()
        );

        let formatted = format_compiler_diagnostics(&src, &raw);

        assert!(formatted.contains(&format!(
            "{}:2:9: error C2143: syntax error: missing ';' before '}}'",
            src.display()
        )));
        assert!(formatted.contains("2 |   return"));
    }

    #[test]
    fn returns_trimmed_raw_output_when_no_known_diagnostic_is_found() {
        assert_eq!(
            format_compiler_diagnostics(Path::new("main.cpp"), " linker exploded \n"),
            "linker exploded"
        );
    }
}
