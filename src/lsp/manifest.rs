//! freight.toml LSP features: diagnostics, completion, hover, signature help.

use std::path::Path;

use crate::manifest::{
    load_manifest_str, load_workspace_manifest_str, validate, validate_dep_compat, WorkspaceSection,
};
use crate::toolchain::CompilerTemplate;
use serde_json::{json, Value};

use super::protocol::parse_line_col;

#[derive(Debug, Clone, Default)]
pub struct WorkspaceInventory {
    pub packages: Vec<WorkspacePackage>,
}

#[derive(Debug, Clone)]
pub struct WorkspacePackage {
    pub name: String,
    pub path: String,
    pub bins: Vec<String>,
    pub lib: Option<String>,
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

pub fn manifest_diagnostics(text: &str, dir: &Path, templates: &[CompilerTemplate]) -> Vec<Value> {
    if let Ok(workspace) = load_workspace_manifest_str(text) {
        return workspace_diagnostics(text, dir, &workspace);
    }

    let manifest = match load_manifest_str(text) {
        Ok(m) => m,
        Err(e) => {
            let (line, character) = parse_line_col(&e.to_string()).unwrap_or((0, 0));
            return vec![diagnostic(
                line,
                character,
                "freight.toml could not be parsed",
                &e.to_string(),
            )];
        }
    };
    let mut errors = validate(&manifest, templates);
    errors.extend(validate_dep_compat(&manifest, dir, templates));
    errors
        .into_iter()
        .map(|e| {
            let line = line_for_context(text, &e.context);
            diagnostic(line, 0, &e.context, &e.message)
        })
        .collect()
}

fn workspace_diagnostics(text: &str, dir: &Path, workspace: &WorkspaceSection) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    if workspace.members.is_empty() {
        diagnostics.push(diagnostic(
            line_for_context(text, "members"),
            0,
            "[workspace]",
            "workspace.members must list at least one package directory",
        ));
        return diagnostics;
    }

    for member in &workspace.members {
        let line =
            line_for_member(text, member).unwrap_or_else(|| line_for_context(text, "members"));
        if member.trim().is_empty() {
            diagnostics.push(diagnostic(
                line,
                0,
                "[workspace.members]",
                "workspace member path must not be empty",
            ));
            continue;
        }
        let member_dir = dir.join(member.trim_end_matches('/'));
        let manifest_path = member_dir.join("freight.toml");
        if !manifest_path.is_file() {
            diagnostics.push(diagnostic(
                line,
                0,
                "[workspace.members]",
                &format!("workspace member `{member}` does not contain a freight.toml"),
            ));
        }
    }
    diagnostics
}

pub fn diagnostic(line: usize, character: usize, code: &str, message: &str) -> Value {
    json!({
        "range": {
            "start": { "line": line, "character": character },
            "end":   { "line": line, "character": character.saturating_add(1) }
        },
        "severity": 1,
        "source": "freight",
        "code": code,
        "message": message
    })
}

fn line_for_context(text: &str, context: &str) -> usize {
    let section = context.split_whitespace().next().unwrap_or(context).trim();
    text.lines().position(|l| l.trim() == section).unwrap_or(0)
}

fn line_for_member(text: &str, member: &str) -> Option<usize> {
    text.lines()
        .position(|line| line.contains(member) && !line.trim_start().starts_with('#'))
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

pub fn completion_result(
    text: Option<&str>,
    pos: Option<(usize, usize)>,
    inventory: Option<&WorkspaceInventory>,
) -> Value {
    let section = text
        .zip(pos)
        .and_then(|(t, (line, _))| current_section(t, line))
        .unwrap_or_default();

    let labels: Vec<(&str, &str, &str)> = if section == "package" {
        vec![
            (
                "name",
                "Package name",
                "Registry and build identity for this package.",
            ),
            (
                "version",
                "SemVer package version",
                "Version published to the Freight registry.",
            ),
            (
                "authors",
                "Package authors",
                "Array of author names or contacts.",
            ),
            (
                "description",
                "Short package description",
                "Shown in registry/package help surfaces.",
            ),
            (
                "license",
                "SPDX license",
                "Use an SPDX expression such as MIT or Apache-2.0.",
            ),
            (
                "readme",
                "README path",
                "Relative path to package README content.",
            ),
            (
                "repository",
                "Source repository URL",
                "Project homepage or source repository.",
            ),
            (
                "supports",
                "Boolean platform support expression",
                "Gate the package before build resolution.",
            ),
            (
                "keywords",
                "Registry search keywords",
                "Terms used by the package registry.",
            ),
            (
                "provides",
                "Virtual slots",
                "Slots such as blas or cxx-stdlib used for conflict checks.",
            ),
        ]
    } else if section == "compiler" {
        vec![
            (
                "backend",
                "Compiler backend",
                "auto, gcc, clang, clang++, hipcc, or a custom template name.",
            ),
            ("warnings", "Warning level", "none, default, all, or error."),
            (
                "opt-level",
                "Optimization level",
                "Integer optimization level from 0 through 3.",
            ),
            ("debug", "Emit debug info", "Boolean debug-symbol toggle."),
            (
                "defines",
                "Project-wide preprocessor defines",
                "Array of defines injected into every compile.",
            ),
            (
                "flags",
                "Project-wide compiler flags",
                "Extra flags injected into every compile.",
            ),
            (
                "includes",
                "Project include directories",
                "Include directories added to every compile.",
            ),
            (
                "pch",
                "Precompiled header",
                "Header path compiled once and injected into supported languages.",
            ),
            (
                "unity",
                "Unity build toggle",
                "Combine C-family sources by language for faster full builds.",
            ),
        ]
    } else if section.contains("dependencies") {
        vec![
            (
                "name = \"*\"",
                "Version dependency",
                "Resolve an explicitly named package from configured resolvers.",
            ),
            (
                "name = { path = \"../lib\" }",
                "Local path dependency",
                "Include one local Freight package by manifest path.",
            ),
            (
                "name = { git = \"https://example/lib.git\" }",
                "Git dependency",
                "Fetch and build an explicitly named git package.",
            ),
            (
                "name = { url = \"https://example/lib.tar.gz\", sha256 = \"...\" }",
                "URL archive dependency",
                "Fetch and verify an explicitly named source archive.",
            ),
            (
                "name = { version = \"1.0\", repo = \"pkg-config\" }",
                "Pinned resolver dependency",
                "Use a specific resolver or registry channel.",
            ),
            (
                "features",
                "Dependency features",
                "Activate named features on this dependency.",
            ),
            (
                "default-features",
                "Default feature toggle",
                "Disable default dependency features when false.",
            ),
            (
                "optional",
                "Optional dependency",
                "Only active when selected by a feature.",
            ),
            (
                "os",
                "OS filter",
                "Include this dependency only on matching OS/family keys.",
            ),
            (
                "arch",
                "Architecture filter",
                "Include this dependency only on matching CPU architectures.",
            ),
            (
                "targets",
                "Target triple filter",
                "Include this prebuilt dependency only for matching triples.",
            ),
            (
                "type",
                "Foreign build type",
                "cmake, make, meson, autotools, scons, bazel, or none.",
            ),
            (
                "include",
                "Exported include dirs",
                "Include dirs exposed by a foreign dependency.",
            ),
            (
                "defines",
                "Build-system defines",
                "Configure defines (`KEY=VALUE`) applied per builder: cmake/meson `-D`, make `KEY=VALUE`.",
            ),
            (
                "patches",
                "Patch files",
                "Patch files applied after fetching.",
            ),
            (
                "channel",
                "Registry channel",
                "Fetch this dependency from a named channel.",
            ),
        ]
    } else if section.starts_with("language.") {
        vec![
            (
                "std",
                "Language standard",
                "Standard such as c17, c++20, f2018, or a compiler-template value.",
            ),
            (
                "stdlib",
                "C++ standard library selection",
                "libc++, libstdc++, or none for C++.",
            ),
        ]
    } else if section == "lib" {
        vec![
            ("type", "Library type", "static, shared, or header."),
            (
                "srcs",
                "Library sources",
                "Source path or array of source paths for this library target.",
            ),
            (
                "hdrs",
                "Public headers",
                "Headers whose parent dirs are exported to dependents.",
            ),
            (
                "link",
                "Prebuilt link name",
                "System/prebuilt library name passed to the linker.",
            ),
        ]
    } else if section == "bin" {
        vec![
            ("name", "Binary name", "Executable target name."),
            (
                "src",
                "Binary entry source",
                "Entry-point source file for this executable.",
            ),
        ]
    } else if section.starts_with("profile.") {
        vec![
            (
                "inherits",
                "Parent profile",
                "Inherit unset values from another named profile.",
            ),
            (
                "opt-level",
                "Optimization level",
                "Integer optimization level from 0 through 3.",
            ),
            (
                "debug",
                "Debug info toggle",
                "Emit debug information for this profile.",
            ),
            ("lto", "Link-time optimization", "Enable or disable LTO."),
            ("strip", "Strip symbols", "Strip final artifacts when true."),
            (
                "sanitize",
                "Sanitizers",
                "Array of sanitizer names for this profile.",
            ),
            (
                "features",
                "Profile features",
                "Features activated automatically for this profile.",
            ),
        ]
    } else if section == "target" {
        vec![
            (
                "arch",
                "CPU architecture",
                "Override host CPU architecture for target-specific settings.",
            ),
            (
                "cpu-extensions",
                "CPU extensions",
                "Array of CPU feature flags such as avx2 or fma.",
            ),
        ]
    } else if section == "formatter" || section == "linter" {
        vec![
            (
                "name",
                "Tool name",
                "Pin a formatter/linter instead of auto-detecting.",
            ),
            (
                "style",
                "Formatter style",
                "Common formatter setting resolved through the tool template.",
            ),
            (
                "checks",
                "Linter checks",
                "Common linter setting resolved through the tool template.",
            ),
        ]
    } else if section == "workspace" {
        vec![(
            "members",
            "Workspace members",
            "Relative paths to package directories.",
        )]
    } else if section.starts_with("os.") || section.starts_with("arch.") {
        vec![
            (
                "srcs",
                "Conditional sources",
                "Glob patterns included only when this OS/arch is active.",
            ),
            (
                "defines",
                "Conditional defines",
                "Defines injected only when this OS/arch is active.",
            ),
            (
                "flags",
                "Conditional flags",
                "Compiler flags injected only when this OS/arch is active.",
            ),
            (
                "includes",
                "Conditional includes",
                "Include paths injected only when this OS/arch is active.",
            ),
            (
                "features",
                "System libraries",
                "System libraries to link when this OS/arch is active (→ -l<lib>, macOS -framework, MSVC <name>.lib).",
            ),
            (
                "version",
                "Min OS/SDK version",
                "Minimum target OS/SDK version (Apple deployment target; -DFREIGHT_OS_VERSION).",
            ),
            (
                "dependencies",
                "Conditional dependencies",
                "Dependencies included only when this OS/arch is active.",
            ),
            (
                "language",
                "Conditional language settings",
                "Language overrides active only for this OS/arch.",
            ),
        ]
    } else {
        vec![
            (
                "[workspace]",
                "Workspace root",
                "Declare workspace member package paths.",
            ),
            (
                "[package]",
                "Package metadata",
                "Name, version, registry metadata, and package support gates.",
            ),
            (
                "[language.c]",
                "C language settings",
                "C standard and template-defined options.",
            ),
            (
                "[language.cpp]",
                "C++ language settings",
                "C++ standard, stdlib, and template-defined options.",
            ),
            (
                "[language.fortran]",
                "Fortran language settings",
                "Fortran standard and template-defined options.",
            ),
            (
                "[language.asm]",
                "Assembly language settings",
                "Assembler template-defined options.",
            ),
            (
                "[language.cuda]",
                "CUDA language settings",
                "CUDA standard/options when using CUDA sources.",
            ),
            (
                "[language.hip]",
                "HIP language settings",
                "HIP standard/options when using HIP sources.",
            ),
            (
                "[language.objc]",
                "Objective-C language settings",
                "Objective-C standard/options.",
            ),
            (
                "[language.objcpp]",
                "Objective-C++ language settings",
                "Objective-C++ standard/options.",
            ),
            ("[[bin]]", "Binary target", "Executable target entry point."),
            (
                "[lib]",
                "Library target",
                "Library artifact and exported headers.",
            ),
            (
                "[dependencies]",
                "Runtime dependencies",
                "Packages explicitly included in this build.",
            ),
            (
                "[build-dependencies]",
                "Build-time dependencies",
                "Tools fetched before regular build steps.",
            ),
            (
                "[dev-dependencies]",
                "Dev dependencies",
                "Debug/test-only dependencies.",
            ),
            (
                "[compiler]",
                "Compiler settings",
                "Backend, warnings, flags, includes, PCH, and unity settings.",
            ),
            ("[profile.dev]", "Debug profile", "Debug profile overrides."),
            (
                "[profile.release]",
                "Release profile",
                "Release profile overrides.",
            ),
            (
                "[features]",
                "Feature graph",
                "Feature names and dependency feature activation.",
            ),
            (
                "[target]",
                "CPU target settings",
                "Architecture and CPU extension settings.",
            ),
            (
                "[formatter]",
                "Formatter settings",
                "Project formatter requirements.",
            ),
            (
                "[linter]",
                "Linter settings",
                "Project linter requirements.",
            ),
            (
                "[os.linux]",
                "Linux-only settings",
                "Sources, defines, includes, deps, and language overrides for Linux.",
            ),
            (
                "[arch.x86_64]",
                "x86_64-only settings",
                "Sources, defines, includes, deps, and language overrides for x86_64.",
            ),
        ]
    };

    let mut items: Vec<Value> = labels
        .into_iter()
        .map(|(label, detail, docs)| {
            json!({
                "label": label, "kind": 10, "detail": detail,
                "documentation": { "kind": "markdown", "value": docs },
                "insertText": label
            })
        })
        .collect();
    items.extend(inventory_completion_items(&section, inventory));
    if section.contains("dependencies") {
        let existing: std::collections::HashSet<String> = items
            .iter()
            .filter_map(|i| i.get("label").and_then(Value::as_str).map(str::to_string))
            .collect();
        items.extend(system_package_completion_items(&existing));
    }
    json!({ "isIncomplete": false, "items": items })
}

/// `[dependencies]` completions for the common system libraries freight knows
/// about (the Tier-A header-ownership table): `zlib`, `sqlite3`, `openssl`, …
/// Inserts a snippet `name = "${1:version}"` with the cursor on the version, so
/// the user pins it (freight requires a concrete version — no bare `*`). The
/// version isn't pre-filled here to avoid a pkg-config probe per completion item;
/// the undeclared-include quick-fix does pin the installed version.
fn system_package_completion_items(existing: &std::collections::HashSet<String>) -> Vec<Value> {
    crate::build::header_ownership::load()
        .known_packages()
        .into_iter()
        .filter(|name| !existing.contains(name))
        .map(|name| {
            json!({
                "label": name,
                "kind": 9, // Module
                "detail": "Known system library",
                "documentation": {
                    "kind": "markdown",
                    "value": format!(
                        "Add a dependency on `{name}`. freight uses the version \
                         installed on the system if present, and downloads it from \
                         the registry otherwise. A concrete version is required."
                    )
                },
                "insertText": format!("{name} = \"${{1:version}}\""),
                "insertTextFormat": 2 // Snippet
            })
        })
        .collect()
}

fn inventory_completion_items(section: &str, inventory: Option<&WorkspaceInventory>) -> Vec<Value> {
    let Some(inventory) = inventory else {
        return vec![];
    };
    if section == "workspace" {
        return inventory
            .packages
            .iter()
            .map(|pkg| {
                json!({
                    "label": pkg.path,
                    "kind": 18,
                    "detail": format!("Workspace package {}", pkg.name),
                    "documentation": {
                        "kind": "markdown",
                        "value": format!("Workspace member package `{}`.", pkg.name)
                    },
                    "insertText": format!("\"{}\"", pkg.path)
                })
            })
            .collect();
    }
    if section.contains("dependencies") {
        return inventory
            .packages
            .iter()
            .filter(|pkg| pkg.lib.is_some())
            .map(|pkg| {
                json!({
                    "label": pkg.name,
                    "kind": 9,
                    "detail": format!("Workspace library at {}", pkg.path),
                    "documentation": {
                        "kind": "markdown",
                        "value": format!("Add explicit path dependency on workspace library `{}`.", pkg.name)
                    },
                    "insertText": format!("{} = {{ path = \"{}\" }}", pkg.name, pkg.path)
                })
            })
            .collect();
    }
    vec![]
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

pub fn hover_result(text: Option<&str>, pos: Option<(usize, usize)>) -> Option<Value> {
    let (text, (line, character)) = text.zip(pos)?;
    let section = current_section(text, line).unwrap_or_default();
    let line_text = text.lines().nth(line).unwrap_or("").trim();
    let key = key_at_position(line_text, character);
    let value = if let Some(value) = key.and_then(hover_for_key) {
        value
    } else if section.contains("dependencies") || line_text == "[dependencies]" {
        "Dependencies are explicit in Freight. Headers and link flags are included only when the package is listed in `freight.toml` and active for the current OS, architecture, target, profile, and feature set."
    } else if section == "lib" || line_text == "[lib]" {
        "`[lib]` declares this package's library artifact. Dependents only see headers listed in `hdrs` or discovered include/inc directories from this package."
    } else if line_text == "[[bin]]" || section == "bin" {
        "`[[bin]]` declares an executable entry point. Freight links shared project sources, but avoids linking another target's `main`."
    } else if section.starts_with("language.") {
        "`[language.*]` configures a language that Freight detects from source extensions. Standards are checked against detected compiler templates."
    } else if section.starts_with("profile.") {
        "`[profile.*]` overrides compiler/build settings for one named profile. Custom profiles can inherit from `dev`, `release`, or another custom profile."
    } else if section == "target" {
        "`[target]` controls CPU architecture and extension settings used by compiler templates and assembly output."
    } else if section == "formatter" {
        "`[formatter]` pins project formatting requirements while still allowing tool templates to define supported settings."
    } else if section == "linter" {
        "`[linter]` pins project lint requirements while still allowing tool templates to define supported settings."
    } else if section == "workspace" {
        "`[workspace]` marks this manifest as a workspace root. It contains member package paths and no `[package]` section."
    } else if section.starts_with("os.") || section.starts_with("arch.") {
        "`[os.*]` and `[arch.*]` sections add sources, flags, includes, language overrides, and dependencies only when that platform key is active."
    } else if section == "compiler" {
        "`[compiler]` controls backend selection, warnings, cross-compilation target/sysroot, defines, includes, and extra flags."
    } else if line_text == "[package]" || section == "package" {
        "`[package]` names the package, version, metadata, and optional `supports` expression used before building."
    } else {
        return None;
    };
    Some(json!({ "contents": { "kind": "markdown", "value": value } }))
}

fn key_at_position(line_text: &str, character: usize) -> Option<&str> {
    let before_comment = line_text.split('#').next()?.trim();
    let key = before_comment.split('=').next()?.trim();
    if key.is_empty() || character > before_comment.len().saturating_add(1) {
        return None;
    }
    Some(key.trim_matches('"'))
}

fn hover_for_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "name" => "Name field. In `[package]` this is the package identity; in `[[bin]]` it is the executable target name.",
        "version" => "Version requirement or package SemVer version, depending on context.",
        "path" => "`path` dependencies include exactly the local Freight package at that path. Freight does not scan sibling directories automatically.",
        "git" => "`git` dependencies fetch exactly this repository. Use `branch`, `tag`, or `rev` to control the checked-out ref.",
        "url" => "`url` dependencies fetch exactly this archive. Pair with `sha256` for reproducible fetches.",
        "sha256" => "Expected SHA-256 for a URL archive. Freight rejects the archive when the digest does not match.",
        "registry" => "Resolver override for a version dependency, such as `pkg-config`, `system`, or a named registry.",
        "features" => "Feature list. In a dependency this activates dependency features; in a profile it activates project features for that profile.",
        "default-features" => "Set to `false` to disable a dependency's default feature set.",
        "optional" => "Optional dependencies are available to features but are not included unless selected.",
        "os" => "OS/family filter for a dependency. Supported family keys include `unix`, `bsd`, `linux`, `windows`, and `macos`.",
        "arch" => "CPU architecture filter or target override. Values mirror Rust target architecture names such as `x86_64` and `aarch64`.",
        "targets" => "Target triple allowlist for a dependency, mainly for prebuilts and cross-compilation.",
        "type" => "Artifact or foreign build type depending on context: library `static/shared/header`, or dependency `cmake/make/meson/autotools/scons/bazel/none`.",
        "include" => "Include directories exported by a foreign dependency, relative to that dependency's source root.",
        "includes" => "Include directories added to compiler invocations in the current section.",
        "patches" => "Patch files applied after fetching a dependency, in order.",
        "channel" => "Registry channel used for this dependency, such as `stable` or `experimental`.",
        "std" => "Language standard checked against the detected compiler template before compilation.",
        "stdlib" => "C++ standard library selection. Supported values are `libc++`, `libstdc++`, and `none`.",
        "srcs" => "Source files or glob patterns, depending on the section.",
        "hdrs" => "Public headers exported by a library target to packages that explicitly depend on it.",
        "link" => "Prebuilt or system library name passed to the linker.",
        "backend" => "Compiler backend selection. `auto` lets Freight choose the first available matching template.",
        "warnings" => "Warning policy: `none`, `default`, `all`, or `error`.",
        "opt-level" => "Optimization level from 0 through 3.",
        "debug" => "Emit debug information when true.",
        "defines" => "In `[compiler]`/sections: preprocessor defines for compilation. On a foreign dependency: build-system configure defines (`KEY=VALUE`), applied in each builder's native form (cmake/meson `-D`, make `KEY=VALUE`).",
        "flags" => "Compiler flags injected in the current section.",
        "pch" => "Header path to precompile once and inject into supported language compilations.",
        "unity" => "Enable or disable C-family unity builds.",
        "inherits" => "Parent profile whose unset values are inherited by this profile.",
        "lto" => "Enable or disable link-time optimization.",
        "strip" => "Strip symbols from final artifacts when true.",
        "sanitize" => "Sanitizer names enabled for this profile.",
        "cpu-extensions" => "CPU extensions such as `avx2` or `fma` converted through compiler templates.",
        "members" => "Workspace member directories relative to the workspace root.",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Signature help
// ---------------------------------------------------------------------------

pub fn signature_help_result(text: Option<&str>, pos: Option<(usize, usize)>) -> Option<Value> {
    let (text, (line, character)) = text.zip(pos)?;
    let section = current_section(text, line).unwrap_or_default();
    let full_line = text.lines().nth(line).unwrap_or("");
    let line_until_pos = full_line.get(..character).unwrap_or(full_line);
    let spec = signature_spec_for_context(&section, line_until_pos)?;
    let active_parameter = active_parameter_for_signature(line_until_pos, spec.params);
    Some(render_signature_help(
        spec.label,
        spec.params,
        active_parameter,
        spec.documentation,
    ))
}

struct SignatureSpec {
    label: &'static str,
    params: &'static [(&'static str, &'static str)],
    documentation: &'static str,
}

const PACKAGE_PARAMS: &[(&str, &str)] = &[
    (
        "name",
        "Package name used by builds, dependencies, and the registry.",
    ),
    ("version", "SemVer package version."),
    ("authors", "Package authors."),
    ("description", "Short registry/package description."),
    ("license", "SPDX license expression."),
    ("readme", "Relative README path."),
    ("repository", "Source repository URL."),
    ("keywords", "Registry search keywords."),
    ("supports", "Boolean platform support expression."),
    ("provides", "Virtual slots this package fills."),
];

const DEPENDENCY_PARAMS: &[(&str, &str)] = &[
    (
        "version",
        "Version requirement resolved from pkg-config, system stubs, or a registry.",
    ),
    ("path", "Explicit local Freight package path."),
    ("git", "Explicit git repository URL."),
    ("branch", "Git branch to check out."),
    ("tag", "Git tag to check out."),
    ("rev", "Pinned git revision."),
    ("url", "Explicit source archive URL."),
    ("sha256", "Expected SHA-256 digest for a URL archive."),
    (
        "repo",
        "Resolver override such as pkg-config, system, or a named registry.",
    ),
    ("features", "Dependency features to activate."),
    (
        "default-features",
        "Whether default dependency features are active.",
    ),
    (
        "optional",
        "Whether this dependency is only enabled through features.",
    ),
    ("os", "OS or OS-family allowlist."),
    ("arch", "CPU architecture allowlist."),
    ("targets", "Target triple allowlist."),
    ("type", "Foreign build type."),
    (
        "include",
        "Foreign dependency include dirs exported to dependents.",
    ),
    ("defines", "Build-system configure defines (KEY=VALUE)."),
    ("patches", "Patch files applied after fetching."),
    ("unity", "Override unity builds for this dependency."),
    ("channel", "Registry channel to use."),
];

const LANGUAGE_PARAMS: &[(&str, &str)] = &[
    (
        "std",
        "Language standard checked against the active compiler template.",
    ),
    ("stdlib", "C++ standard library selection."),
];
const LIB_PARAMS: &[(&str, &str)] = &[
    ("type", "Library artifact type: static, shared, or header."),
    ("srcs", "Library source file or source list."),
    ("hdrs", "Public headers exported to dependents."),
    (
        "link",
        "Prebuilt or system library name passed to the linker.",
    ),
];
const BIN_PARAMS: &[(&str, &str)] = &[
    ("name", "Executable target name."),
    ("src", "Executable entry source."),
];
const COMPILER_PARAMS: &[(&str, &str)] = &[
    ("backend", "Compiler backend name or auto."),
    ("opt-level", "Optimization level from 0 through 3."),
    ("debug", "Emit debug information."),
    ("warnings", "Warning policy."),
    ("defines", "Project-wide defines."),
    ("flags", "Project-wide compiler flags."),
    ("includes", "Project-wide include paths."),
    ("pch", "Precompiled header path."),
    ("unity", "Enable C-family unity builds."),
];
const PROFILE_PARAMS: &[(&str, &str)] = &[
    ("inherits", "Parent profile for inherited unset values."),
    ("opt-level", "Optimization level from 0 through 3."),
    ("debug", "Emit debug information."),
    ("lto", "Enable link-time optimization."),
    ("strip", "Strip final artifacts."),
    ("sanitize", "Sanitizers enabled for this profile."),
    ("features", "Features activated by this profile."),
];
const TARGET_PARAMS: &[(&str, &str)] = &[
    ("arch", "Target CPU architecture."),
    ("cpu-extensions", "CPU extensions such as avx2 or fma."),
];
const CONDITIONAL_PARAMS: &[(&str, &str)] = &[
    ("srcs", "Platform-specific source globs."),
    ("defines", "Platform-specific defines."),
    ("flags", "Platform-specific compiler flags."),
    ("includes", "Platform-specific include paths."),
    (
        "features",
        "System libraries to link on this platform (→ -l<lib>, macOS -framework, MSVC <name>.lib).",
    ),
    (
        "version",
        "Minimum target OS/SDK version (Apple deployment target; -DFREIGHT_OS_VERSION).",
    ),
    ("dependencies", "Platform-specific dependency table."),
    ("language", "Platform-specific language overrides."),
];
const WORKSPACE_PARAMS: &[(&str, &str)] = &[("members", "Relative member package paths.")];
const TOOL_PARAMS: &[(&str, &str)] = &[
    ("name", "Tool name to prefer."),
    ("style", "Formatter style setting."),
    ("checks", "Linter checks setting."),
];

fn signature_spec_for_context(section: &str, line_until_pos: &str) -> Option<SignatureSpec> {
    if section.contains("dependencies") || inline_table_key(line_until_pos).is_some() {
        return Some(SignatureSpec {
            label: "freight::dependency { semver version, path path, url git, string branch, string tag, string rev, url url, sha256 sha256, resolver repo, string[] features, bool default-features, bool optional, os[] os, arch[] arch, triple[] targets, build type, path[] include, string[] defines, path[] patches, bool unity, string channel }",
            params: DEPENDENCY_PARAMS,
            documentation: "Freight dependency table. Only explicitly listed, active dependencies contribute headers and link flags.",
        });
    }
    let (label, params, documentation) = if section == "package" {
        ("freight::package { string name, semver version, string[] authors, string description, spdx license, path readme, url repository, string[] keywords, expr supports, string[] provides }", PACKAGE_PARAMS, "Package metadata used by builds and the registry.")
    } else if section.starts_with("language.") {
        (
            "freight::language { standard std, cxx-stdlib stdlib }",
            LANGUAGE_PARAMS,
            "Language settings for the active compiler template.",
        )
    } else if section == "lib" {
        (
            "freight::lib { lib-kind type, path[] srcs, path[] hdrs, string link }",
            LIB_PARAMS,
            "Library target declaration.",
        )
    } else if section == "bin" {
        (
            "freight::bin { string name, path src }",
            BIN_PARAMS,
            "Executable target declaration.",
        )
    } else if section == "compiler" {
        ("freight::compiler { backend backend, int opt-level, bool debug, warning-level warnings, string[] defines, string[] flags, path[] includes, path pch, bool unity }", COMPILER_PARAMS, "Compiler settings applied before profile and platform overlays.")
    } else if section.starts_with("profile.") {
        ("freight::profile { string inherits, int opt-level, bool debug, bool lto, bool strip, string[] sanitize, string[] features }", PROFILE_PARAMS, "Build profile overrides.")
    } else if section == "target" {
        (
            "freight::target { arch arch, string[] cpu-extensions }",
            TARGET_PARAMS,
            "CPU target settings.",
        )
    } else if section.starts_with("os.") || section.starts_with("arch.") {
        ("freight::platform { path[] srcs, string[] defines, string[] flags, path[] includes, table dependencies, table language }", CONDITIONAL_PARAMS, "OS or architecture conditional overlay.")
    } else if section == "workspace" {
        (
            "freight::workspace { path[] members }",
            WORKSPACE_PARAMS,
            "Workspace root manifest.",
        )
    } else if section == "formatter" || section == "linter" {
        (
            "freight::tool { string name, string style, string checks }",
            TOOL_PARAMS,
            "Formatter or linter settings resolved through tool templates.",
        )
    } else {
        return None;
    };
    Some(SignatureSpec {
        label,
        params,
        documentation,
    })
}

fn render_signature_help(
    label: &str,
    params: &[(&str, &str)],
    active_parameter: usize,
    documentation: &str,
) -> Value {
    let parameters: Vec<Value> = params
        .iter()
        .map(|(name, doc)| {
            let range = parameter_label_range(label, name)
                .map(|(s, e)| json!([s, e]))
                .unwrap_or_else(|| json!(name));
            json!({ "label": range, "documentation": { "kind": "markdown", "value": *doc } })
        })
        .collect();
    let active = active_parameter.min(params.len().saturating_sub(1));
    json!({
        "signatures": [{
            "label": label,
            "documentation": { "kind": "markdown", "value": documentation },
            "parameters": parameters,
            "activeParameter": active
        }],
        "activeSignature": 0,
        "activeParameter": active
    })
}

fn parameter_label_range(label: &str, param: &str) -> Option<(usize, usize)> {
    let start = label.find(param)?;
    Some((start, start + param.len()))
}

fn active_parameter_for_signature(line_until_pos: &str, params: &[(&str, &str)]) -> usize {
    if let Some(key) = inline_table_key(line_until_pos) {
        return params
            .iter()
            .position(|(n, _)| *n == key)
            .unwrap_or_else(|| comma_count_after_open_brace(line_until_pos));
    }
    let key = line_until_pos
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    params.iter().position(|(n, _)| *n == key).unwrap_or(0)
}

fn inline_table_key(line_until_pos: &str) -> Option<&str> {
    let open = line_until_pos.rfind('{')?;
    let key = line_until_pos[open + 1..]
        .rsplit(',')
        .next()
        .unwrap_or("")
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn comma_count_after_open_brace(line_until_pos: &str) -> usize {
    let Some(open) = line_until_pos.rfind('{') else {
        return 0;
    };
    line_until_pos[open + 1..]
        .chars()
        .filter(|ch| *ch == ',')
        .count()
}

pub fn current_section(text: &str, line: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().take(line + 1).collect();
    lines.into_iter().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with("[[") && trimmed.ends_with("]]") {
            return Some(trimmed.trim_matches(['[', ']']).to_string());
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return Some(trimmed.trim_matches(['[', ']']).to_string());
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_manifest_does_not_require_package_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let member = tmp.path().join("core");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            member.join("freight.toml"),
            r#"
[package]
name = "core"
version = "0.1.0"

[language.c]
std = "c17"

[lib]
type = "header"
hdrs = ["include/core.h"]
"#,
        )
        .unwrap();

        let diagnostics = manifest_diagnostics(
            r#"
[workspace]
members = ["core"]
"#,
            tmp.path(),
            &[],
        );

        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    #[test]
    fn workspace_manifest_reports_missing_member_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let diagnostics = manifest_diagnostics(
            r#"
[workspace]
members = ["missing"]
"#,
            tmp.path(),
            &[],
        );

        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("does not contain a freight.toml"));
    }

    #[test]
    fn dependency_completion_includes_workspace_library_paths() {
        let inventory = WorkspaceInventory {
            packages: vec![
                WorkspacePackage {
                    name: "core".to_string(),
                    path: "core".to_string(),
                    bins: vec![],
                    lib: Some("Static".to_string()),
                },
                WorkspacePackage {
                    name: "app".to_string(),
                    path: "app".to_string(),
                    bins: vec!["demo".to_string()],
                    lib: None,
                },
            ],
        };
        let result = completion_result(
            Some(
                r#"
[dependencies]
"#,
            ),
            Some((2, 0)),
            Some(&inventory),
        );
        let items = result["items"].as_array().unwrap();

        assert!(items.iter().any(|item| {
            item["label"] == json!("core")
                && item["insertText"] == json!("core = { path = \"core\" }")
        }));
        assert!(!items.iter().any(|item| item["label"] == json!("app")));
    }

    #[test]
    fn dependency_completion_offers_known_system_packages() {
        let result = completion_result(Some("[dependencies]\n"), Some((1, 0)), None);
        let items = result["items"].as_array().unwrap();
        // zlib is in the Tier-A ownership seed; offered with a bare-version insert.
        let zlib = items
            .iter()
            .find(|i| i["label"] == json!("zlib"))
            .expect("zlib offered as a known system package");
        assert_eq!(zlib["insertText"], json!("zlib = \"${1:version}\""));
        assert_eq!(zlib["insertTextFormat"], json!(2));
        assert_eq!(zlib["detail"], json!("Known system library"));
    }

    #[test]
    fn system_package_completion_skips_already_offered_names() {
        let existing = ["zlib".to_string()].into_iter().collect();
        let items = system_package_completion_items(&existing);
        assert!(
            !items.iter().any(|i| i["label"] == json!("zlib")),
            "an already-offered name must not be duplicated"
        );
    }
}
