/// Import an Autotools project (configure.ac + optional Makefile.am) into freight.
///
/// Parsing strategy (static only — no shell execution):
///   1. `configure.ac` / `configure.in` → package name, version, dependencies
///   2. `Makefile.am`                    → targets (bin_PROGRAMS, lib_LIBRARIES, …)
///   3. `Makefile.in` fallback          → treat like a Makefile after stripping @...@ tokens
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;
use toml_edit::{Array, DocumentMut, Item, Table, value};

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ImportResult {
    pub written: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Purge artefacts left by autotools from `dir`.
pub fn purge_autotools(dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let files = [
        "configure", "configure.ac", "configure.in", "config.status",
        "config.guess", "config.sub", "config.log",
        "Makefile.am", "Makefile.in", "Makefile",
        "aclocal.m4", "install-sh", "missing", "depcomp", "compile",
        "ltmain.sh", "libtool",
    ];
    for name in &files {
        let p = dir.join(name);
        if p.exists() {
            if std::fs::remove_file(&p).is_ok() {
                removed.push(format!("removed {}", p.display()));
            }
        }
    }
    // autom4te.cache directory
    let cache = dir.join("autom4te.cache");
    if cache.is_dir() {
        if std::fs::remove_dir_all(&cache).is_ok() {
            removed.push(format!("removed {}/", cache.display()));
        }
    }
    removed
}

pub fn import_autotools(input: &Path, out_dir: Option<&Path>) -> Result<ImportResult> {
    let project_dir = if input.is_dir() {
        input.to_path_buf()
    } else {
        input.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let out_root = out_dir.unwrap_or(&project_dir);
    let mut warnings: Vec<String> = Vec::new();

    // ── 1. Parse configure.ac / configure.in ─────────────────────────────────
    let dir_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let configure_ac = find_configure_ac(&project_dir);
    let (parsed_name, pkg_version, mut deps) = match &configure_ac {
        Some(path) => {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            parse_configure_ac(&content, &mut warnings)
        }
        None => {
            warnings.push("No configure.ac / configure.in found — using defaults".into());
            (String::new(), "0.1.0".to_string(), vec![])
        }
    };
    // Prefer explicit name from AC_INIT; fall back to directory name
    let pkg_name = if parsed_name.is_empty() || parsed_name == "project" {
        sanitize_name(&dir_name)
    } else {
        parsed_name
    };

    // ── 2. Parse Makefile.am (preferred) or Makefile.in (fallback) ───────────
    let (targets, lang_std, extra_defines) =
        if let Some(am) = find_file(&project_dir, "Makefile.am") {
            let content = std::fs::read_to_string(&am)
                .with_context(|| format!("reading {}", am.display()))?;
            parse_makefile_am(&content, &mut warnings)
        } else if let Some(mf_in) = find_file(&project_dir, "Makefile.in") {
            let content = std::fs::read_to_string(&mf_in)
                .with_context(|| format!("reading {}", mf_in.display()))?;
            warnings.push("No Makefile.am found — falling back to Makefile.in".into());
            parse_makefile_in(&content, &mut warnings)
        } else {
            warnings.push("No Makefile.am or Makefile.in — inferring from filesystem".into());
            let kind = if has_main_function(&project_dir) {
                TargetKind::Bin
            } else {
                TargetKind::StaticLib
            };
            let name = sanitize_name(&pkg_name);
            (vec![TargetSpec { name, kind }], None, vec![])
        };

    // Filter well-known auto-linked libs (pkg-config finds them anyway)
    deps.retain(|d| !AUTO_LINKED.contains(&d.as_str()));

    // ── 3. Emit freight.toml ──────────────────────────────────────────────────
    let toml = emit_toml(
        &pkg_name, &pkg_version, &targets, lang_std.as_deref(),
        &extra_defines, &deps, &warnings,
    );

    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;
    let dest = out_root.join("freight.toml");
    std::fs::write(&dest, &toml)
        .with_context(|| format!("writing {}", dest.display()))?;

    Ok(ImportResult { written: vec![dest], warnings })
}

// ── configure.ac parser ───────────────────────────────────────────────────────

fn find_configure_ac(dir: &Path) -> Option<PathBuf> {
    for name in &["configure.ac", "configure.in"] {
        let p = dir.join(name);
        if p.exists() { return Some(p); }
    }
    None
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.exists() { Some(p) } else { None }
}

fn parse_configure_ac(content: &str, warnings: &mut Vec<String>) -> (String, String, Vec<String>) {
    let mut name = String::new();
    let mut version = String::from("0.1.0");
    let mut deps: Vec<String> = Vec::new();

    // AC_INIT([name], [version]) or AC_INIT(name, version)
    let ac_init = Regex::new(r"AC_INIT\(\s*\[?([^\],\)]+)\]?\s*,\s*\[?([^\],\)]+)\]?").unwrap();
    if let Some(cap) = ac_init.captures(content) {
        let n = sanitize_name(cap[1].trim());
        let v = cap[2].trim().to_string();
        if !n.is_empty() { name = n; }
        if !v.is_empty() { version = v; }
    }

    // PKG_CHECK_MODULES(PREFIX, module [>= ver])
    let pkg_check = Regex::new(r"PKG_CHECK_MODULES(?:_OPTIONAL)?\s*\(\s*\w+\s*,\s*([^\)]+)\)").unwrap();
    for cap in pkg_check.captures_iter(content) {
        // May be "foo >= 1.0" or "foo bar" (multiple)
        for part in cap[1].split_whitespace() {
            let part = part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            if part.is_empty() || part.starts_with(|c: char| c.is_ascii_digit())
                || matches!(part, ">=" | "<=" | ">" | "<" | "=")
            {
                continue;
            }
            if !deps.contains(&part.to_string()) {
                deps.push(part.to_string());
            }
        }
    }

    // AC_CHECK_LIB(name, function)
    let check_lib = Regex::new(r"AC_CHECK_LIB\(\s*\[?(\w[\w-]*)\]?\s*,").unwrap();
    for cap in check_lib.captures_iter(content) {
        let lib = cap[1].to_string();
        if !deps.contains(&lib) {
            deps.push(lib);
        }
    }

    // Warn about things we can't convert
    if content.contains("AC_CHECK_HEADERS") {
        warnings.push("AC_CHECK_HEADERS detected — review include paths manually".into());
    }

    (name, version, deps)
}

// ── Makefile.am parser ────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Clone, Copy)]
enum TargetKind { Bin, StaticLib, SharedLib }

#[derive(Debug)]
struct TargetSpec { name: String, kind: TargetKind }

fn parse_makefile_am(
    content: &str,
    warnings: &mut Vec<String>,
) -> (Vec<TargetSpec>, Option<String>, Vec<String>) {
    let mut targets: Vec<TargetSpec> = Vec::new();
    let mut lang_std: Option<String> = None;
    let mut defines: Vec<String> = Vec::new();

    // Join continuation lines
    let joined = join_continuations(content);

    for line in joined.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        // bin_PROGRAMS / noinst_PROGRAMS / check_PROGRAMS / sbin_PROGRAMS / libexec_PROGRAMS
        if let Some(rest) = strip_lhs(line, &["bin_PROGRAMS", "sbin_PROGRAMS",
            "libexec_PROGRAMS", "noinst_PROGRAMS", "check_PROGRAMS",
            "dist_bin_SCRIPTS", "noinst_PROGRAMS"])
        {
            for name in rest.split_whitespace() {
                let n = sanitize_name(name);
                if !n.is_empty() && !targets.iter().any(|t| t.name == n) {
                    targets.push(TargetSpec { name: n, kind: TargetKind::Bin });
                }
            }
        }
        // lib_LIBRARIES / noinst_LIBRARIES (static)
        else if let Some(rest) = strip_lhs(line, &["lib_LIBRARIES", "noinst_LIBRARIES",
            "pkglib_LIBRARIES", "check_LIBRARIES"])
        {
            for name in rest.split_whitespace() {
                let n = sanitize_name(strip_lib_ext(name));
                if !n.is_empty() && !targets.iter().any(|t| t.name == n) {
                    targets.push(TargetSpec { name: n, kind: TargetKind::StaticLib });
                }
            }
        }
        // lib_LTLIBRARIES / noinst_LTLIBRARIES (libtool → shared)
        else if let Some(rest) = strip_lhs(line, &["lib_LTLIBRARIES", "noinst_LTLIBRARIES",
            "pkglib_LTLIBRARIES"])
        {
            for name in rest.split_whitespace() {
                // .la → treat as shared lib
                let stripped = name.strip_suffix(".la").unwrap_or(name);
                let n = sanitize_name(strip_lib_ext(stripped));
                if !n.is_empty() && !targets.iter().any(|t| t.name == n) {
                    targets.push(TargetSpec { name: n, kind: TargetKind::SharedLib });
                }
            }
        }
        // AM_CFLAGS / AM_CXXFLAGS
        else if let Some(flags) = strip_lhs(line, &["AM_CFLAGS", "AM_CXXFLAGS",
            "AM_CPPFLAGS", "CFLAGS", "CXXFLAGS"])
        {
            if lang_std.is_none() {
                lang_std = extract_std(flags);
            }
            for tok in flags.split_whitespace() {
                if let Some(def) = tok.strip_prefix("-D") {
                    if !def.is_empty() { defines.push(def.to_string()); }
                }
            }
        }
        // SUBDIRS — warn about them
        else if line.starts_with("SUBDIRS") {
            warnings.push("Makefile.am has SUBDIRS — workspace members not yet auto-detected".into());
        }
    }

    // If nothing found, fall back
    if targets.is_empty() {
        warnings.push("No target declarations found in Makefile.am".into());
    }

    (targets, lang_std, defines)
}

// ── Makefile.in fallback parser ───────────────────────────────────────────────

fn parse_makefile_in(
    content: &str,
    warnings: &mut Vec<String>,
) -> (Vec<TargetSpec>, Option<String>, Vec<String>) {
    // Strip @VARIABLE@ tokens (autoconf substitutions) to reduce parse noise
    let cleaned = Regex::new(r"@[A-Z_]+@").unwrap()
        .replace_all(content, "")
        .into_owned();

    let mut targets: Vec<TargetSpec> = Vec::new();
    let mut lang_std: Option<String> = None;
    let mut defines: Vec<String> = Vec::new();

    let joined = join_continuations(&cleaned);

    // Look for STATICLIB / SHAREDLIB / PROGRAMS variable patterns
    for line in joined.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if let Some(rest) = strip_lhs(line, &["STATICLIB", "STATIC_LIB"]) {
            for name in rest.split_whitespace() {
                let n = sanitize_name(strip_lib_ext(name));
                if !n.is_empty() {
                    targets.push(TargetSpec { name: n, kind: TargetKind::StaticLib });
                }
            }
        } else if let Some(rest) = strip_lhs(line, &["SHAREDLIB", "SHARED_LIB", "SHAREDLIBV", "SHAREDLIBM"]) {
            // Only take the first one
            if targets.iter().all(|t| t.kind != TargetKind::SharedLib) {
                if let Some(name) = rest.split_whitespace().next() {
                    let n = sanitize_name(strip_lib_ext(name));
                    if !n.is_empty() {
                        targets.push(TargetSpec { name: n, kind: TargetKind::SharedLib });
                    }
                }
            }
        } else if let Some(rest) = strip_lhs(line, &["PROGRAMS", "BINS", "BIN"]) {
            for name in rest.split_whitespace() {
                let n = sanitize_name(name);
                if !n.is_empty() {
                    targets.push(TargetSpec { name: n, kind: TargetKind::Bin });
                }
            }
        } else if let Some(flags) = strip_lhs(line, &["CFLAGS", "CXXFLAGS", "CPPFLAGS"]) {
            if lang_std.is_none() { lang_std = extract_std(flags); }
            for tok in flags.split_whitespace() {
                if let Some(def) = tok.strip_prefix("-D") {
                    if !def.is_empty() { defines.push(def.to_string()); }
                }
            }
        }
    }

    if targets.is_empty() {
        warnings.push("Could not determine targets from Makefile.in — check output manually".into());
    }

    (targets, lang_std, defines)
}

// ── TOML emitter ──────────────────────────────────────────────────────────────

fn emit_toml(
    name: &str,
    version: &str,
    targets: &[TargetSpec],
    lang_std: Option<&str>,
    defines: &[String],
    deps: &[String],
    warnings: &[String],
) -> String {
    let mut doc = DocumentMut::new();

    let header: String = warnings
        .iter()
        .map(|w| format!("# warning: {w}\n"))
        .collect::<String>();
    let header = format!("# Generated by freight migrate autotools — review before committing.\n{header}");

    let mut pkg = Table::new();
    pkg["name"]    = value(name);
    pkg["version"] = value(version);
    doc["package"] = Item::Table(pkg);

    if let Some(std) = lang_std {
        let lang_key = if std.starts_with("c++") || std.starts_with("gnu++") { "c++" } else { "c" };
        let mut lang_tbl = Table::new();
        lang_tbl["std"] = value(std);
        let mut lang_outer = Table::new();
        lang_outer[lang_key] = Item::Table(lang_tbl);
        doc["language"] = Item::Table(lang_outer);
    }

    if !defines.is_empty() {
        let mut build_tbl = Table::new();
        let mut arr = Array::new();
        for d in defines { arr.push(d.as_str()); }
        build_tbl["defines"] = value(arr);
        doc["build"] = Item::Table(build_tbl);
    }

    if !deps.is_empty() {
        let mut dep_tbl = Table::new();
        for d in deps {
            dep_tbl[d.as_str()] = value("*");
        }
        doc["dependencies"] = Item::Table(dep_tbl);
    }

    for t in targets {
        let section = match t.kind {
            TargetKind::Bin => "bin",
            TargetKind::StaticLib | TargetKind::SharedLib => "lib",
        };
        let mut tbl = Table::new();
        tbl["name"] = value(t.name.as_str());
        if t.kind == TargetKind::SharedLib {
            tbl["type"] = value("shared");
        } else if t.kind == TargetKind::StaticLib {
            tbl["type"] = value("static");
        }
        let arr = doc[section].or_insert(Item::ArrayOfTables(Default::default()));
        if let Item::ArrayOfTables(aot) = arr {
            aot.push(tbl);
        }
    }

    // Prepend header comment manually (toml_edit doesn't expose file-level comments)
    format!("{header}{doc}")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Libraries that pkg-config / freight finds automatically — don't emit as deps.
const AUTO_LINKED: &[&str] = &[
    "m", "c", "gcc", "gcc_s", "stdc++", "dl", "rt", "pthread",
    "resolv", "nsl", "socket", "util",
];

fn join_continuations(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut iter = content.lines().peekable();
    while let Some(line) = iter.next() {
        if line.ends_with('\\') {
            out.push_str(line.trim_end_matches('\\'));
            out.push(' ');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Strip a leading `VAR =` or `VAR +=` or `VAR :=` prefix; return the RHS if matched.
fn strip_lhs<'a>(line: &'a str, names: &[&str]) -> Option<&'a str> {
    for name in names {
        if let Some(rest) = line.strip_prefix(name) {
            let rest = rest.trim_start();
            if let Some(rhs) = rest.strip_prefix("+=")
                .or_else(|| rest.strip_prefix(":="))
                .or_else(|| rest.strip_prefix('='))
            {
                return Some(rhs.trim());
            }
        }
    }
    None
}

fn extract_std(flags: &str) -> Option<String> {
    for tok in flags.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("-std=") {
            let s = rest.replace("gnu++", "c++").replace("gnu", "c");
            return Some(s);
        }
    }
    None
}

fn strip_lib_ext(name: &str) -> &str {
    let name = name.strip_suffix(".la").unwrap_or(name);
    let name = name.strip_suffix(".a").unwrap_or(name);
    let name = if let Some(p) = name.find(".so") { &name[..p] } else { name };
    name.strip_prefix("lib").unwrap_or(name)
}

fn sanitize_name(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

fn has_main_function(dir: &Path) -> bool {
    use walkdir::WalkDir;
    let exts = ["c", "cpp", "cc", "cxx", "C"];
    for entry in WalkDir::new(dir).max_depth(3).into_iter().flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !exts.contains(&ext) { continue; }
        if let Ok(content) = std::fs::read_to_string(path) {
            if content.contains("int main(") || content.contains("int main (") {
                return true;
            }
        }
    }
    false
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ac_init_bracket_style() {
        let content = "AC_INIT([mylib], [1.2.3], [bugs@example.com])";
        let mut w = vec![];
        let (name, ver, _) = parse_configure_ac(content, &mut w);
        assert_eq!(name, "mylib");
        assert_eq!(ver, "1.2.3");
    }

    #[test]
    fn parse_ac_init_bare_style() {
        let content = "AC_INIT(myapp, 2.0)";
        let mut w = vec![];
        let (name, ver, _) = parse_configure_ac(content, &mut w);
        assert_eq!(name, "myapp");
        assert_eq!(ver, "2.0");
    }

    #[test]
    fn parse_pkg_check_modules() {
        let content = "PKG_CHECK_MODULES(SSL, openssl >= 1.0 libcurl)\n";
        let mut w = vec![];
        let (_, _, deps) = parse_configure_ac(content, &mut w);
        assert!(deps.contains(&"openssl".to_string()));
        assert!(deps.contains(&"libcurl".to_string()));
    }

    #[test]
    fn parse_ac_check_lib() {
        let content = "AC_CHECK_LIB([ssl], [SSL_new])\nAC_CHECK_LIB(m, sin)\n";
        let mut w = vec![];
        let (_, _, deps) = parse_configure_ac(content, &mut w);
        assert!(deps.contains(&"ssl".to_string()));
        assert!(deps.contains(&"m".to_string()));
    }

    #[test]
    fn parse_makefile_am_bin_programs() {
        let content = "bin_PROGRAMS = foo bar\nAM_CFLAGS = -std=c11 -DFOO\n";
        let mut w = vec![];
        let (targets, std, defines) = parse_makefile_am(content, &mut w);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, TargetKind::Bin);
        assert_eq!(targets[0].name, "foo");
        assert_eq!(std.as_deref(), Some("c11"));
        assert_eq!(defines, vec!["FOO"]);
    }

    #[test]
    fn parse_makefile_am_lib_ltlibraries() {
        let content = "lib_LTLIBRARIES = libfoo.la\n";
        let mut w = vec![];
        let (targets, _, _) = parse_makefile_am(content, &mut w);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "foo");
        assert_eq!(targets[0].kind, TargetKind::SharedLib);
    }

    #[test]
    fn parse_makefile_in_staticlib() {
        let content = "STATICLIB=libz.a\nSHAREDLIB=libz.so\n";
        let mut w = vec![];
        let (targets, _, _) = parse_makefile_in(content, &mut w);
        let static_t = targets.iter().find(|t| t.kind == TargetKind::StaticLib);
        assert!(static_t.is_some());
        assert_eq!(static_t.unwrap().name, "z");
    }

    #[test]
    fn auto_linked_libs_filtered() {
        let content = "AC_CHECK_LIB(m, sin)\nAC_CHECK_LIB(ssl, SSL_new)\n";
        let mut w = vec![];
        let (_, _, deps) = parse_configure_ac(content, &mut w);
        let mut deps_copy = deps.clone();
        deps_copy.retain(|d| !AUTO_LINKED.contains(&d.as_str()));
        assert!(!deps_copy.contains(&"m".to_string()));
        assert!(deps_copy.contains(&"ssl".to_string()));
    }
}
