/// Import an Autotools project (configure.ac + optional Makefile.am) into freight.
///
/// Parsing strategy (static only — no shell execution):
///   1. `configure.ac` / `configure.in` → package name, version, dependencies
///   2. `Makefile.am`                    → targets (bin_PROGRAMS, lib_LIBRARIES, …),
///                                         `SUBDIRS` (recursed automatically)
///   3. `Makefile.in` fallback          → treat like a Makefile after stripping @...@ tokens
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;
use toml_edit::{value, Array, DocumentMut, Item, Table};

use super::sanitize_name;

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ImportResult {
    pub written: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// A local path dependency discovered while recursing into a SUBDIRS entry.
struct PathDep {
    /// Dep name as it appears in `[dependencies]`.
    name: String,
    /// Path relative to the project root (e.g. `"lib/foo"`).
    rel_path: String,
}

/// Purge artefacts left by autotools from `dir`.
pub fn purge_autotools(dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let files = [
        "configure",
        "configure.ac",
        "configure.in",
        "config.status",
        "config.guess",
        "config.sub",
        "config.log",
        "Makefile.am",
        "Makefile.in",
        "Makefile",
        "aclocal.m4",
        "install-sh",
        "missing",
        "depcomp",
        "compile",
        "ltmain.sh",
        "libtool",
    ];
    for name in &files {
        let p = dir.join(name);
        if p.exists() && std::fs::remove_file(&p).is_ok() {
            removed.push(format!("removed {}", p.display()));
        }
    }
    // autom4te.cache directory
    let cache = dir.join("autom4te.cache");
    if cache.is_dir() && std::fs::remove_dir_all(&cache).is_ok() {
        removed.push(format!("removed {}/", cache.display()));
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
    let mut all_written: Vec<PathBuf> = Vec::new();

    // ── 1. Parse configure.ac / configure.in ─────────────────────────────────
    let dir_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let configure_ac = find_configure_ac(&project_dir);
    let (parsed_name, pkg_version, mut deps, ac_defines, ac_features) = match &configure_ac {
        Some(path) => {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            parse_configure_ac(&content, &mut warnings)
        }
        None => {
            warnings.push("No configure.ac / configure.in found — using defaults".into());
            (String::new(), "0.1.0".to_string(), vec![], vec![], vec![])
        }
    };
    // Prefer explicit name from AC_INIT; fall back to directory name
    let pkg_name = if parsed_name.is_empty() || parsed_name == "project" {
        sanitize_name(&dir_name)
    } else {
        parsed_name
    };

    // ── 2. Parse Makefile.am (preferred) or Makefile.in (fallback) ───────────
    let (targets, lang_std, extra_defines, subdirs, am_ldadd_deps) =
        if let Some(am) = find_file(&project_dir, "Makefile.am") {
            let content = std::fs::read_to_string(&am)
                .with_context(|| format!("reading {}", am.display()))?;
            parse_makefile_am(&content, &mut warnings)
        } else if let Some(mf_in) = find_file(&project_dir, "Makefile.in") {
            let content = std::fs::read_to_string(&mf_in)
                .with_context(|| format!("reading {}", mf_in.display()))?;
            warnings.push("No Makefile.am found — falling back to Makefile.in".into());
            let (t, s, d) = parse_makefile_in(&content, &mut warnings);
            (t, s, d, vec![], vec![])
        } else {
            warnings.push("No Makefile.am or Makefile.in — inferring from filesystem".into());
            let kind = if has_main_function(&project_dir) {
                TargetKind::Bin
            } else {
                TargetKind::StaticLib
            };
            let name = sanitize_name(&pkg_name);
            (
                vec![TargetSpec { name, kind }],
                None,
                vec![],
                vec![],
                vec![],
            )
        };

    // Merge per-target / global LDADD deps from Makefile.am
    for d in am_ldadd_deps {
        if !deps.contains(&d) {
            deps.push(d);
        }
    }
    // Drop compiler-driver libs and route OS system libraries (pthread, m, …) to
    // `[os.*] features`; real packages stay in [dependencies].
    let mut os_features: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    {
        let mut kept = Vec::new();
        super::split_link_libs(&deps, &mut kept, &mut os_features);
        deps = kept;
    }

    // Merge AC_DEFINE symbols into the defines list from Makefile.am
    // (deduplication happens in emit_toml via the combined slice)

    // ── 3. Recurse into SUBDIRS ───────────────────────────────────────────────
    let mut path_deps: Vec<PathDep> = Vec::new();

    for subdir_name in &subdirs {
        let subdir_path = project_dir.join(subdir_name);
        if !subdir_path.is_dir() {
            warnings.push(format!(
                "SUBDIRS: '{subdir_name}' does not exist — skipping"
            ));
            continue;
        }

        // Only recurse if the subdir has autotools files to migrate.
        let has_configure = find_configure_ac(&subdir_path).is_some();
        let has_makefile_am = find_file(&subdir_path, "Makefile.am").is_some();
        if !has_configure && !has_makefile_am {
            warnings.push(format!(
                "SUBDIRS: '{subdir_name}' has no configure.ac or Makefile.am — skipping"
            ));
            continue;
        }

        // Migrate the subdir in-place (writes its own freight.toml).
        match import_autotools(&subdir_path, Some(&subdir_path)) {
            Ok(sub_result) => {
                // Prefix subdir warnings so the user knows which subdir they came from.
                warnings.extend(
                    sub_result
                        .warnings
                        .into_iter()
                        .map(|w| format!("[{subdir_name}] {w}")),
                );
                all_written.extend(sub_result.written);

                // If the subdir produces library targets, add it as a path dep.
                if let Some(am) = find_file(&subdir_path, "Makefile.am") {
                    if let Ok(content) = std::fs::read_to_string(&am) {
                        let mut sub_warnings = Vec::new();
                        let (sub_targets, ..) = parse_makefile_am(&content, &mut sub_warnings);
                        for t in &sub_targets {
                            if matches!(t.kind, TargetKind::StaticLib | TargetKind::SharedLib) {
                                // Relative path from the project root to the subdir.
                                let rel = subdir_name.clone();
                                if !path_deps.iter().any(|p| p.name == t.name) {
                                    path_deps.push(PathDep {
                                        name: t.name.clone(),
                                        rel_path: rel.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push(format!("SUBDIRS: failed to migrate '{subdir_name}': {e:#}"));
            }
        }
    }

    // ── 4. Emit freight.toml ──────────────────────────────────────────────────
    // Merge AM_CFLAGS defines with AC_DEFINE symbols
    let mut all_defines = extra_defines;
    for d in &ac_defines {
        if !all_defines.contains(d) {
            all_defines.push(d.clone());
        }
    }

    let toml = emit_toml(
        &pkg_name,
        &pkg_version,
        &targets,
        lang_std.as_deref(),
        &all_defines,
        &deps,
        &path_deps,
        &os_features,
        &ac_features,
        &warnings,
    );
    // Fold in a sibling vcpkg.json's declared dependencies, if present.
    let toml = super::vcpkg::apply_vcpkg_manifest(toml, &project_dir, &mut warnings);

    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;
    let dest = out_root.join("freight.toml");
    std::fs::write(&dest, &toml).with_context(|| format!("writing {}", dest.display()))?;
    all_written.push(dest);

    Ok(ImportResult {
        written: all_written,
        warnings,
    })
}

// ── configure.ac parser ───────────────────────────────────────────────────────

fn find_configure_ac(dir: &Path) -> Option<PathBuf> {
    for name in &["configure.ac", "configure.in"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

/// Returns `(name, version, deps, defines, features)`.
/// - `defines`  — symbols from `AC_DEFINE`
/// - `features` — names from `AC_ARG_ENABLE` / `AC_ARG_WITH`
fn parse_configure_ac(
    content: &str,
    warnings: &mut Vec<String>,
) -> (String, String, Vec<String>, Vec<String>, Vec<String>) {
    let mut name = String::new();
    let mut version = String::from("0.1.0");
    let mut deps: Vec<String> = Vec::new();
    let mut defines: Vec<String> = Vec::new();
    let mut features: Vec<String> = Vec::new();

    // AC_INIT([name], [version]) or AC_INIT(name, version)
    let ac_init = Regex::new(r"AC_INIT\(\s*\[?([^\],\)]+)\]?\s*,\s*\[?([^\],\)]+)\]?").unwrap();
    if let Some(cap) = ac_init.captures(content) {
        let n = sanitize_name(cap[1].trim());
        let v = cap[2].trim().to_string();
        if !n.is_empty() {
            name = n;
        }
        if !v.is_empty() {
            version = v;
        }
    }

    // PKG_CHECK_MODULES(PREFIX, module [>= ver])
    let pkg_check =
        Regex::new(r"PKG_CHECK_MODULES(?:_OPTIONAL)?\s*\(\s*\w+\s*,\s*([^\)]+)\)").unwrap();
    for cap in pkg_check.captures_iter(content) {
        // May be "foo >= 1.0" or "foo bar" (multiple)
        for part in cap[1].split_whitespace() {
            let part =
                part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            if part.is_empty()
                || part.starts_with(|c: char| c.is_ascii_digit())
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

    // AC_DEFINE([SYMBOL], ...) / AC_DEFINE_UNQUOTED([SYMBOL], ...)
    let ac_define = Regex::new(r"AC_DEFINE(?:_UNQUOTED)?\(\s*\[?(\w+)\]?").unwrap();
    for cap in ac_define.captures_iter(content) {
        let sym = cap[1].to_string();
        if !defines.contains(&sym) {
            defines.push(sym);
        }
    }
    if !defines.is_empty() {
        warnings.push(
            "AC_DEFINE macros found — emitted under [build] defines; \
             verify they apply to your environment"
                .into(),
        );
    }

    // AC_ARG_ENABLE([feature], ...) and AC_ARG_WITH([name], ...)
    let ac_arg_enable = Regex::new(r"AC_ARG_ENABLE\(\s*\[?([a-zA-Z0-9_-]+)\]?").unwrap();
    for cap in ac_arg_enable.captures_iter(content) {
        let feat = sanitize_name(cap[1].trim());
        if !feat.is_empty() && !features.contains(&feat) {
            features.push(feat);
        }
    }
    let ac_arg_with = Regex::new(r"AC_ARG_WITH\(\s*\[?([a-zA-Z0-9_-]+)\]?").unwrap();
    for cap in ac_arg_with.captures_iter(content) {
        let feat = sanitize_name(cap[1].trim());
        if !feat.is_empty() && !features.contains(&feat) {
            features.push(feat);
        }
    }

    // Warn about things we can't convert
    if content.contains("AC_CHECK_HEADERS") {
        warnings.push("AC_CHECK_HEADERS detected — review include paths manually".into());
    }

    (name, version, deps, defines, features)
}

// ── Makefile.am parser ────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Clone, Copy)]
enum TargetKind {
    Bin,
    StaticLib,
    SharedLib,
}

#[derive(Debug)]
struct TargetSpec {
    name: String,
    kind: TargetKind,
}

/// Returns `(targets, lang_std, defines, subdirs, ldadd_deps)`.
/// - `subdirs`    — SUBDIRS directory names
/// - `ldadd_deps` — system libs from LDADD / *_LDADD / *_LIBADD / *_LDFLAGS
fn parse_makefile_am(
    content: &str,
    warnings: &mut Vec<String>,
) -> (
    Vec<TargetSpec>,
    Option<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let mut targets: Vec<TargetSpec> = Vec::new();
    let mut lang_std: Option<String> = None;
    let mut defines: Vec<String> = Vec::new();
    let mut subdirs: Vec<String> = Vec::new();
    let mut ldadd_deps: Vec<String> = Vec::new();

    // Join continuation lines
    let joined = join_continuations(content);

    for line in joined.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // bin_PROGRAMS / noinst_PROGRAMS / check_PROGRAMS / sbin_PROGRAMS / libexec_PROGRAMS
        if let Some(rest) = strip_lhs(
            line,
            &[
                "bin_PROGRAMS",
                "sbin_PROGRAMS",
                "libexec_PROGRAMS",
                "noinst_PROGRAMS",
                "check_PROGRAMS",
                "dist_bin_SCRIPTS",
            ],
        ) {
            for name in rest.split_whitespace() {
                let n = sanitize_name(name);
                if !n.is_empty() && !targets.iter().any(|t| t.name == n) {
                    targets.push(TargetSpec {
                        name: n,
                        kind: TargetKind::Bin,
                    });
                }
            }
        }
        // lib_LIBRARIES / noinst_LIBRARIES (static)
        else if let Some(rest) = strip_lhs(
            line,
            &[
                "lib_LIBRARIES",
                "noinst_LIBRARIES",
                "pkglib_LIBRARIES",
                "check_LIBRARIES",
            ],
        ) {
            for name in rest.split_whitespace() {
                let n = sanitize_name(strip_lib_ext(name));
                if !n.is_empty() && !targets.iter().any(|t| t.name == n) {
                    targets.push(TargetSpec {
                        name: n,
                        kind: TargetKind::StaticLib,
                    });
                }
            }
        }
        // lib_LTLIBRARIES / noinst_LTLIBRARIES (libtool → shared)
        else if let Some(rest) = strip_lhs(
            line,
            &[
                "lib_LTLIBRARIES",
                "noinst_LTLIBRARIES",
                "pkglib_LTLIBRARIES",
            ],
        ) {
            for name in rest.split_whitespace() {
                // .la → treat as shared lib
                let stripped = name.strip_suffix(".la").unwrap_or(name);
                let n = sanitize_name(strip_lib_ext(stripped));
                if !n.is_empty() && !targets.iter().any(|t| t.name == n) {
                    targets.push(TargetSpec {
                        name: n,
                        kind: TargetKind::SharedLib,
                    });
                }
            }
        }
        // AM_CFLAGS / AM_CXXFLAGS
        else if let Some(flags) = strip_lhs(
            line,
            &[
                "AM_CFLAGS",
                "AM_CXXFLAGS",
                "AM_CPPFLAGS",
                "CFLAGS",
                "CXXFLAGS",
            ],
        ) {
            if lang_std.is_none() {
                lang_std = extract_std(flags);
            }
            for tok in flags.split_whitespace() {
                if let Some(def) = tok.strip_prefix("-D") {
                    if !def.is_empty() {
                        defines.push(def.to_string());
                    }
                }
            }
        }
        // SUBDIRS — collect subdir names for recursive migration
        else if let Some(rest) = strip_lhs(line, &["SUBDIRS"]) {
            for name in rest.split_whitespace() {
                // Skip special tokens like '.' (current dir) or '..'
                if name != "." && name != ".." && !name.is_empty() {
                    subdirs.push(name.to_string());
                }
            }
        }
        // DIST_SUBDIRS is informational only — don't recurse into it
        else if line.starts_with("DIST_SUBDIRS") {
            // ignore
        }
        // Global LDADD / LIBADD / LDFLAGS / AM_LDFLAGS
        else if let Some(rhs) = strip_lhs(line, &["LDADD", "LIBADD", "LDFLAGS", "AM_LDFLAGS"]) {
            for tok in rhs.split_whitespace() {
                if let Some(lib) = tok.strip_prefix("-l") {
                    if !lib.is_empty() && !ldadd_deps.contains(&lib.to_string()) {
                        ldadd_deps.push(lib.to_string());
                    }
                }
            }
        }
        // AM_CONDITIONAL if/endif blocks — warn and still extract -l deps from body
        else if line.starts_with("if ") && !line.starts_with("ifeq") && !line.starts_with("ifdef")
        {
            let cond_name = line[3..].trim();
            warnings.push(format!(
                "AM_CONDITIONAL block 'if {cond_name}' detected — \
                 conditional sources/deps not converted; review manually"
            ));
        }
        // Per-target *_LDADD / *_LIBADD / *_LDFLAGS (e.g. myapp_LDADD = -lssl)
        else if (line.contains("_LDADD") || line.contains("_LIBADD") || line.contains("_LDFLAGS"))
            && (line.contains('='))
        {
            let rhs = line
                .find("+=")
                .map(|p| &line[p + 2..])
                .or_else(|| line.find(":=").map(|p| &line[p + 2..]))
                .or_else(|| line.find('=').map(|p| &line[p + 1..]))
                .unwrap_or("");
            for tok in rhs.split_whitespace() {
                if let Some(lib) = tok.strip_prefix("-l") {
                    if !lib.is_empty() && !ldadd_deps.contains(&lib.to_string()) {
                        ldadd_deps.push(lib.to_string());
                    }
                }
            }
        }
    }

    // If nothing found (and no subdirs to recurse), warn
    if targets.is_empty() && subdirs.is_empty() {
        warnings.push("No target declarations found in Makefile.am".into());
    }

    (targets, lang_std, defines, subdirs, ldadd_deps)
}

// ── Makefile.in fallback parser ───────────────────────────────────────────────

fn parse_makefile_in(
    content: &str,
    warnings: &mut Vec<String>,
) -> (Vec<TargetSpec>, Option<String>, Vec<String>) {
    // Strip @VARIABLE@ tokens (autoconf substitutions) to reduce parse noise
    let cleaned = Regex::new(r"@[A-Z_]+@")
        .unwrap()
        .replace_all(content, "")
        .into_owned();

    let mut targets: Vec<TargetSpec> = Vec::new();
    let mut lang_std: Option<String> = None;
    let mut defines: Vec<String> = Vec::new();

    let joined = join_continuations(&cleaned);

    // Look for STATICLIB / SHAREDLIB / PROGRAMS variable patterns
    for line in joined.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = strip_lhs(line, &["STATICLIB", "STATIC_LIB"]) {
            for name in rest.split_whitespace() {
                let n = sanitize_name(strip_lib_ext(name));
                if !n.is_empty() {
                    targets.push(TargetSpec {
                        name: n,
                        kind: TargetKind::StaticLib,
                    });
                }
            }
        } else if let Some(rest) = strip_lhs(
            line,
            &["SHAREDLIB", "SHARED_LIB", "SHAREDLIBV", "SHAREDLIBM"],
        ) {
            // Only take the first one
            if targets.iter().all(|t| t.kind != TargetKind::SharedLib) {
                if let Some(name) = rest.split_whitespace().next() {
                    let n = sanitize_name(strip_lib_ext(name));
                    if !n.is_empty() {
                        targets.push(TargetSpec {
                            name: n,
                            kind: TargetKind::SharedLib,
                        });
                    }
                }
            }
        } else if let Some(rest) = strip_lhs(line, &["PROGRAMS", "BINS", "BIN"]) {
            for name in rest.split_whitespace() {
                let n = sanitize_name(name);
                if !n.is_empty() {
                    targets.push(TargetSpec {
                        name: n,
                        kind: TargetKind::Bin,
                    });
                }
            }
        } else if let Some(flags) = strip_lhs(line, &["CFLAGS", "CXXFLAGS", "CPPFLAGS"]) {
            if lang_std.is_none() {
                lang_std = extract_std(flags);
            }
            for tok in flags.split_whitespace() {
                if let Some(def) = tok.strip_prefix("-D") {
                    if !def.is_empty() {
                        defines.push(def.to_string());
                    }
                }
            }
        }
    }

    if targets.is_empty() {
        warnings
            .push("Could not determine targets from Makefile.in — check output manually".into());
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
    path_deps: &[PathDep],
    os_features: &std::collections::BTreeMap<String, Vec<String>>,
    features: &[String],
    warnings: &[String],
) -> String {
    use toml_edit::InlineTable;

    let mut doc = DocumentMut::new();

    let header: String = warnings
        .iter()
        .map(|w| format!("# warning: {w}\n"))
        .collect::<String>();
    let header =
        format!("# Generated by freight migrate autotools — review before committing.\n{header}");

    let mut pkg = Table::new();
    pkg["name"] = value(name);
    pkg["version"] = value(version);
    doc["package"] = Item::Table(pkg);

    // [features] — from AC_ARG_ENABLE / AC_ARG_WITH
    if !features.is_empty() {
        let mut feat_tbl = Table::new();
        for f in features {
            feat_tbl[f.as_str()] = value(Array::new());
        }
        doc["features"] = Item::Table(feat_tbl);
    }

    if let Some(std) = lang_std {
        let lang_key = if std.starts_with("c++") || std.starts_with("gnu++") {
            "c++"
        } else {
            "c"
        };
        let mut lang_tbl = Table::new();
        lang_tbl["std"] = value(std);
        let mut lang_outer = Table::new();
        lang_outer[lang_key] = Item::Table(lang_tbl);
        doc["language"] = Item::Table(lang_outer);
    }

    if !defines.is_empty() {
        let mut build_tbl = Table::new();
        let mut arr = Array::new();
        for d in defines {
            arr.push(d.as_str());
        }
        build_tbl["defines"] = value(arr);
        doc["build"] = Item::Table(build_tbl);
    }

    if !deps.is_empty() || !path_deps.is_empty() {
        let mut dep_tbl = Table::new();
        for d in deps {
            dep_tbl[d.as_str()] = super::system_dep_item(d);
        }
        for pd in path_deps {
            let mut inline = InlineTable::new();
            inline.insert("path", pd.rel_path.as_str().into());
            dep_tbl[pd.name.as_str()] = Item::Value(toml_edit::Value::InlineTable(inline));
        }
        doc["dependencies"] = Item::Table(dep_tbl);
    }

    // [os.*] features — system libraries (pthread, m, …) linked via `-l`.
    if !os_features.is_empty() {
        let mut os_tbl = Table::new();
        os_tbl.set_implicit(true);
        for (os_key, feats) in os_features {
            let mut arr = Array::new();
            for f in feats {
                arr.push(f.as_str());
            }
            let mut sec = Table::new();
            sec["features"] = value(arr);
            os_tbl[os_key.as_str()] = Item::Table(sec);
        }
        doc["os"] = Item::Table(os_tbl);
    }

    for t in targets {
        let mut tbl = Table::new();
        tbl["name"] = value(t.name.as_str());
        match t.kind {
            TargetKind::Bin => {
                let arr = doc["bin"].or_insert(Item::ArrayOfTables(Default::default()));
                if let Item::ArrayOfTables(aot) = arr {
                    aot.push(tbl);
                }
            }
            // `lib` is a single table; only the first library target is kept
            // (a freight package has at most one library).
            TargetKind::StaticLib | TargetKind::SharedLib if !doc.contains_key("lib") => {
                tbl["type"] = value(if t.kind == TargetKind::SharedLib {
                    "shared"
                } else {
                    "static"
                });
                doc["lib"] = Item::Table(tbl);
            }
            _ => {}
        }
    }

    format!("{header}{doc}")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn join_continuations(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
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
            if let Some(rhs) = rest
                .strip_prefix("+=")
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
    let name = if let Some(p) = name.find(".so") {
        &name[..p]
    } else {
        name
    };
    name.strip_prefix("lib").unwrap_or(name)
}

fn has_main_function(dir: &Path) -> bool {
    use walkdir::WalkDir;
    let exts = ["c", "cpp", "cc", "cxx", "C"];
    for entry in WalkDir::new(dir).max_depth(3).into_iter().flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !exts.contains(&ext) {
            continue;
        }
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
        let (name, ver, _, _, _) = parse_configure_ac(content, &mut w);
        assert_eq!(name, "mylib");
        assert_eq!(ver, "1.2.3");
    }

    #[test]
    fn parse_ac_init_bare_style() {
        let content = "AC_INIT(myapp, 2.0)";
        let mut w = vec![];
        let (name, ver, _, _, _) = parse_configure_ac(content, &mut w);
        assert_eq!(name, "myapp");
        assert_eq!(ver, "2.0");
    }

    #[test]
    fn parse_pkg_check_modules() {
        let content = "PKG_CHECK_MODULES(SSL, openssl >= 1.0 libcurl)\n";
        let mut w = vec![];
        let (_, _, deps, _, _) = parse_configure_ac(content, &mut w);
        assert!(deps.contains(&"openssl".to_string()));
        assert!(deps.contains(&"libcurl".to_string()));
    }

    #[test]
    fn parse_ac_check_lib() {
        let content = "AC_CHECK_LIB([ssl], [SSL_new])\nAC_CHECK_LIB(m, sin)\n";
        let mut w = vec![];
        let (_, _, deps, _, _) = parse_configure_ac(content, &mut w);
        assert!(deps.contains(&"ssl".to_string()));
        assert!(deps.contains(&"m".to_string()));
    }

    #[test]
    fn parse_makefile_am_bin_programs() {
        let content = "bin_PROGRAMS = foo bar\nAM_CFLAGS = -std=c11 -DFOO\n";
        let mut w = vec![];
        let (targets, std, defines, subdirs, _) = parse_makefile_am(content, &mut w);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].kind, TargetKind::Bin);
        assert_eq!(targets[0].name, "foo");
        assert_eq!(std.as_deref(), Some("c11"));
        assert_eq!(defines, vec!["FOO"]);
        assert!(subdirs.is_empty());
    }

    #[test]
    fn parse_makefile_am_lib_ltlibraries() {
        let content = "lib_LTLIBRARIES = libfoo.la\n";
        let mut w = vec![];
        let (targets, _, _, _, _) = parse_makefile_am(content, &mut w);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "foo");
        assert_eq!(targets[0].kind, TargetKind::SharedLib);
    }

    #[test]
    fn parse_makefile_am_subdirs_collected() {
        let content = "SUBDIRS = lib src tests\n";
        let mut w = vec![];
        let (targets, _, _, subdirs, _) = parse_makefile_am(content, &mut w);
        assert!(targets.is_empty());
        assert_eq!(subdirs, vec!["lib", "src", "tests"]);
        // No "not detected" warning anymore
        assert!(!w.iter().any(|w| w.contains("workspace members not yet")));
    }

    #[test]
    fn parse_makefile_am_subdirs_dot_skipped() {
        let content = "SUBDIRS = . lib\n";
        let mut w = vec![];
        let (_, _, _, subdirs, _) = parse_makefile_am(content, &mut w);
        assert_eq!(subdirs, vec!["lib"]);
    }

    #[test]
    fn per_target_ldadd_extracted() {
        let content = "bin_PROGRAMS = foo\nfoo_LDADD = -lssl -lcurl\n";
        let mut w = vec![];
        let (_, _, _, _, ldadd) = parse_makefile_am(content, &mut w);
        assert!(ldadd.contains(&"ssl".to_string()));
        assert!(ldadd.contains(&"curl".to_string()));
    }

    #[test]
    fn global_ldadd_extracted() {
        let content = "bin_PROGRAMS = app\nLDADD = -lz -lpng\n";
        let mut w = vec![];
        let (_, _, _, _, ldadd) = parse_makefile_am(content, &mut w);
        assert!(ldadd.contains(&"z".to_string()));
        assert!(ldadd.contains(&"png".to_string()));
    }

    #[test]
    fn ldadd_auto_linked_filtered_in_caller() {
        // pthread and m are AUTO_LINKED — they should be filtered in import_autotools
        // but parse_makefile_am itself returns them raw
        let content = "LDADD = -lpthread -lm -lssl\n";
        let mut w = vec![];
        let (_, _, _, _, ldadd) = parse_makefile_am(content, &mut w);
        assert!(ldadd.contains(&"pthread".to_string())); // raw, not yet filtered
        assert!(ldadd.contains(&"ssl".to_string()));
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
    fn subdirs_recursed_and_emitted_as_path_deps() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Root: configure.ac + Makefile.am with SUBDIRS = mylib
        fs::create_dir_all(root.join("mylib")).unwrap();
        fs::write(root.join("configure.ac"), "AC_INIT([myapp], [1.0])\n").unwrap();
        fs::write(
            root.join("Makefile.am"),
            "SUBDIRS = mylib\nbin_PROGRAMS = myapp\n",
        )
        .unwrap();
        // Subdir: its own Makefile.am with a static library
        fs::write(
            root.join("mylib/Makefile.am"),
            "lib_LIBRARIES = libmylib.a\n",
        )
        .unwrap();

        let result = import_autotools(root, Some(root)).unwrap();

        // Both freight.tomls should have been written
        assert!(result
            .written
            .iter()
            .any(|p| p == &root.join("freight.toml")));
        assert!(result
            .written
            .iter()
            .any(|p| p == &root.join("mylib/freight.toml")));

        // Root manifest should have mylib as a path dep
        let root_toml = fs::read_to_string(root.join("freight.toml")).unwrap();
        assert!(
            root_toml.contains("mylib"),
            "expected mylib path dep in root manifest:\n{root_toml}"
        );
        assert!(
            root_toml.contains("mylib"),
            "expected 'mylib' in root manifest:\n{root_toml}"
        );
    }

    #[test]
    fn system_libs_routed_to_features() {
        // `m` is a system library → `[os.unix] features`; `ssl` is a real dep.
        let content = "AC_CHECK_LIB(m, sin)\nAC_CHECK_LIB(ssl, SSL_new)\n";
        let mut w = vec![];
        let (_, _, deps, _, _) = parse_configure_ac(content, &mut w);
        let mut kept = Vec::new();
        let mut feats: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        super::super::split_link_libs(&deps, &mut kept, &mut feats);
        assert!(kept.contains(&"ssl".to_string()));
        assert!(!kept.contains(&"m".to_string()));
        assert!(feats
            .get("unix")
            .is_some_and(|v| v.contains(&"m".to_string())));
    }

    #[test]
    fn parse_ac_define() {
        let content = "AC_DEFINE([HAVE_SSL], [1], [OpenSSL available])\nAC_DEFINE_UNQUOTED([VERSION], [\"1.0\"], [])\n";
        let mut w = vec![];
        let (_, _, _, defines, _) = parse_configure_ac(content, &mut w);
        assert!(defines.contains(&"HAVE_SSL".to_string()));
        assert!(defines.contains(&"VERSION".to_string()));
        assert!(!w.is_empty(), "should warn about AC_DEFINE");
    }

    #[test]
    fn parse_ac_arg_enable_and_with() {
        let content = "AC_ARG_ENABLE([tls], AS_HELP_STRING([--enable-tls], [Enable TLS]))\n\
             AC_ARG_WITH([openssl], AS_HELP_STRING([--with-openssl], [Use OpenSSL]))\n";
        let mut w = vec![];
        let (_, _, _, _, features) = parse_configure_ac(content, &mut w);
        assert!(features.contains(&"tls".to_string()));
        assert!(features.contains(&"openssl".to_string()));
    }

    #[test]
    fn emit_toml_features_section() {
        let toml = emit_toml(
            "mylib",
            "1.0.0",
            &[],
            None,
            &[],
            &[],
            &[],
            &Default::default(),
            &["tls".to_string(), "openssl".to_string()],
            &[],
        );
        assert!(
            toml.contains("[features]"),
            "expected [features] section:\n{toml}"
        );
        assert!(toml.contains("tls"), "expected tls feature:\n{toml}");
        assert!(
            toml.contains("openssl"),
            "expected openssl feature:\n{toml}"
        );
    }

    #[test]
    fn emit_toml_ac_defines_in_build() {
        let toml = emit_toml(
            "mylib",
            "1.0.0",
            &[],
            None,
            &["HAVE_SSL".to_string()],
            &[],
            &[],
            &Default::default(),
            &[],
            &[],
        );
        assert!(
            toml.contains("[build]"),
            "expected [build] section:\n{toml}"
        );
        assert!(
            toml.contains("HAVE_SSL"),
            "expected HAVE_SSL define:\n{toml}"
        );
    }

    #[test]
    fn am_conditional_block_warned() {
        let content = "bin_PROGRAMS = foo\nif HAVE_SSL\nfoo_LDADD = -lssl\nendif\n";
        let mut w = vec![];
        let (_, _, _, _, ldadd) = parse_makefile_am(content, &mut w);
        assert!(
            w.iter().any(|s| s.contains("HAVE_SSL")),
            "expected AM_CONDITIONAL warning:\n{w:?}"
        );
        // Even inside the conditional block, ssl should be extracted from foo_LDADD
        assert!(
            ldadd.contains(&"ssl".to_string()),
            "ldadd should contain ssl:\n{ldadd:?}"
        );
    }
}
