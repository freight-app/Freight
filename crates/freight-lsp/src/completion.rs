//! Context-aware completions for Freight manifests, build scripts, and compiler templates.
//!
//! Strategy: keep completion lightweight and resilient while the user is editing.
//! Manifest completions look at the current TOML section; Rhai-based scripts and
//! templates suggest the Freight-specific globals/functions that the runtime exposes.

use freight_core::toolchain::CompilerTemplate;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Manifest,
    BuildScript,
    CompilerTemplate,
    FortranSource,
}

/// Build a list of completion items for `pos` in `src`.
pub fn complete(
    src: &str,
    pos: Position,
    templates: &[CompilerTemplate],
    kind: DocumentKind,
) -> Vec<CompletionItem> {
    match kind {
        DocumentKind::Manifest => complete_manifest(src, pos, templates),
        DocumentKind::BuildScript => build_script_items(),
        DocumentKind::CompilerTemplate => compiler_template_items(),
        DocumentKind::FortranSource => crate::fortran::completions(src),
    }
}

fn complete_manifest(
    src: &str,
    pos: Position,
    templates: &[CompilerTemplate],
) -> Vec<CompletionItem> {
    let ctx = context_at(src, pos);

    match ctx.section.as_deref() {
        // Top-level: suggest the well-known section headers.
        None | Some("") => top_level_sections(templates),

        // [package]
        Some("package") => field_values(&ctx, "license", LICENSES).unwrap_or_else(package_fields),

        // [compiler]
        Some("compiler") => field_values(&ctx, "warnings", WARNINGS)
            .or_else(|| field_values(&ctx, "opt-level", OPT_LEVELS))
            .or_else(|| field_values(&ctx, "pch", &[]))
            .unwrap_or_else(compiler_fields),

        // [compiler.includes]
        Some("compiler.includes") => compiler_includes_fields(),

        // [language.<key>]
        Some(s) if s.starts_with("language.") => field_values(&ctx, "std", &std_values(templates))
            .or_else(|| field_values(&ctx, "stdlib", STDLIBS))
            .unwrap_or_else(language_fields),

        // [lib]
        Some("lib") => field_values(&ctx, "type", LIB_TYPES).unwrap_or_else(lib_fields),

        // [[bin]]
        Some("bin") => bin_fields(),

        // [profile.dev] / [profile.release] / [profile.<custom>]
        Some(s) if s.starts_with("profile.") => profile_fields(),

        // [features]
        Some("features") => Vec::new(),

        // [dependencies] / [dev-dependencies]
        Some("dependencies") | Some("dev-dependencies") => dependency_value_snippets(),

        // [platform.<os>] / nested platform tables
        Some(s) if s.starts_with("platform.") => platform_fields(s),

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
    let section = before.lines().rev().find_map(|l| parse_header(l.trim()));

    // Current line (up to the cursor).
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_prefix = &src[line_start..byte];

    let (current_key, on_value) = if let Some(eq) = line_prefix.find('=') {
        let key = line_prefix[..eq]
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_string();
        (Some(key), true)
    } else {
        (None, false)
    };

    LineCtx {
        section,
        current_key,
        on_value,
    }
}

/// `[package]` → `"package"`, `[[bin]]` → `"bin"`, `[language.cpp]` → `"language.cpp"`.
fn parse_header(line: &str) -> Option<String> {
    let s = line
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .or_else(|| line.strip_prefix('[').and_then(|s| s.strip_suffix(']')))?;
    Some(s.to_string())
}

// ── Value completion helpers ─────────────────────────────────────────────────

fn field_values(ctx: &LineCtx, key: &str, values: &[&str]) -> Option<Vec<CompletionItem>> {
    if !ctx.on_value || ctx.current_key.as_deref() != Some(key) || values.is_empty() {
        return None;
    }
    Some(
        values
            .iter()
            .map(|v| CompletionItem {
                label: format!("\"{v}\""),
                kind: Some(CompletionItemKind::VALUE),
                insert_text: Some(format!("\"{v}\"")),
                ..Default::default()
            })
            .collect(),
    )
}

// ── Section + field menus ────────────────────────────────────────────────────

fn top_level_sections(templates: &[CompilerTemplate]) -> Vec<CompletionItem> {
    let mut sections = vec![
        "[package]".to_string(),
        "[lib]".to_string(),
        "[[bin]]".to_string(),
        "[dependencies]".to_string(),
        "[dev-dependencies]".to_string(),
        "[features]".to_string(),
        "[compiler]".to_string(),
        "[compiler.includes]".to_string(),
        "[profile.dev]".to_string(),
        "[profile.release]".to_string(),
        "[target]".to_string(),
        "[platform.linux.dependencies]".to_string(),
    ];

    for lang in language_keys(templates) {
        let section = format!("[language.{lang}]");
        if !sections.contains(&section) {
            sections.push(section);
        }
    }

    sections
        .into_iter()
        .map(|s| CompletionItem {
            label: s,
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
        ("readme", "readme = \"${1:README.md}\""),
        (
            "repository",
            "repository = \"${1:https://example.com/repo}\"",
        ),
        ("keywords", "keywords = [\"${1:cpp}\"]"),
        ("provides", "provides = [\"${1:blas}\"]"),
    ])
}

fn compiler_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("opt-level", "opt-level = ${1:2}"),
        ("debug", "debug = ${1:false}"),
        ("warnings", "warnings = \"${1:all}\""),
        ("defines", "defines = [${1}]"),
        ("flags", "flags = [${1}]"),
        ("overrides", "overrides = { ${1:cpp = \"gcc\"} }"),
        ("pch", "pch = \"${1:include/stdafx.h}\""),
    ])
}

fn compiler_includes_fields() -> Vec<CompletionItem> {
    snippet_fields(&[("paths", "paths = [\"${1:include}\"]")])
}

fn language_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("std", "std = \"${1:c++20}\""),
        ("stdlib", "stdlib = \"${1:libstdc++}\""),
    ])
}

fn lib_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("type", "type = \"${1:static}\""),
        ("src", "src = \"${1:src/}\""),
        ("inc", "inc = \"${1:include/}\""),
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
        ("inherits", "inherits = \"${1:release}\""),
        ("opt-level", "opt-level = ${1:3}"),
        ("debug", "debug = ${1:false}"),
        ("lto", "lto = ${1:true}"),
        ("strip", "strip = ${1:true}"),
        ("sanitize", "sanitize = [\"${1:address}\"]"),
        ("features", "features = [\"${1:feature-name}\"]"),
    ])
}

fn target_fields() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("arch", "arch = \"${1:x86_64}\""),
        ("cpu_extensions", "cpu_extensions = [\"${1:avx2}\"]"),
    ])
}

fn dependency_value_snippets() -> Vec<CompletionItem> {
    snippet_fields(&[
        ("path dep", "${1:name} = { path = \"${2:../dep}\" }"),
        ("system dep", "${1:name} = { system = \"${2:ssl}\" }"),
        ("git dep", "${1:name} = { git = \"${2:https://example.com/repo.git}\", rev = \"${3:main}\" }"),
        ("url dep", "${1:name} = { url = \"${2:https://example.com/archive.tar.gz}\", sha256 = \"${3:checksum}\" }"),
        ("pkg-config dep", "${1:name} = { pkg_config = \"${2:libfoo >= 1.0}\" }"),
    ])
}

fn platform_fields(section: &str) -> Vec<CompletionItem> {
    if section.ends_with(".compiler") {
        snippet_fields(&[("defines", "defines = [${1}]"), ("flags", "flags = [${1}]")])
    } else if section.contains(".language.") {
        language_fields()
    } else if section.ends_with(".dependencies") {
        dependency_value_snippets()
    } else {
        snippet_fields(&[
            ("dependencies", "[platform.${1:linux}.dependencies]"),
            ("compiler", "[platform.${1:linux}.compiler]"),
            ("language", "[platform.${1:linux}.language.${2:cpp}]"),
        ])
    }
}

fn snippet_fields(fields: &[(&str, &str)]) -> Vec<CompletionItem> {
    fields
        .iter()
        .map(|(label, snippet)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            insert_text: Some(snippet.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        })
        .collect()
}

// ── Rhai completion menus ────────────────────────────────────────────────────

fn build_script_items() -> Vec<CompletionItem> {
    let functions = [
        ("package_name", "package_name()", "Current package name."),
        (
            "package_version",
            "package_version()",
            "Current package version.",
        ),
        ("profile", "profile()", "Active build profile."),
        ("out_dir", "out_dir()", "Generated build output directory."),
        ("src_dir", "src_dir()", "Project source directory."),
        (
            "define",
            "define(\"${1:NAME}\")",
            "Add a preprocessor define.",
        ),
        (
            "define_value",
            "define_value(\"${1:NAME}\", \"${2:value}\")",
            "Add a define with a value.",
        ),
        (
            "add_include",
            "add_include(\"${1:path}\")",
            "Add an include directory.",
        ),
        (
            "add_flag",
            "add_flag(\"${1:-Wall}\")",
            "Add a compiler flag.",
        ),
        (
            "add_link_lib",
            "add_link_lib(\"${1:ssl}\")",
            "Link a system library.",
        ),
        (
            "add_link_flag",
            "add_link_flag(\"${1:-Wl,...}\")",
            "Add a linker flag.",
        ),
        (
            "link_path",
            "link_path(\"${1:path}\")",
            "Add a library search path.",
        ),
        (
            "add_source",
            "add_source(\"${1:path}\")",
            "Add a generated source file.",
        ),
        (
            "warning",
            "warning(\"${1:message}\")",
            "Emit a non-fatal warning.",
        ),
        (
            "rerun_if",
            "rerun_if(\"${1:path}\")",
            "Cache the script until this path changes.",
        ),
        (
            "write_file",
            "write_file(${1:path}, ${2:content})",
            "Write a generated file.",
        ),
        (
            "read_file",
            "read_file(\"${1:path}\")",
            "Read a file or return an empty string.",
        ),
        (
            "path_exists",
            "path_exists(\"${1:path}\")",
            "Check whether a path exists.",
        ),
        ("mkdir", "mkdir(\"${1:path}\")", "Create a directory tree."),
        (
            "pkg_config_cflags",
            "pkg_config_cflags(\"${1:openssl}\")",
            "Query pkg-config cflags.",
        ),
        (
            "pkg_config_libs",
            "pkg_config_libs(\"${1:openssl}\")",
            "Query pkg-config libs.",
        ),
        (
            "pkg_config_apply",
            "pkg_config_apply(\"${1:openssl}\")",
            "Apply pkg-config flags/libs.",
        ),
        (
            "find_tool",
            "find_tool(\"${1:protoc}\")",
            "Find a tool on PATH.",
        ),
        (
            "pkg_config_exists",
            "pkg_config_exists(\"${1:openssl}\")",
            "Check pkg-config availability.",
        ),
        (
            "run",
            "run(\"${1:cmd}\", [${2}])",
            "Run a command in the project directory.",
        ),
        ("fail", "fail(\"${1:message}\")", "Abort the build script."),
        (
            "glob",
            "glob(\"${1:**/*.proto}\")",
            "List matching project files.",
        ),
        (
            "changed_files",
            "changed_files(\"${1:**/*.proto}\")",
            "List matching files newer than the stamp.",
        ),
        (
            "file_stem",
            "file_stem(\"${1:path}\")",
            "Return a path's file stem.",
        ),
        (
            "file_name",
            "file_name(\"${1:path}\")",
            "Return a path's file name.",
        ),
        (
            "file_dir",
            "file_dir(\"${1:path}\")",
            "Return a path's parent directory.",
        ),
    ];

    let globals = [
        (
            "env",
            "env[\"${1:VAR}\"]",
            "Read or set environment variables for script commands.",
        ),
        (
            "toolchain",
            "toolchain[\"${1:backend}\"]",
            "Read backend/version/target/arch/os toolchain metadata.",
        ),
        (
            "packages",
            "packages[\"${1:name}\"].found",
            "Read resolved pkg-config dependency status.",
        ),
    ];

    functions
        .into_iter()
        .map(function_item)
        .chain(globals.into_iter().map(global_item))
        .collect()
}

fn compiler_template_items() -> Vec<CompletionItem> {
    let variables = [
        ("name", "name = \"${1:gcc}\"", "Template identifier."),
        ("family", "family = \"${1:gnu}\"", "Toolchain family."),
        (
            "sanitizers",
            "sanitizers = [\"${1:address}\"]",
            "Supported sanitizer names.",
        ),
        (
            "homepage",
            "homepage = \"${1:https://example.com}\"",
            "Informational homepage.",
        ),
        ("binary", "binary = \"${1:cc}\"", "Primary compiler binary."),
        (
            "version_arg",
            "version_arg = \"${1:--version}\"",
            "Version probe argument.",
        ),
        (
            "version_regex",
            "version_regex = \"${1:\\\\b(\\\\d+\\\\.\\\\d+)\\\\b}\"",
            "Regex capture for compiler version.",
        ),
        (
            "extensions",
            "extensions = [\"${1:.cpp}\"]",
            "Claimed source extensions.",
        ),
        (
            "always_flags",
            "always_flags = [\"${1:-pipe}\"]",
            "Flags always emitted.",
        ),
        (
            "passthrough",
            "passthrough = ${1:false}",
            "Whether this is a wrapper compiler.",
        ),
        (
            "passthrough_prefix",
            "passthrough_prefix = \"${1:-Xcompiler}\"",
            "Wrapper passthrough prefix.",
        ),
        (
            "supported_archs",
            "supported_archs = [\"${1:x86_64}\"]",
            "Host architecture allowlist.",
        ),
        (
            "supported_os",
            "supported_os = [\"${1:linux}\"]",
            "Host OS allowlist.",
        ),
        (
            "required_tools",
            "required_tools = [\"${1:ar}\"]",
            "Required tools on PATH.",
        ),
        (
            "required_env",
            "required_env = [\"${1:ONEAPI_ROOT}\"]",
            "Required environment variables.",
        ),
        (
            "requires_toolchain",
            "requires_toolchain = [\"${1:cpp}\"]",
            "Host language ABI requirements.",
        ),
        (
            "min_version",
            "min_version = \"${1:12.0}\"",
            "Minimum compiler version.",
        ),
        (
            "include_dir",
            "include_dir = \"${1:-I{path}}\"",
            "Include directory flag template.",
        ),
        (
            "define",
            "define = \"${1:-D{name}}\"",
            "Define flag template.",
        ),
        (
            "define_value",
            "define_value = \"${1:-D{name}={value}}\"",
            "Define-with-value flag template.",
        ),
        (
            "output",
            "output = \"${1:-o {path}}\"",
            "Default output flag template.",
        ),
        (
            "output_obj",
            "output_obj = \"${1:-o {path}}\"",
            "Compile-step output flag template.",
        ),
        (
            "output_bin",
            "output_bin = \"${1:-o {path}}\"",
            "Link-step output flag template.",
        ),
        (
            "compile_only",
            "compile_only = \"${1:-c}\"",
            "Compile-only flag.",
        ),
        (
            "dep_file",
            "dep_file = \"${1:-MMD -MF {path}}\"",
            "Dependency file flag template.",
        ),
        (
            "dep_file_mode",
            "dep_file_mode = \"${1:file}\"",
            "Dependency tracking mode.",
        ),
        (
            "system_lib",
            "system_lib = \"${1:-l{name}}\"",
            "System library link template.",
        ),
        (
            "target",
            "target = \"${1:--target={triple}}\"",
            "Target triple flag template.",
        ),
        (
            "sysroot",
            "sysroot = \"${1:--sysroot={path}}\"",
            "Sysroot flag template.",
        ),
        ("arch", "arch", "Host architecture string."),
        ("os", "os", "Host operating system string."),
    ];

    let maps = [
        ("flags", "flags[\"${1:opt}\"][\"${2:2}\"] = \"${3:-O2}\"", "Compiler flag maps."),
        ("standards", "standards[\"${1:c++20}\"] = \"${2:-std=c++20}\"", "Language standard flags."),
        ("modules", "modules[\"${1:style}\"] = \"${2:gcc}\"", "Module support parameters."),
        ("linking", "linking[\"${1:cpp}\"] = #{ abi: \"${2:c++}\", compatible: [${3}], linker: \"\", extensions: [\"${4:.cpp}\"] };", "Language ABI/linking metadata."),
        ("toolset", "toolset[\"${1:cc}\"] = \"${2:gcc}\"", "Tool role binary map."),
        ("load_flags", "load_flags[\"${1:cc}\"] += [\"${2:-m64}\"]", "Dynamic flags appended by load()."),
        ("arch_flags", "arch_flags[\"${1:x86_64.linux}\"] = \"${2:-f elf64}\"", "Architecture-specific flags."),
        ("pch", "pch[\"${1:compile}\"] = \"${2:-x c++-header}\"", "Precompiled header parameters."),
        ("env", "env[\"${1:CC}\"]", "Read host environment variables."),
    ];

    let functions = [
        (
            "find_tool",
            "find_tool(\"${1:gcc}\")",
            "Find a binary on PATH.",
        ),
        (
            "check",
            "fn check() {\n    ${1:true}\n}",
            "Optional availability hook.",
        ),
        (
            "load",
            "fn load() {\n    ${1:// add load_flags here}\n}",
            "Optional dynamic load hook.",
        ),
    ];

    variables
        .into_iter()
        .map(field_item)
        .chain(maps.into_iter().map(global_item))
        .chain(functions.into_iter().map(function_item))
        .collect()
}

fn field_item((label, snippet, detail): (&str, &str, &str)) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(detail.to_string()),
        insert_text: Some(snippet.to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

fn function_item((label, snippet, detail): (&str, &str, &str)) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(detail.to_string()),
        insert_text: Some(snippet.to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

fn global_item((label, snippet, detail): (&str, &str, &str)) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::VARIABLE),
        detail: Some(detail.to_string()),
        insert_text: Some(snippet.to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

// ── Value tables ─────────────────────────────────────────────────────────────

const WARNINGS: &[&str] = &["none", "default", "all", "error"];
const LIB_TYPES: &[&str] = &["static", "shared", "header-only"];
const OPT_LEVELS: &[&str] = &["0", "1", "2", "3"];
const LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-3-Clause",
    "GPL-3.0-or-later",
    "MPL-2.0",
];
const STDLIBS: &[&str] = &["libstdc++", "libc++", "none"];

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

fn language_keys(templates: &[CompilerTemplate]) -> Vec<&str> {
    let mut out = Vec::new();
    for t in templates {
        for key in t.linking.keys() {
            if !out.contains(&key.as_str()) {
                out.push(key.as_str());
            }
        }
    }
    if out.is_empty() {
        out.extend(&["cpp", "c", "fortran", "ada", "d", "cuda", "asm"]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_completion_includes_compiler_includes_paths() {
        let src = "[compiler.includes]\n";
        let items = complete(
            src,
            Position {
                line: 1,
                character: 0,
            },
            &[],
            DocumentKind::Manifest,
        );
        assert!(items.iter().any(|i| i.label == "paths"));
    }

    #[test]
    fn build_script_completion_knows_freight_apis() {
        let items = complete(
            "",
            Position {
                line: 0,
                character: 0,
            },
            &[],
            DocumentKind::BuildScript,
        );
        assert!(items.iter().any(|i| i.label == "add_source"));
        assert!(items.iter().any(|i| i.label == "packages"));
    }

    #[test]
    fn template_completion_knows_template_maps() {
        let items = complete(
            "",
            Position {
                line: 0,
                character: 0,
            },
            &[],
            DocumentKind::CompilerTemplate,
        );
        assert!(items.iter().any(|i| i.label == "flags"));
        assert!(items.iter().any(|i| i.label == "linking"));
    }
}
