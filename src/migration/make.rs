/// Convert a Make-based project into a freight.toml (or workspace of freight.toml files).
///
/// Strategy: static extraction only — Makefiles are Turing-complete so we
/// never try to evaluate shell expansions or complex conditionals.  What we
/// *do* handle covers ~95 % of real projects:
///
///   • Variable assignments (=, :=, +=) with simple $(VAR) expansion
///   • CFLAGS / CXXFLAGS / CPPFLAGS  →  language std, defines, includes
///   • LDFLAGS / LDLIBS / LIBS        →  system library deps
///   • Final-target name / extension  →  [[bin]] vs [[lib]]
///   • SUBDIRS / $(MAKE) -C           →  workspace layout
///   • Filesystem walk                →  source language detection
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use makefile_lossless::{Makefile, VariableDefinition};
use toml_edit::{value, Array, DocumentMut, Item, Table};
use walkdir::WalkDir;

use super::sanitize_name;

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ImportResult {
    /// Files written, relative to the project root.
    pub written: Vec<PathBuf>,
    /// Human-readable notes about things that couldn't be converted.
    pub warnings: Vec<String>,
}

/// Import a Make project rooted at `input` (a directory or a Makefile path).
/// Writes `freight.toml` (and workspace members) next to the Makefile unless
/// `out_dir` is given.
pub fn import_make(input: &Path, out_dir: Option<&Path>) -> Result<ImportResult> {
    let (project_dir, makefile_path) = resolve_input(input)?;
    let out_root = out_dir.unwrap_or(&project_dir);

    let content = std::fs::read_to_string(&makefile_path)
        .with_context(|| format!("reading {}", makefile_path.display()))?;

    let mf = Makefile::read_relaxed(content.as_bytes())
        .with_context(|| format!("parsing {}", makefile_path.display()))?;

    let vars = collect_vars(&mf);
    let expanded = ExpandedVars::new(vars);
    let mut warnings: Vec<String> = Vec::new();

    // ── Autotools stub detection ──────────────────────────────────────────────
    let is_autotools_stub = project_dir.join("configure.ac").exists()
        || project_dir.join("configure.in").exists()
        || mf
            .rules_by_target("all")
            .flat_map(|r| r.recipes())
            .any(|recipe| recipe.contains("configure") && recipe.contains("echo"));
    if is_autotools_stub {
        warnings.push(
            "This looks like an autotools project — consider `freight migrate autotools` instead"
                .into(),
        );
    }

    // ── Workspace detection ───────────────────────────────────────────────────
    let subdirs = find_subdirs(&mf, &expanded);
    if !subdirs.is_empty() {
        return import_workspace(&project_dir, out_root, &subdirs, &mut warnings);
    }

    // ── Single project ────────────────────────────────────────────────────────
    let spec = analyze(&mf, &expanded, &project_dir, &content, &mut warnings);
    let toml = emit_toml(&spec);

    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;
    let dest = out_root.join("freight.toml");
    std::fs::write(&dest, &toml).with_context(|| format!("writing {}", dest.display()))?;

    Ok(ImportResult {
        written: vec![dest],
        warnings,
    })
}

// ── Workspace ─────────────────────────────────────────────────────────────────

fn import_workspace(
    project_dir: &Path,
    out_root: &Path,
    subdirs: &[String],
    warnings: &mut Vec<String>,
) -> Result<ImportResult> {
    let mut written = Vec::new();

    // Root workspace freight.toml
    let mut doc = DocumentMut::new();
    let mut ws = Table::new();
    let mut members = Array::new();
    for d in subdirs {
        members.push(d.as_str());
    }
    ws["members"] = value(members);
    doc["workspace"] = Item::Table(ws);

    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;
    let root_toml = out_root.join("freight.toml");
    std::fs::write(&root_toml, doc.to_string())
        .with_context(|| format!("writing {}", root_toml.display()))?;
    written.push(root_toml);

    // Per-member freight.toml files
    for subdir in subdirs {
        let member_dir = project_dir.join(subdir);
        if !member_dir.exists() {
            warnings.push(format!("subdir {subdir:?} not found — skipping"));
            continue;
        }
        let mf_path = find_makefile(&member_dir);
        if mf_path.is_none() {
            warnings.push(format!("no Makefile in {subdir} — skipping"));
            continue;
        }
        let content = std::fs::read_to_string(mf_path.as_ref().unwrap())?;
        let Ok(mf) = Makefile::read_relaxed(content.as_bytes()) else {
            warnings.push(format!("could not parse Makefile in {subdir}"));
            continue;
        };
        let vars = collect_vars(&mf);
        let expanded = ExpandedVars::new(vars);
        let spec = analyze(&mf, &expanded, &member_dir, &content, warnings);
        let toml = emit_toml(&spec);

        let dest_dir = out_root.join(subdir);
        std::fs::create_dir_all(&dest_dir)?;
        let dest = dest_dir.join("freight.toml");
        std::fs::write(&dest, &toml).with_context(|| format!("writing {}", dest.display()))?;
        written.push(dest);
    }

    Ok(ImportResult {
        written,
        warnings: warnings.clone(),
    })
}

// ── Analysis ──────────────────────────────────────────────────────────────────

struct ProjectSpec {
    name: String,
    version: String,
    targets: Vec<TargetSpec>,
    lang_c: Option<String>,
    lang_cpp: Option<String>,
    system_deps: Vec<String>,
    conditional_deps: ConditionalDeps,
    /// System-library link features per `[os.<os>]` (pthread, m, ws2_32, …).
    os_features: std::collections::BTreeMap<String, Vec<String>>,
    defines: Vec<String>,
    warnings: Vec<String>,
}

struct TargetSpec {
    name: String,
    kind: TargetKind,
    /// Detected entry-point source file relative to the project dir (for bins).
    src: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TargetKind {
    Bin,
    StaticLib,
    DynamicLib,
}

#[derive(Default)]
struct ConditionalDeps {
    windows: Vec<String>,
    linux: Vec<String>,
    macos: Vec<String>,
    unix: Vec<String>,
}

fn analyze(
    mf: &Makefile,
    vars: &ExpandedVars,
    project_dir: &Path,
    raw_content: &str,
    warnings: &mut Vec<String>,
) -> ProjectSpec {
    // Collect all flag variables
    let cflags = vars.get_joined(&["CFLAGS", "CCFLAGS"]);
    let cxxflags = vars.get_joined(&["CXXFLAGS", "CPPFLAGS"]);
    let ldflags = vars.get_joined(&["LDFLAGS", "LDLIBS", "LIBS", "LDADD"]);

    // Language standards
    let lang_c = extract_std(&cflags, 'c');
    let lang_cpp = extract_std(&cxxflags, '+').or_else(|| extract_std(&cflags, '+'));

    // Defines (-D flags from all compile-flag vars)
    let all_cflags = format!("{cflags} {cxxflags}");
    let defines = extract_defines(&all_cflags);

    // System deps from linker flags; filter auto-detected ones
    let mut system_deps = extract_libs(&ldflags);
    // Also scan link recipes for -l flags
    for rule in mf.rules() {
        for recipe in rule.recipes() {
            if is_link_recipe(&recipe) {
                system_deps.extend(extract_libs(&recipe));
            }
        }
    }
    system_deps.sort();
    system_deps.dedup();
    let mut system_deps = filter_auto_detected(system_deps);

    // Warn about unexpandable things we skipped
    for flag_var in &["CFLAGS", "CXXFLAGS", "LDFLAGS", "LIBS"] {
        let raw = vars.raw(flag_var).unwrap_or_default();
        if raw.contains("$(shell") || raw.contains("$(wildcard") {
            warnings.push(format!(
                "{flag_var} contains shell/wildcard expansion — review manually"
            ));
        }
    }

    // Infer final targets from rules
    let phony: HashSet<String> = mf
        .rules_by_target(".PHONY")
        .flat_map(|r| r.prerequisites())
        .collect();
    let mut targets = infer_targets(mf, vars, &phony, project_dir, warnings);

    // Detect entry-point source files for binary targets and warn about flat layout.
    // Search under src/ if it exists, otherwise the project root itself.
    let src_dir = project_dir.join("src");
    let in_src_dir = src_dir.is_dir();
    let search_root = if in_src_dir {
        src_dir.as_path()
    } else {
        project_dir
    };
    let bin_count = targets.iter().filter(|t| t.kind == TargetKind::Bin).count();
    if bin_count > 0 {
        // Paths are relative to project_dir so freight can resolve them
        let entry_points = find_entry_points(search_root, project_dir);
        // Assign each bin the best-matching entry-point file
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        for target in &mut targets {
            if target.kind != TargetKind::Bin {
                continue;
            }
            let n = target.name.replace('-', "_").to_lowercase();
            // Priority 1: stem exactly matches binary name
            // Priority 2: binary name contains the stem (e.g. "linenoise-example" ⊃ "example")
            // Priority 3: stem is "main" or starts with "main"
            // Priority 4: first unused entry point
            let ep = entry_points
                .iter()
                .filter(|(_, p)| !used.contains(p))
                .find(|(stem, _)| stem.replace('-', "_").to_lowercase() == n)
                .or_else(|| {
                    entry_points
                        .iter()
                        .filter(|(_, p)| !used.contains(p))
                        .find(|(stem, _)| n.contains(&stem.replace('-', "_").to_lowercase()))
                })
                .or_else(|| {
                    entry_points
                        .iter()
                        .filter(|(_, p)| !used.contains(p))
                        .find(|(stem, _)| stem == "main" || stem.starts_with("main"))
                })
                .or_else(|| entry_points.iter().find(|(_, p)| !used.contains(p)));
            if let Some((_, path)) = ep {
                used.insert(path.clone());
                target.src = Some(path.clone());
            }
        }
        if !in_src_dir && !entry_points.is_empty() {
            warnings.push(
                "Source files are at project root — move them to src/ for freight auto-discovery"
                    .into(),
            );
        }
    }

    // Project name — prefer TARGET/BIN/LIB variable, fall back to dir name
    let name = vars
        .get_first_nonempty(&["TARGET", "BIN", "LIB", "PROG", "NAME", "PACKAGE"])
        .or_else(|| {
            project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(sanitize_name)
        })
        .unwrap_or_else(|| "project".to_string());

    let mut conditional_deps = parse_conditional_deps(raw_content);
    let unknown_ifdef_deps = parse_ifdef_deps(raw_content, &mut conditional_deps);
    if !unknown_ifdef_deps.is_empty() {
        warnings.push(format!(
            "ifdef/ifndef blocks with unclassifiable condition — \
             {} lib(s) added to [dependencies]; review manually",
            unknown_ifdef_deps.len()
        ));
        for lib in &unknown_ifdef_deps {
            if !system_deps.contains(lib) {
                system_deps.push(lib.clone());
            }
        }
    }

    // Extract package names from $(shell pkg-config --libs …) in flag variables
    for var in &["LDFLAGS", "LDLIBS", "LIBS", "LDADD"] {
        let raw = vars.raw(var).unwrap_or_default();
        let pkgs = filter_auto_detected(extract_pkgconfig_libs(&raw));
        for pkg in pkgs {
            if !system_deps.contains(&pkg) {
                system_deps.push(pkg);
            }
        }
    }

    // Route system libraries (pthread, m, ws2_32, …) to `[os.*] features`,
    // leaving real packages in [dependencies] / [os.*.dependencies].
    let mut os_features: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    {
        let mut kept = Vec::new();
        super::split_link_libs(&system_deps, &mut kept, &mut os_features);
        system_deps = kept;
    }
    for libs in [
        &mut conditional_deps.windows,
        &mut conditional_deps.linux,
        &mut conditional_deps.macos,
        &mut conditional_deps.unix,
    ] {
        let mut kept = Vec::new();
        super::split_link_libs(libs, &mut kept, &mut os_features);
        *libs = kept;
    }

    ProjectSpec {
        name,
        version: "0.1.0".to_string(),
        targets,
        lang_c,
        lang_cpp,
        system_deps,
        conditional_deps,
        os_features,
        defines,
        warnings: warnings.clone(),
    }
}

/// Well-known phony-ish names that are never real build outputs.
const PSEUDO_TARGETS: &[&str] = &[
    "all",
    "clean",
    "distclean",
    "install",
    "uninstall",
    "test",
    "tests",
    "check",
    "build",
    "dist",
    "docs",
    "help",
    "default",
    "debug",
    "release",
    "package",
    "run",
    "fmt",
    "format",
    "lint",
    "prepare",
    "setup",
];

fn infer_targets(
    mf: &Makefile,
    vars: &ExpandedVars,
    phony: &HashSet<String>,
    project_dir: &Path,
    warnings: &mut Vec<String>,
) -> Vec<TargetSpec> {
    // 1a. Multi-binary variable hints (BINS, PROGS, TARGETS)
    for var in &["BINS", "PROGS", "TARGETS", "PROGRAMS"] {
        let val = vars.get(var);
        if val.is_empty() {
            continue;
        }
        let specs: Vec<TargetSpec> = val
            .split_whitespace()
            .filter(|n| !PSEUDO_TARGETS.contains(n))
            .filter_map(classify_target)
            .collect();
        if !specs.is_empty() {
            return specs;
        }
    }

    // 1b. Single-value variable hints
    if let Some(name) = vars.get_first_nonempty(&["BIN", "PROG", "BINARY", "TARGET", "EXECUTABLE"])
    {
        if !PSEUDO_TARGETS.contains(&name.as_str()) {
            return vec![TargetSpec {
                name: sanitize_name(&name),
                kind: TargetKind::Bin,
                src: None,
            }];
        }
    }
    if let Some(name) =
        vars.get_first_nonempty(&["LIB", "LIBRARY", "LIBNAME", "SHLIB", "STATIC_LIB"])
    {
        let kind = if name.ends_with(".so") || name.contains(".so.") {
            TargetKind::DynamicLib
        } else {
            TargetKind::StaticLib
        };
        return vec![TargetSpec {
            name: strip_lib_ext(&name),
            kind,
            src: None,
        }];
    }

    // 2. Look at the "all" rule's prerequisites — those are the real output targets.
    //    The parser extracts the identifier inside $(VAR) as the literal var name,
    //    so we expand any prerequisite that matches a known variable.
    let mut candidates: Vec<TargetSpec> = mf
        .rules_by_target("all")
        .flat_map(|r| r.prerequisites())
        .flat_map(|p| {
            // If the prerequisite name matches a make variable, expand it
            let expanded = vars.get(&p);
            if !expanded.is_empty() && expanded != p {
                expanded
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            } else {
                vec![p]
            }
        })
        .filter(|p| !phony.contains(p.as_str()) && !PSEUDO_TARGETS.contains(&p.as_str()))
        .filter_map(|p| classify_target(&p))
        .collect();

    if !candidates.is_empty() {
        return candidates;
    }

    // 3. First explicit non-phony, non-pseudo, non-pattern rule that looks like output
    for rule in mf.rules() {
        for target in rule.targets() {
            if phony.contains(target.as_str()) {
                continue;
            }
            if PSEUDO_TARGETS.contains(&target.as_str()) {
                continue;
            }
            if target.contains('%') || target.contains('$') {
                continue;
            }
            if let Some(spec) = classify_target(&target) {
                candidates.push(spec);
                break;
            }
        }
        if !candidates.is_empty() {
            break;
        }
    }

    if candidates.is_empty() {
        // 4. Fallback: infer from source files on disk
        let has_main = has_main_function(project_dir);
        let dir_name = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");
        let name = sanitize_name(dir_name);
        let kind = if has_main {
            TargetKind::Bin
        } else {
            TargetKind::StaticLib
        };
        if has_main {
            warnings.push(
                "Could not determine target from Makefile — assuming binary (found main())".into(),
            );
        } else {
            warnings
                .push("Could not determine target from Makefile — assuming static library".into());
        }
        candidates.push(TargetSpec {
            name,
            kind,
            src: None,
        });
    }

    candidates
}

fn classify_target(target: &str) -> Option<TargetSpec> {
    let t = target.trim();
    if t.is_empty() || t.starts_with('.') {
        return None;
    }

    if t.ends_with(".a") {
        return Some(TargetSpec {
            name: strip_lib_ext(t),
            kind: TargetKind::StaticLib,
            src: None,
        });
    }
    if t.ends_with(".so") || t.contains(".so.") {
        return Some(TargetSpec {
            name: strip_lib_ext(t),
            kind: TargetKind::DynamicLib,
            src: None,
        });
    }
    // No extension (or .exe) → binary
    let stem = Path::new(t).file_stem()?.to_str()?;
    if !stem.contains('.') {
        return Some(TargetSpec {
            name: sanitize_name(stem),
            kind: TargetKind::Bin,
            src: None,
        });
    }
    None
}

// ── TOML emitter ──────────────────────────────────────────────────────────────

fn emit_toml(spec: &ProjectSpec) -> String {
    let mut doc = DocumentMut::new();

    // [package]
    let mut pkg = Table::new();
    pkg["name"] = value(spec.name.as_str());
    pkg["version"] = value(spec.version.as_str());
    doc["package"] = Item::Table(pkg);

    // Header comment
    let mut header =
        String::from("# Generated by freight migrate make — review before committing.\n");
    for w in &spec.warnings {
        header.push_str(&format!("# warning: {w}\n"));
    }
    if let Some(Item::Table(t)) = doc.get_mut("package") {
        t.decor_mut().set_prefix(&header);
    }

    // [language.c] / [language.cpp]
    if spec.lang_c.is_some() || spec.lang_cpp.is_some() {
        let mut lang = Table::new();
        lang.set_implicit(true);
        if let Some(std) = &spec.lang_c {
            let mut c = Table::new();
            c["std"] = value(std.as_str());
            lang["c"] = Item::Table(c);
        }
        if let Some(std) = &spec.lang_cpp {
            let mut cpp = Table::new();
            cpp["std"] = value(std.as_str());
            lang["cpp"] = Item::Table(cpp);
        }
        doc["language"] = Item::Table(lang);
    }

    // [build] defines
    if !spec.defines.is_empty() {
        let mut build = Table::new();
        let mut arr = Array::new();
        for d in &spec.defines {
            arr.push(d.as_str());
        }
        build["defines"] = value(arr);
        doc["build"] = Item::Table(build);
    }

    // [[bin]] / [[lib]]
    for target in &spec.targets {
        match target.kind {
            TargetKind::Bin => {
                let mut tbl = Table::new();
                tbl["name"] = value(target.name.as_str());
                if let Some(src) = &target.src {
                    tbl["src"] = value(src.as_str());
                }
                doc["bin"]
                    .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                    .as_array_of_tables_mut()
                    .unwrap()
                    .push(tbl);
            }
            TargetKind::StaticLib => {
                let mut tbl = Table::new();
                tbl["name"] = value(target.name.as_str());
                tbl["type"] = value("static");
                doc["lib"]
                    .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                    .as_array_of_tables_mut()
                    .unwrap()
                    .push(tbl);
            }
            TargetKind::DynamicLib => {
                let mut tbl = Table::new();
                tbl["name"] = value(target.name.as_str());
                tbl["type"] = value("shared");
                doc["lib"]
                    .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
                    .as_array_of_tables_mut()
                    .unwrap()
                    .push(tbl);
            }
        }
    }

    // [dependencies]
    if !spec.system_deps.is_empty() {
        let mut deps = Table::new();
        for dep in &spec.system_deps {
            deps[dep.as_str()] = super::system_dep_item(dep);
        }
        doc["dependencies"] = Item::Table(deps);
    }

    // [os.*] features — system libraries (pthread, m, ws2_32, …). Emitted before
    // the dependency sub-tables so the `[os.<os>]` header precedes `.dependencies`.
    for (os_key, feats) in &spec.os_features {
        add_os_features_section(&mut doc, os_key, feats);
    }

    // [os.*.dependencies] — from ifeq conditional blocks
    add_os_deps_section(&mut doc, "windows", &spec.conditional_deps.windows);
    add_os_deps_section(&mut doc, "linux", &spec.conditional_deps.linux);
    add_os_deps_section(&mut doc, "macos", &spec.conditional_deps.macos);
    add_os_deps_section(&mut doc, "unix", &spec.conditional_deps.unix);

    doc.to_string()
}

fn add_os_features_section(doc: &mut DocumentMut, os_key: &str, features: &[String]) {
    if features.is_empty() {
        return;
    }
    if !doc.contains_key("os") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc["os"] = Item::Table(t);
    }
    let os_tbl = doc["os"].as_table_mut().expect("os is a table");
    if !os_tbl.contains_key(os_key) {
        let mut t = Table::new();
        t.set_implicit(true);
        os_tbl[os_key] = Item::Table(t);
    }
    let platform_tbl = os_tbl[os_key].as_table_mut().expect("platform is a table");
    let mut arr = toml_edit::Array::new();
    for f in features {
        arr.push(f.as_str());
    }
    platform_tbl["features"] = Item::Value(arr.into());
}

fn add_os_deps_section(doc: &mut DocumentMut, os_key: &str, deps: &[String]) {
    if deps.is_empty() {
        return;
    }
    if !doc.contains_key("os") {
        let mut t = Table::new();
        t.set_implicit(true);
        doc["os"] = Item::Table(t);
    }
    let os_tbl = doc["os"].as_table_mut().expect("os is a table");
    if !os_tbl.contains_key(os_key) {
        let mut t = Table::new();
        t.set_implicit(true);
        os_tbl[os_key] = Item::Table(t);
    }
    let platform_tbl = os_tbl[os_key].as_table_mut().expect("platform is a table");
    let mut dep_tbl = Table::new();
    for dep in deps {
        dep_tbl[dep.as_str()] = super::system_dep_item(dep);
    }
    platform_tbl["dependencies"] = Item::Table(dep_tbl);
}

// ── Conditional (ifeq/endif) parsing ─────────────────────────────────────────

/// Scan the raw Makefile text for top-level `ifeq`/`ifneq` blocks whose
/// condition references an OS-detection variable, and collect -l flags found
/// inside each branch into the appropriate OS bucket.
fn parse_conditional_deps(content: &str) -> ConditionalDeps {
    let mut result = ConditionalDeps::default();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();
        let negated = t.starts_with("ifneq ");
        let is_cond = t.starts_with("ifeq ") || negated;

        if is_cond {
            let keyword = if negated { "ifneq" } else { "ifeq" };
            let cond_str = t[keyword.len()..].trim();
            let os_key = classify_make_condition(cond_str);

            let mut then_libs: Vec<String> = Vec::new();
            let mut else_libs: Vec<String> = Vec::new();
            let mut in_else = false;
            let mut depth = 1usize;
            i += 1;

            while i < lines.len() {
                let it = lines[i].trim();
                if it.starts_with("ifeq")
                    || it.starts_with("ifneq")
                    || it.starts_with("ifdef")
                    || it.starts_with("ifndef")
                {
                    depth += 1;
                } else if it.starts_with("endif") {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                } else if it == "else" && depth == 1 {
                    in_else = true;
                    i += 1;
                    continue;
                }
                if depth == 1 {
                    let libs = filter_auto_detected(extract_libs_from_line(it));
                    let target = if in_else {
                        &mut else_libs
                    } else {
                        &mut then_libs
                    };
                    for lib in libs {
                        if !target.contains(&lib) {
                            target.push(lib);
                        }
                    }
                }
                i += 1;
            }

            if let Some(os) = os_key {
                let (then_os, else_os) = if negated {
                    (invert_os(os), os)
                } else {
                    (os, invert_os(os))
                };
                push_unique(cond_bucket(&mut result, then_os), then_libs);
                push_unique(cond_bucket(&mut result, else_os), else_libs);
            }
        } else {
            i += 1;
        }
    }

    result
}

/// Classify an `ifeq` condition string into a target OS, or return `None`.
///
/// Handles `($(OS),Windows_NT)`, `($(UNAME_S),Linux)`, and double-quote forms.
/// Requires the LHS to reference a known OS-detection variable name to avoid
/// false positives from unrelated conditions like `ifeq ($(MAKECMDGOALS),linux)`.
fn classify_make_condition(cond: &str) -> Option<&'static str> {
    let cond = cond.trim().trim_start_matches('(');
    let (lhs, rhs) = if let Some((l, r)) = cond.split_once(',') {
        (l, r)
    } else {
        // Double-quote form: `"$(OS)" "Windows_NT"` — split on whitespace
        let mut parts = cond.split_whitespace();
        let l = parts.next().unwrap_or("");
        let r = parts.next().unwrap_or("");
        (l, r)
    };

    let lhs_lower = lhs.to_ascii_lowercase();
    let rhs = rhs.trim().trim_matches([')', '"', '\'', ' ']);
    let rhs_lower = rhs.to_ascii_lowercase();

    // LHS must reference an OS-detection variable
    let is_os_var = lhs_lower.contains("os")
        || lhs_lower.contains("uname")
        || lhs_lower.contains("system")
        || lhs_lower.contains("platform")
        || lhs_lower.contains("host");
    if !is_os_var {
        return None;
    }

    if rhs_lower == "windows_nt"
        || rhs_lower.contains("mingw")
        || rhs_lower.contains("msys")
        || rhs_lower.contains("win32")
    {
        Some("windows")
    } else if rhs_lower == "linux" {
        Some("linux")
    } else if rhs_lower == "darwin" {
        Some("macos")
    } else if rhs_lower == "freebsd"
        || rhs_lower == "openbsd"
        || rhs_lower == "netbsd"
        || rhs_lower == "dragonfly"
    {
        Some("unix")
    } else {
        None
    }
}

fn invert_os(os: &'static str) -> &'static str {
    match os {
        "windows" => "unix",
        _ => "windows",
    }
}

fn cond_bucket<'a>(result: &'a mut ConditionalDeps, os: &str) -> &'a mut Vec<String> {
    match os {
        "windows" => &mut result.windows,
        "linux" => &mut result.linux,
        "macos" => &mut result.macos,
        _ => &mut result.unix,
    }
}

fn push_unique(vec: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !vec.contains(&item) {
            vec.push(item);
        }
    }
}

/// Extract package names from `$(shell pkg-config --libs foo bar)` expressions.
///
/// We can't evaluate shell calls, but we can recognise the pkg-config idiom and
/// pull out the package names — they become `dep = "*"` entries that freight then
/// resolves via its own pkg-config integration.
fn extract_pkgconfig_libs(s: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut pos = 0;
    while let Some(idx) = s[pos..].find("pkg-config") {
        let abs = pos + idx;
        // Grab everything up to the end of the surrounding $(...) or backtick block
        let after = &s[abs + 10..];
        let end = after.find([')', '`']).unwrap_or(after.len());
        let args = &after[..end];
        for tok in args.split_whitespace() {
            // Skip pkg-config flags
            if tok.starts_with("--") || tok == "pkg-config" {
                continue;
            }
            let pkg =
                tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            if !pkg.is_empty() && !deps.contains(&pkg.to_string()) {
                deps.push(pkg.to_string());
            }
        }
        pos = abs + 1;
    }
    deps
}

/// Scan raw Makefile text for `ifdef`/`ifndef` blocks that look OS-specific,
/// collect -l flags from their bodies into conditional buckets.  Blocks whose
/// variable name can't be classified go into `ConditionalDeps::unix` (best guess
/// for most platform guards) and the caller emits a warning.
fn parse_ifdef_deps(content: &str, result: &mut ConditionalDeps) -> Vec<String> {
    let mut unknown_body_deps: Vec<String> = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();
        let is_ifdef = t.starts_with("ifdef ");
        let is_ifndef = t.starts_with("ifndef ");
        if !is_ifdef && !is_ifndef {
            i += 1;
            continue;
        }

        let keyword_len = if is_ifdef { 6 } else { 7 };
        let var_name = t[keyword_len..].trim().to_ascii_uppercase();

        // Try to classify the variable name by OS
        let os_key: Option<&'static str> = if var_name.contains("WIN")
            || var_name.contains("MSVC")
            || var_name.contains("MINGW")
        {
            Some("windows")
        } else if var_name.contains("LINUX") || var_name.contains("GNU") {
            Some("linux")
        } else if var_name.contains("DARWIN")
            || var_name.contains("APPLE")
            || var_name.contains("MACOS")
        {
            Some("macos")
        } else {
            None
        };

        let mut then_libs: Vec<String> = Vec::new();
        let mut else_libs: Vec<String> = Vec::new();
        let mut in_else = false;
        let mut depth = 1usize;
        i += 1;

        while i < lines.len() {
            let it = lines[i].trim();
            if it.starts_with("ifeq")
                || it.starts_with("ifneq")
                || it.starts_with("ifdef")
                || it.starts_with("ifndef")
            {
                depth += 1;
            } else if it.starts_with("endif") {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
            } else if it == "else" && depth == 1 {
                in_else = true;
                i += 1;
                continue;
            }
            if depth == 1 {
                let libs = filter_auto_detected(extract_libs_from_line(it));
                let target = if in_else {
                    &mut else_libs
                } else {
                    &mut then_libs
                };
                for lib in libs {
                    if !target.contains(&lib) {
                        target.push(lib);
                    }
                }
            }
            i += 1;
        }

        if let Some(os) = os_key {
            let (then_os, else_os) = if is_ifndef {
                (invert_os(os), os)
            } else {
                (os, invert_os(os))
            };
            push_unique(cond_bucket(result, then_os), then_libs);
            push_unique(cond_bucket(result, else_os), else_libs);
        } else {
            // Unclassifiable guard — add all body deps to unknown bucket
            push_unique(&mut unknown_body_deps, then_libs);
            push_unique(&mut unknown_body_deps, else_libs);
        }
    }

    unknown_body_deps
}

/// Extract -l<lib> flags from any line, stripping assignment operators first.
fn extract_libs_from_line(line: &str) -> Vec<String> {
    let rhs = if let Some(p) = line.find("+=") {
        &line[p + 2..]
    } else if let Some(p) = line.find(":=") {
        &line[p + 2..]
    } else if let Some(p) = line.find('=') {
        let before = line[..p].trim();
        if before.ends_with(['!', '<', '>', '=']) {
            line
        } else {
            &line[p + 1..]
        }
    } else {
        line
    };
    extract_libs(rhs)
}

// ── Variable handling ─────────────────────────────────────────────────────────

struct ExpandedVars {
    raw: HashMap<String, String>,
}

impl ExpandedVars {
    fn new(raw: HashMap<String, String>) -> Self {
        Self { raw }
    }

    fn raw(&self, name: &str) -> Option<String> {
        self.raw.get(name).cloned()
    }

    fn get(&self, name: &str) -> String {
        let val = self.raw.get(name).map(|s| s.as_str()).unwrap_or("");
        self.expand(val, 0)
    }

    fn get_joined(&self, names: &[&str]) -> String {
        names
            .iter()
            .map(|n| self.get(n))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn get_first_nonempty(&self, names: &[&str]) -> Option<String> {
        names.iter().find_map(|n| {
            let v = self.get(n);
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        })
    }

    fn expand(&self, val: &str, depth: usize) -> String {
        if depth > 8 {
            return val.to_string();
        }
        let mut out = String::with_capacity(val.len());
        let mut chars = val.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            match chars.peek() {
                Some('(') => {
                    chars.next();
                    let name: String = chars.by_ref().take_while(|&c| c != ')').collect();
                    // Skip shell/function calls — leave unexpanded
                    if name.starts_with("shell")
                        || name.starts_with("wildcard")
                        || name.contains(' ')
                    {
                        out.push_str(&format!("$({name})"));
                    } else if let Some(v) = self.raw.get(&name) {
                        out.push_str(&self.expand(v, depth + 1));
                    }
                }
                Some('{') => {
                    chars.next();
                    let name: String = chars.by_ref().take_while(|&c| c != '}').collect();
                    if name.starts_with("shell") || name.contains(' ') {
                        out.push_str(&format!("${{{name}}}"));
                    } else if let Some(v) = self.raw.get(&name) {
                        out.push_str(&self.expand(v, depth + 1));
                    }
                }
                _ => out.push(c),
            }
        }
        out
    }
}

fn is_append_assignment(def: &VariableDefinition) -> bool {
    let text = def.to_string();
    let name = def.name().unwrap_or_default();
    text.find(&name)
        .map(|i| text[i + name.len()..].trim_start().starts_with("+="))
        .unwrap_or(false)
}

fn is_conditional_assignment(def: &VariableDefinition) -> bool {
    let text = def.to_string();
    let name = def.name().unwrap_or_default();
    text.find(&name)
        .map(|i| text[i + name.len()..].trim_start().starts_with("?="))
        .unwrap_or(false)
}

fn collect_vars(mf: &Makefile) -> HashMap<String, String> {
    let mut vars: HashMap<String, String> = HashMap::new();
    for def in mf.variable_definitions() {
        let Some(name) = def.name() else { continue };
        let val = def.raw_value().unwrap_or_default();
        if is_append_assignment(&def) {
            let entry = vars.entry(name).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(&val);
        } else if is_conditional_assignment(&def) {
            // ?= only sets the variable if it is not already defined
            vars.entry(name).or_insert(val);
        } else {
            vars.insert(name, val);
        }
    }
    vars
}

// ── Subdir / workspace detection ──────────────────────────────────────────────

fn find_subdirs(mf: &Makefile, vars: &ExpandedVars) -> Vec<String> {
    for name in &["SUBDIRS", "DIRS", "MODULES", "SUBDIR", "SUBDIRECTORIES"] {
        let val = vars.get(name);
        if !val.is_empty() {
            return val.split_whitespace().map(str::to_string).collect();
        }
    }
    // Scan recipes for $(MAKE) -C <dir>
    let mut subdirs = Vec::new();
    for rule in mf.rules() {
        for recipe in rule.recipes() {
            if let Some(pos) = recipe.find("-C ") {
                let after = recipe[pos + 3..].trim();
                let dir = after.split_whitespace().next().unwrap_or("");
                if !dir.is_empty() && !dir.starts_with('$') && !subdirs.contains(&dir.to_string()) {
                    subdirs.push(dir.to_string());
                }
            }
        }
    }
    subdirs
}

// ── Flag parsing helpers ──────────────────────────────────────────────────────

fn extract_std(flags: &str, lang_hint: char) -> Option<String> {
    for token in flags.split_whitespace() {
        if let Some(rest) = token.strip_prefix("-std=") {
            // Normalise: gnu++17 → c++17, gnu11 → c11
            let std = rest.replace("gnu", "c");
            if lang_hint == '+' && std.contains("++") {
                return Some(std);
            }
            if lang_hint == 'c' && !std.contains("++") {
                return Some(std);
            }
        }
    }
    None
}

fn extract_defines(flags: &str) -> Vec<String> {
    flags
        .split_whitespace()
        .filter_map(|t| t.strip_prefix("-D").map(str::to_string))
        .filter(|d| !d.is_empty())
        .collect()
}

fn extract_libs(flags: &str) -> Vec<String> {
    flags
        .split_whitespace()
        .filter_map(|t| t.strip_prefix("-l").map(str::to_string))
        .filter(|l| !l.is_empty())
        .collect()
}

/// Libraries that freight auto-detects via pkg-config — don't emit them.
/// Drop only the compiler-driver libs (libc, libgcc, libstdc++ …). OS system
/// libraries (pthread, m, ws2_32 …) are kept here and later routed to
/// `[os.<os>] features` by [`super::split_link_libs`].
fn filter_auto_detected(libs: Vec<String>) -> Vec<String> {
    libs.into_iter()
        .filter(|l| !super::DRIVER_LINKED.contains(&l.as_str()))
        .collect()
}

fn is_link_recipe(recipe: &str) -> bool {
    let r = recipe.trim_start_matches('@').trim();
    r.starts_with("$(CC)")
        || r.starts_with("$(CXX)")
        || r.starts_with("$(LD)")
        || r.starts_with("gcc")
        || r.starts_with("g++")
        || r.starts_with("cc")
        || r.starts_with("clang")
}

// ── Source-file helpers ───────────────────────────────────────────────────────

/// Return `(stem, project-root-relative-path)` for every source file under
/// `search_root` that contains `main()`. Paths are made relative to `project_dir`
/// so they can be written directly into `[[bin]] src = "..."`.
fn find_entry_points(search_root: &Path, project_dir: &Path) -> Vec<(String, String)> {
    let src_exts = ["c", "cpp", "cxx", "cc", "C"];
    let mut out = Vec::new();
    for entry in WalkDir::new(search_root).max_depth(3).into_iter().flatten() {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        if !src_exts.contains(&ext) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if content.contains("int main(") || content.contains("int main (") {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let rel = path.strip_prefix(project_dir).unwrap_or(path);
            out.push((stem, rel.to_string_lossy().into_owned()));
        }
    }
    out
}

fn has_main_function(dir: &Path) -> bool {
    let src_exts = ["c", "cpp", "cxx", "cc", "C"];
    WalkDir::new(dir)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| src_exts.contains(&x))
                .unwrap_or(false)
        })
        .any(|e| {
            std::fs::read_to_string(e.path())
                .map(|s| s.contains("int main(") || s.contains("int main ("))
                .unwrap_or(false)
        })
}

// ── Name helpers ──────────────────────────────────────────────────────────────

fn strip_lib_ext(name: &str) -> String {
    // Strip versioned suffix first (libbaz.so.1 → libbaz.so), then static/shared ext
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    // If still ends with .so (versioned lib), strip it again
    let stem = if stem.ends_with(".so") {
        &stem[..stem.len() - 3]
    } else {
        stem
    };
    // Strip trailing .a or .so extensions via another pass
    let stem = stem
        .strip_suffix(".a")
        .or_else(|| stem.strip_suffix(".so"))
        .unwrap_or(stem);
    // Strip leading "lib" prefix
    let stripped = stem.strip_prefix("lib").unwrap_or(stem);
    sanitize_name(stripped)
}

// ── Input resolution ──────────────────────────────────────────────────────────

fn resolve_input(input: &Path) -> Result<(PathBuf, PathBuf)> {
    if input.is_dir() {
        let mf = find_makefile(input)
            .ok_or_else(|| anyhow::anyhow!("no Makefile found in {}", input.display()))?;
        Ok((input.to_path_buf(), mf))
    } else {
        let dir = input.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok((dir, input.to_path_buf()))
    }
}

fn find_makefile(dir: &Path) -> Option<PathBuf> {
    for name in &["GNUmakefile", "Makefile", "makefile"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

// ── Purge ─────────────────────────────────────────────────────────────────────

/// Remove Makefile build-system artefacts from `dir`. Returns messages about
/// what was removed (for display by the CLI layer).
pub fn purge_make(dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    for name in &["GNUmakefile", "Makefile", "makefile"] {
        let p = dir.join(name);
        if p.exists() && std::fs::remove_file(&p).is_ok() {
            removed.push(format!("removed {}", p.display()));
        }
    }
    removed
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> ExpandedVars {
        ExpandedVars::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn extracts_cpp_std() {
        assert_eq!(
            extract_std("-O2 -std=c++17 -Wall", '+'),
            Some("c++17".into())
        );
        assert_eq!(extract_std("-std=gnu++20", '+'), Some("c++20".into()));
        assert_eq!(extract_std("-std=c11", 'c'), Some("c11".into()));
        assert_eq!(extract_std("-std=c++17", 'c'), None);
    }

    #[test]
    fn extracts_libs() {
        let libs = extract_libs("-lfoo -lbar -lbaz");
        assert_eq!(libs, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn filters_driver_libs_keeps_system_libs() {
        // Only compiler-driver libs are dropped here; OS system libs (pthread, m,
        // dl) are kept and routed to `[os.*] features` later by analyze().
        let raw = vec![
            "c".into(),
            "gcc".into(),
            "stdc++".into(),
            "ssl".into(),
            "pthread".into(),
        ];
        assert_eq!(filter_auto_detected(raw), vec!["ssl", "pthread"]);
    }

    #[test]
    fn extracts_defines() {
        let defs = extract_defines("-DNDEBUG -DVERSION=2 -Wall");
        assert_eq!(defs, vec!["NDEBUG", "VERSION=2"]);
    }

    #[test]
    fn classify_target_bin() {
        let s = classify_target("myprog").unwrap();
        assert_eq!(s.name, "myprog");
        assert_eq!(s.kind, TargetKind::Bin);
    }

    #[test]
    fn classify_target_static_lib() {
        let s = classify_target("libfoo.a").unwrap();
        assert_eq!(s.name, "foo");
        assert_eq!(s.kind, TargetKind::StaticLib);
    }

    #[test]
    fn classify_target_dynamic_lib() {
        let s = classify_target("libbar.so").unwrap();
        assert_eq!(s.name, "bar");
        assert_eq!(s.kind, TargetKind::DynamicLib);
    }

    #[test]
    fn var_expansion_simple() {
        let v = vars(&[("CC", "gcc"), ("CFLAGS", "-O2 $(CC)")]);
        assert_eq!(v.get("CFLAGS"), "-O2 gcc");
    }

    #[test]
    fn var_expansion_leaves_shell_unexpanded() {
        let v = vars(&[("SRCS", "$(shell find . -name '*.c')")]);
        assert!(v.get("SRCS").contains("$(shell"));
    }

    #[test]
    fn strip_lib_ext_variants() {
        assert_eq!(strip_lib_ext("libfoo.a"), "foo");
        assert_eq!(strip_lib_ext("libbar.so"), "bar");
        assert_eq!(strip_lib_ext("libbaz.so.1"), "baz"); // versioned .so: strip version then .so
    }

    #[test]
    fn classify_condition_windows() {
        assert_eq!(
            classify_make_condition("($(OS),Windows_NT)"),
            Some("windows")
        );
        assert_eq!(
            classify_make_condition("($(OS), Windows_NT)"),
            Some("windows")
        );
        assert_eq!(
            classify_make_condition("\"$(OS)\" \"Windows_NT\""),
            Some("windows")
        );
    }

    #[test]
    fn classify_condition_linux() {
        assert_eq!(classify_make_condition("($(UNAME_S),Linux)"), Some("linux"));
        assert_eq!(
            classify_make_condition("($(UNAME_S), linux)"),
            Some("linux")
        );
    }

    #[test]
    fn classify_condition_macos() {
        assert_eq!(
            classify_make_condition("($(UNAME_S),Darwin)"),
            Some("macos")
        );
    }

    #[test]
    fn classify_condition_ignores_unrelated() {
        // MAKECMDGOALS contains "linux" as a target name — should not match
        assert_eq!(classify_make_condition("($(MAKECMDGOALS),linux)"), None);
        assert_eq!(classify_make_condition("($(CC),gcc)"), None);
    }

    #[test]
    fn conditional_windows_deps_extracted() {
        let content = "ifeq ($(OS),Windows_NT)\nLDFLAGS += -lws2_32 -lshlwapi\nendif\n";
        let deps = parse_conditional_deps(content);
        assert!(deps.windows.contains(&"ws2_32".to_string()));
        assert!(deps.windows.contains(&"shlwapi".to_string()));
        assert!(deps.linux.is_empty());
    }

    #[test]
    fn conditional_else_branch_goes_to_unix() {
        let content =
            "ifeq ($(OS),Windows_NT)\nLDFLAGS += -lws2_32\nelse\nLDFLAGS += -lssl\nendif\n";
        let deps = parse_conditional_deps(content);
        assert!(deps.windows.contains(&"ws2_32".to_string()));
        assert!(deps.unix.contains(&"ssl".to_string()));
    }

    #[test]
    fn conditional_keeps_system_libs_at_parse() {
        // Parse keeps system libs (rt); analyze() later routes them to
        // `[os.*] features`. Driver libs would be dropped; real deps (ssl) stay.
        let content = "ifeq ($(UNAME_S),Linux)\nLDFLAGS += -lrt -lssl\nendif\n";
        let deps = parse_conditional_deps(content);
        assert!(deps.linux.contains(&"rt".to_string()));
        assert!(deps.linux.contains(&"ssl".to_string()));
    }

    #[test]
    fn emit_toml_system_deps_as_individual_entries() {
        let spec = ProjectSpec {
            name: "myapp".to_string(),
            version: "0.1.0".to_string(),
            targets: vec![TargetSpec {
                name: "myapp".to_string(),
                kind: TargetKind::Bin,
                src: None,
            }],
            lang_c: None,
            lang_cpp: None,
            system_deps: vec!["ssl".to_string(), "curl".to_string()],
            conditional_deps: ConditionalDeps::default(),
            os_features: Default::default(),
            defines: vec![],
            warnings: vec![],
        };
        let toml = emit_toml(&spec);
        // Version is pkg-config's `--modversion` when known, else `*`; assert on
        // the dep key, not the version.
        assert!(toml.contains("ssl ="), "expected individual ssl dep:\n{toml}");
        assert!(toml.contains("curl ="), "expected individual curl dep:\n{toml}");
        // Real deps must not be grouped as features.
        assert!(
            !toml.contains("features"),
            "must not group deps as features:\n{toml}"
        );
    }

    #[test]
    fn emit_toml_windows_syslib_as_feature() {
        let mut os_features = std::collections::BTreeMap::new();
        os_features.insert("windows".to_string(), vec!["ws2_32".to_string()]);
        let spec = ProjectSpec {
            name: "myapp".to_string(),
            version: "0.1.0".to_string(),
            targets: vec![TargetSpec {
                name: "myapp".to_string(),
                kind: TargetKind::Bin,
                src: None,
            }],
            lang_c: None,
            lang_cpp: None,
            system_deps: vec![],
            conditional_deps: ConditionalDeps::default(),
            os_features,
            defines: vec![],
            warnings: vec![],
        };
        let toml = emit_toml(&spec);
        // ws2_32 is a system library → `[os.windows] features`, not a dep.
        assert!(toml.contains("[os.windows]"), "expected os.windows:\n{toml}");
        assert!(
            toml.contains("features = [\"ws2_32\"]"),
            "ws2_32 should be a windows feature:\n{toml}"
        );
        assert!(
            !toml.contains("[dependencies]"),
            "ws2_32 must not appear in deps:\n{toml}"
        );
    }

    #[test]
    fn conditional_assign_does_not_override() {
        let content = "CFLAGS = -O2\nCFLAGS ?= -g\n";
        let mf = Makefile::read_relaxed(content.as_bytes()).unwrap();
        let vars = ExpandedVars::new(collect_vars(&mf));
        // ?= should not override the already-set value
        assert_eq!(vars.get("CFLAGS"), "-O2");
    }

    #[test]
    fn conditional_assign_sets_when_absent() {
        let content = "CFLAGS ?= -O2\n";
        let mf = Makefile::read_relaxed(content.as_bytes()).unwrap();
        let vars = ExpandedVars::new(collect_vars(&mf));
        assert_eq!(vars.get("CFLAGS"), "-O2");
    }

    #[test]
    fn pkgconfig_libs_extracted() {
        let libs = extract_pkgconfig_libs("$(shell pkg-config --libs openssl libcurl)");
        assert!(libs.contains(&"openssl".to_string()));
        assert!(libs.contains(&"libcurl".to_string()));
    }

    #[test]
    fn pkgconfig_flags_skipped() {
        let libs = extract_pkgconfig_libs("$(shell pkg-config --cflags --libs zlib)");
        assert!(libs.contains(&"zlib".to_string()));
        assert!(!libs.contains(&"--cflags".to_string()));
    }

    #[test]
    fn ifdef_windows_routes_to_windows_bucket() {
        let content = "ifdef WINDIR\nLDFLAGS += -lws2_32\nendif\n";
        let mut result = ConditionalDeps::default();
        parse_ifdef_deps(content, &mut result);
        assert!(result.windows.contains(&"ws2_32".to_string()));
    }

    #[test]
    fn ifdef_unknown_var_goes_to_system_deps_via_analyze() {
        let content = "ifdef HAVE_ZLIB\nLDFLAGS += -lz\nendif\n";
        let mf = Makefile::read_relaxed(content.as_bytes()).unwrap();
        let vars = ExpandedVars::new(collect_vars(&mf));
        let mut warnings = vec![];
        let _spec = analyze(&mf, &vars, Path::new("/tmp"), content, &mut warnings);
        // z is in AUTO_LINKED so it will be filtered out, but the warning should fire
        assert!(
            warnings.iter().any(|w| w.contains("ifdef")),
            "expected ifdef warning:\n{warnings:?}"
        );
    }

    #[test]
    fn find_subdirs_from_variable() {
        let content = "SUBDIRS = liba libb\nall:\n\t$(MAKE) -C liba\n";
        let mf: Makefile = content.parse().unwrap();
        let vars = ExpandedVars::new(collect_vars(&mf));
        let dirs = find_subdirs(&mf, &vars);
        assert_eq!(dirs, vec!["liba", "libb"]);
    }

    #[test]
    fn full_parse_simple_makefile() {
        let content = "\
CC = gcc
CFLAGS = -std=c11 -O2 -DNDEBUG
LDLIBS = -lssl -lm
TARGET = myapp

all: $(TARGET)

$(TARGET): main.o
\t$(CC) -o $@ $^ $(LDLIBS)
";
        let mf = Makefile::read_relaxed(content.as_bytes()).unwrap();
        let vars = ExpandedVars::new(collect_vars(&mf));
        let mut warnings = vec![];
        let spec = analyze(&mf, &vars, Path::new("/tmp"), content, &mut warnings);

        assert_eq!(spec.name, "myapp");
        assert_eq!(spec.lang_c, Some("c11".into()));
        assert_eq!(spec.system_deps, vec!["ssl"]); // m is in AUTO_LINKED, filtered
        assert_eq!(spec.defines, vec!["NDEBUG"]);
        assert_eq!(spec.targets.len(), 1);
        assert_eq!(spec.targets[0].kind, TargetKind::Bin);
    }
}
