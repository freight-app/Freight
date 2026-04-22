//! Context-aware completions for `crane.toml`.
//!
//! Strategy: look at the current line and the most recent `[section]` header
//! above the cursor. That's enough to produce useful suggestions without a
//! full TOML parse — which would fail on the in-progress text anyway.

use crane_core::toolchain::CompilerTemplate;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};

/// Build a list of completion items for `pos` in `src`.
pub fn complete(
    src: &str,
    pos: Position,
    templates: &[CompilerTemplate],
) -> Vec<CompletionItem> {
    let ctx = context_at(src, pos);

    match ctx.section.as_deref() {
        // Top-level: suggest the well-known section headers.
        None | Some("") => top_level_sections(),

        // [package]
        Some("package") => field_values(&ctx, "name", &[])
            .or_else(|| field_values(&ctx, "license", &LICENSES))
            .unwrap_or_else(|| package_fields()),

        // [compiler]
        Some("compiler") => field_values(&ctx, "backend", &backend_names(templates))
            .or_else(|| field_values(&ctx, "warnings", &WARNINGS))
            .or_else(|| field_values(&ctx, "opt-level", &OPT_LEVELS))
            .unwrap_or_else(|| compiler_fields()),

        // [language.<key>]
        Some(s) if s.starts_with("language.") => {
            field_values(&ctx, "std", &std_values(templates))
                .unwrap_or_else(language_fields)
        }

        // [lib]
        Some("lib") => field_values(&ctx, "type", &LIB_TYPES)
            .unwrap_or_else(lib_fields),

        // [[bin]]
        Some("bin") => bin_fields(),

        // [profile.dev] / [profile.release]
        Some(s) if s.starts_with("profile.") => profile_fields(),

        // [dependencies] / [dev-dependencies]
        Some("dependencies") | Some("dev-dependencies") => Vec::new(),

        // [target]
        Some("target") => target_fields(),

        _ => Vec::new(),
    }
}

// ── Context detection ────────────────────────────────────────────────────────

struct LineCtx {
    /// Section name (without brackets), e.g. `"package"` or `"language.cpp"`.
    /// `None` at the very top of the file before any header.
    section: Option<String>,
    /// The key being typed on the current line (text before `=`), if any.
    current_key: Option<String>,
    /// `true` when the cursor is clearly on the value side of `=`.
    on_value: bool,
}

fn context_at(src: &str, pos: Position) -> LineCtx {
    let byte = crate::position::position_to_byte(src, pos);
    let before = &src[..byte];

    // Most recent header in the prefix.
    let section = before
        .lines()
        .rev()
        .find_map(|l| parse_header(l.trim()));

    // Current line (up to the cursor).
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_prefix = &src[line_start..byte];

    let (current_key, on_value) = if let Some(eq) = line_prefix.find('=') {
        let key = line_prefix[..eq].trim().trim_start_matches('#').trim().to_string();
        (Some(key), true)
    } else {
        (None, false)
    };

    LineCtx { section, current_key, on_value }
}

/// `[package]` → `"package"`, `[[bin]]` → `"bin"`, `[language.cpp]` → `"language.cpp"`.
fn parse_header(line: &str) -> Option<String> {
    let s = line.strip_prefix("[[").and_then(|s| s.strip_suffix("]]"))
        .or_else(|| line.strip_prefix('[').and_then(|s| s.strip_suffix(']')))?;
    Some(s.to_string())
}

// ── Value completion helpers ─────────────────────────────────────────────────

fn field_values(ctx: &LineCtx, key: &str, values: &[&str]) -> Option<Vec<CompletionItem>> {
    if !ctx.on_value || ctx.current_key.as_deref() != Some(key) {
        return None;
    }
    Some(values.iter().map(|v| CompletionItem {
        label: format!("\"{v}\""),
        kind: Some(CompletionItemKind::VALUE),
        insert_text: Some(format!("\"{v}\"")),
        ..Default::default()
    }).collect())
}

// ── Section + field menus ────────────────────────────────────────────────────

fn top_level_sections() -> Vec<CompletionItem> {
    [
        "[package]", "[lib]", "[[bin]]",
        "[dependencies]", "[dev-dependencies]",
        "[language.cpp]", "[language.c]", "[language.fortran]",
        "[compiler]", "[compiler.includes]",
        "[profile.dev]", "[profile.release]",
        "[target]",
    ]
    .into_iter()
    .map(|s| CompletionItem {
        label: s.into(),
        kind: Some(CompletionItemKind::MODULE),
        ..Default::default()
    })
    .collect()
}

fn package_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("name", "name = \"${1:my-package}\""),
        ("version", "version = \"${1:0.1.0}\""),
        ("authors", "authors = [\"${1:You <you@example.com>}\"]"),
        ("description", "description = \"${1:short summary}\""),
        ("license", "license = \"${1:MIT}\""),
        ("repository", "repository = \"${1:https://example.com/repo}\""),
    ])
}

fn compiler_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("backend", "backend = \"${1:auto}\""),
        ("opt-level", "opt-level = ${1:2}"),
        ("debug", "debug = ${1:false}"),
        ("warnings", "warnings = \"${1:all}\""),
        ("defines", "defines = [${1}]"),
        ("flags", "flags = [${1}]"),
    ])
}

fn language_fields() -> Vec<CompletionItem> {
    snippet_fields(&[("std", "std = \"${1:c++20}\"")])
}

fn lib_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("type", "type = \"${1:static}\""),
        ("src", "src = \"${1:src/}\""),
        ("include", "include = \"${1:include/}\""),
    ])
}

fn bin_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("name", "name = \"${1:my-bin}\""),
        ("src", "src = \"${1:src/main.cpp}\""),
    ])
}

fn profile_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("opt-level", "opt-level = ${1:3}"),
        ("debug", "debug = ${1:false}"),
        ("lto", "lto = ${1:true}"),
        ("strip", "strip = ${1:true}"),
        ("sanitize", "sanitize = [${1}]"),
    ])
}

fn target_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("arch", "arch = \"${1:x86_64}\""),
        ("cpu_extensions", "cpu_extensions = [${1}]"),
    ])
}

fn snippet_fields(fields: &[(&str, &str)]) -> Vec<CompletionItem> {
    fields.iter().map(|(label, snippet)| CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::FIELD),
        insert_text: Some(snippet.to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }).collect()
}

// ── Value tables ─────────────────────────────────────────────────────────────

const WARNINGS: &[&str] = &["none", "default", "all", "error"];
const LIB_TYPES: &[&str] = &["static", "shared", "header-only"];
const OPT_LEVELS: &[&str] = &["0", "1", "2", "3"];
const LICENSES: &[&str] = &["MIT", "Apache-2.0", "BSD-3-Clause", "GPL-3.0-or-later", "MPL-2.0"];

fn backend_names(templates: &[CompilerTemplate]) -> Vec<&str> {
    let mut out = vec!["auto"];
    for t in templates {
        out.push(t.name.as_str());
    }
    out
}

fn std_values(templates: &[CompilerTemplate]) -> Vec<&str> {
    // Collect every standard string the loaded templates know about.
    let mut out: Vec<&str> = Vec::new();
    for t in templates {
        for k in t.standards.keys() {
            if !out.contains(&k.as_str()) {
                out.push(k.as_str());
            }
        }
    }
    if out.is_empty() {
        out.extend(&["c11", "c17", "c23", "c++17", "c++20", "c++23"]);
    }
    out
}
