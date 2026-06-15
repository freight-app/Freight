/// Import a CMake project (CMakeLists.txt) into freight.toml.
///
/// Parsing is done by `cmake-lossless`, which correctly handles all three CMake
/// argument types (bracket, quoted, unquoted), multi-line calls, block structure
/// (if/foreach/function/macro), and nested argument groups.
///
/// Analysis is static only — CMake is Turing-complete so variable expansion is
/// best-effort and conditionals are never evaluated.  What we *do* extract:
///
///   - `project(name [VERSION x.y] …)` → package name + version
///   - `add_executable(name srcs…)` → [[bin]] targets
///   - `add_library(name [STATIC|SHARED] srcs…)` → [[lib]] targets
///   - `target_link_libraries(…)` / `link_libraries(…)` → system deps
///   - `find_package(name …)` / `pkg_check_modules(…)` → dependencies
///   - `set(CMAKE_CXX_STANDARD n)` / `set(CMAKE_C_STANDARD n)` → language std
///   - `include_directories(…)` / `target_include_directories(…)` → includes
///   - `add_definitions(…)` / `target_compile_definitions(…)` → defines
///   - `add_subdirectory(path)` → workspace members
///   - `if(WIN32)/if(MSVC)` blocks → deps go to `[os.windows.dependencies]`
///   - `if(APPLE)` blocks → deps go to `[os.macos.dependencies]`
///   - `if(UNIX)` blocks → deps go to `[os.unix.dependencies]`
///   - OS system libraries (pthread, m, ws2_32, …) and `find_package(Threads)` →
///     `[os.<os>] features = [...]` (linked via `-l`, not packages)
///   - resolved deps are version-pinned via pkg-config `--modversion` (else `*`)
///   - `function()` and `macro()` bodies are skipped (can't evaluate calls)
///   - `FetchContent_Declare(name GIT_REPOSITORY … GIT_TAG …)` → `{ git, tag }`
///   - `FetchContent_Declare(name URL … URL_HASH SHA256=…)` → `{ url, sha256 }`
///   - `ExternalProject_Add(name …)` → same rules; custom build steps warned
///   - `CPMAddPackage(NAME … GITHUB_REPOSITORY … GIT_TAG …)` → `{ git, tag }`
///   - `CPMAddPackage("gh:user/repo#tag")` compact form
///   - `add_compile_options(…)` / `target_compile_options(tgt … flags…)` → `-std=` → lang std, `-D` → defines
///   - `option(NAME "desc" ON|OFF)` / `cmake_dependent_option(…)` → `[features]`
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cmake_lossless::{CMakeFile, CommandInvocation, Node};
use toml_edit::{value, Array, DocumentMut, InlineTable, Item, Table};

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ImportResult {
    pub written: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Purge CMake artefacts left behind in `dir`.
pub fn purge_cmake(dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let files = [
        "CMakeLists.txt",
        "CMakeCache.txt",
        "cmake_install.cmake",
        "CTestTestfile.cmake",
    ];
    for name in &files {
        let p = dir.join(name);
        if p.exists() && std::fs::remove_file(&p).is_ok() {
            removed.push(format!("removed {}", p.display()));
        }
    }
    let cmake_files = dir.join("CMakeFiles");
    if cmake_files.is_dir() && std::fs::remove_dir_all(&cmake_files).is_ok() {
        removed.push(format!("removed {}/", cmake_files.display()));
    }
    let build_dir = dir.join("build");
    if build_dir.join("CMakeCache.txt").exists() && std::fs::remove_dir_all(&build_dir).is_ok() {
        removed.push(format!("removed {}/", build_dir.display()));
    }
    removed
}

pub fn import_cmake(input: &Path, out_dir: Option<&Path>) -> Result<ImportResult> {
    let (project_dir, cmake_path) = resolve_input(input)?;
    let out_root = out_dir.unwrap_or(&project_dir);
    let mut warnings: Vec<String> = Vec::new();

    let content = std::fs::read_to_string(&cmake_path)
        .with_context(|| format!("reading {}", cmake_path.display()))?;

    let file = cmake_lossless::parse(&content).map_err(|e| anyhow::anyhow!("{}", e))?;

    let parsed = extract(&file, &mut warnings);

    // ── Workspace detection ───────────────────────────────────────────────────
    // Only go full-workspace if there are subdirs but NO root-level targets.
    let root_has_targets = !parsed.bins.is_empty() || !parsed.libs.is_empty();
    if !parsed.subdirs.is_empty() && !root_has_targets {
        return import_workspace(&project_dir, out_root, &parsed.subdirs, &mut warnings);
    }

    // ── Package name + version ────────────────────────────────────────────────
    let dir_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();
    let pkg_name = if parsed.name.is_empty() {
        sanitize_name(&dir_name)
    } else {
        sanitize_name(&parsed.name)
    };
    let pkg_version = if parsed.version.is_empty() || parsed.version.contains('$') {
        "0.1.0".to_string()
    } else {
        ensure_three_part(&parsed.version)
    };

    let toml = emit_toml(&pkg_name, &pkg_version, &parsed, &warnings);
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
    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;

    let mut doc = DocumentMut::new();
    let mut ws_tbl = Table::new();
    let mut members = Array::new();
    for sub in subdirs {
        members.push(sub.as_str());
    }
    ws_tbl.insert("members", Item::Value(members.into()));
    doc.insert("workspace", Item::Table(ws_tbl));

    let root_toml = out_root.join("freight.toml");
    std::fs::write(&root_toml, doc.to_string())
        .with_context(|| format!("writing {}", root_toml.display()))?;
    written.push(root_toml);

    for sub in subdirs {
        let sub_dir = project_dir.join(sub);
        let cmake_path = sub_dir.join("CMakeLists.txt");
        if !cmake_path.exists() {
            warnings.push(format!(
                "subdirectory {sub} has no CMakeLists.txt — skipping"
            ));
            continue;
        }
        let sub_out = out_root.join(sub);
        match import_cmake(&sub_dir, Some(&sub_out)) {
            Ok(r) => {
                written.extend(r.written);
                warnings.extend(r.warnings);
            }
            Err(e) => warnings.push(format!("could not convert {sub}: {e}")),
        }
    }

    Ok(ImportResult {
        written,
        warnings: warnings.clone(),
    })
}

// ── Input resolution ──────────────────────────────────────────────────────────

fn resolve_input(input: &Path) -> Result<(PathBuf, PathBuf)> {
    if input.is_dir() {
        let cmake = input.join("CMakeLists.txt");
        anyhow::ensure!(cmake.exists(), "no CMakeLists.txt in {}", input.display());
        Ok((input.to_path_buf(), cmake))
    } else {
        let dir = input.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok((dir, input.to_path_buf()))
    }
}

// ── Extracted data ────────────────────────────────────────────────────────────

/// A dep fetched at configure-time by FetchContent, ExternalProject, or CPM.
struct FetchedDep {
    name: String,
    tag: Option<String>,
    branch: Option<String>,
    rev: Option<String>,
    url: Option<String>,
    sha256: Option<String>,
}

struct Extracted {
    name: String,
    version: String,
    bins: Vec<(String, Vec<String>)>,
    libs: Vec<(String, LibKind, Vec<String>)>,
    deps: Vec<String>,
    find_packages: Vec<String>,
    pkg_modules: Vec<String>,
    fetched_deps: Vec<FetchedDep>,
    /// Platform-conditional deps: OS name (e.g. "windows", "macos", "unix") → dep list.
    platform_deps: HashMap<String, Vec<String>>,
    /// System-library link features per OS (`[os.<os>] features`): pthread, m,
    /// ws2_32, … — versionless OS libraries linked via `-l`, not packages.
    os_features: HashMap<String, Vec<String>>,
    c_std: Option<String>,
    cxx_std: Option<String>,
    defines: Vec<String>,
    includes: Vec<String>,
    subdirs: Vec<String>,
    vars: HashMap<String, Vec<String>>, // name → list of values (set() args after the name)
    /// Features from option() / cmake_dependent_option(): (name, default_on)
    features: Vec<(String, bool)>,
}

#[derive(Clone, Copy, PartialEq)]
enum LibKind {
    Static,
    Shared,
    Interface,
}

impl Extracted {
    fn new() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            bins: Vec::new(),
            libs: Vec::new(),
            deps: Vec::new(),
            find_packages: Vec::new(),
            pkg_modules: Vec::new(),
            fetched_deps: Vec::new(),
            platform_deps: HashMap::new(),
            os_features: HashMap::new(),
            c_std: None,
            cxx_std: None,
            defines: Vec::new(),
            includes: Vec::new(),
            subdirs: Vec::new(),
            vars: HashMap::new(),
            features: Vec::new(),
        }
    }

    fn add_platform_dep(&mut self, dep: String, os: &str) {
        let vec = self.platform_deps.entry(os.to_string()).or_default();
        if !vec.contains(&dep) {
            vec.push(dep);
        }
    }

    fn add_os_feature(&mut self, feat: String, os: &str) {
        let vec = self.os_features.entry(os.to_string()).or_default();
        if !vec.contains(&feat) {
            vec.push(feat);
        }
    }
}

// ── Extraction pass ───────────────────────────────────────────────────────────

/// Walk the AST, routing platform-conditional blocks to per-OS dep buckets.
fn extract(file: &CMakeFile, warnings: &mut Vec<String>) -> Extracted {
    let mut ex = Extracted::new();
    walk_nodes(&file.nodes, &mut ex, None, warnings);
    ex
}

/// Walk `nodes`, routing dep-adding commands through `scope`.
///
/// `scope` is `Some("windows")` / `Some("macos")` / `Some("unix")` etc. when
/// we are inside a platform-specific `if()` branch; `None` for unconditional code.
fn walk_nodes(nodes: &[Node], ex: &mut Extracted, scope: Option<&str>, warnings: &mut Vec<String>) {
    for node in nodes {
        match node {
            Node::Command(cmd) => handle_command(cmd, ex, scope, warnings),
            Node::If(b) => {
                // Use cmake-lossless eval::platform_condition to identify platform blocks.
                let then_scope = cmake_lossless::eval::platform_condition(&b.condition);
                if let Some(os) = then_scope {
                    // Then-branch belongs to this OS; elseif branches may each have
                    // their own platform; else-branch falls through to parent scope.
                    walk_nodes(&b.then_nodes, ex, Some(os), warnings);
                    for (ei_cond, ei_body) in &b.elseif_branches {
                        let ei_scope = cmake_lossless::eval::platform_condition(ei_cond);
                        walk_nodes(ei_body, ex, ei_scope, warnings);
                    }
                    if let Some(else_nodes) = &b.else_nodes {
                        walk_nodes(else_nodes, ex, scope, warnings);
                    }
                } else {
                    // Unknown condition — walk all branches with current scope unchanged.
                    walk_nodes(&b.then_nodes, ex, scope, warnings);
                    for (_, ei_body) in &b.elseif_branches {
                        walk_nodes(ei_body, ex, scope, warnings);
                    }
                    if let Some(else_nodes) = &b.else_nodes {
                        walk_nodes(else_nodes, ex, scope, warnings);
                    }
                }
            }
            Node::Foreach(b) => {
                warnings.push(format!(
                    "line {}: foreach() loop — body skipped (cannot be evaluated statically)",
                    b.line
                ));
            }
            Node::While(b) => {
                warnings.push(format!("line {}: while() loop — body skipped", b.line));
            }
            // Function and macro definitions: skip the body (they define callable
            // templates, not direct build targets — calls appear elsewhere)
            Node::Function(_) | Node::Macro(_) | Node::Comment(_) => {}
            Node::Block(b) => walk_nodes(&b.body, ex, scope, warnings),
        }
    }
}

fn handle_command(
    cmd: &CommandInvocation,
    ex: &mut Extracted,
    scope: Option<&str>,
    warnings: &mut Vec<String>,
) {
    let args = cmd.arg_values();
    match cmd.name.as_str() {
        "project" => handle_project(&args, ex),
        "set" => handle_set(&args, ex),
        "add_executable" => handle_add_executable(&args, ex, warnings),
        "add_library" => handle_add_library(&args, ex, warnings),
        "target_link_libraries" | "link_libraries" => handle_link_libraries(&args, ex, scope),
        "find_package" => handle_find_package(&args, ex, scope),
        "pkg_check_modules" | "pkg_search_module" => handle_pkg_check_modules(&args, ex, scope),
        "include_directories" => {
            if scope.is_none() {
                handle_include_dirs(&args, ex, false);
            }
        }
        "target_include_directories" => {
            if scope.is_none() {
                handle_include_dirs(&args, ex, true);
            }
        }
        "add_definitions" => {
            if scope.is_none() {
                handle_add_definitions(&args, ex, false);
            }
        }
        "target_compile_definitions" => {
            if scope.is_none() {
                handle_add_definitions(&args, ex, true);
            }
        }
        "add_subdirectory" => {
            if let Some(first) = args.first() {
                let sub = expand_var(first, &ex.vars);
                if !sub.is_empty() && !sub.contains('$') && !ex.subdirs.contains(&sub) {
                    ex.subdirs.push(sub);
                }
            }
        }
        "add_compile_options" | "target_compile_options" => {
            handle_compile_options(&args, ex, scope)
        }
        "option" => handle_option(&args, ex),
        "cmake_dependent_option" => handle_cmake_dependent_option(&args, ex),
        "fetchcontent_declare" => handle_fetchcontent_declare(&args, ex, warnings),
        "externalproject_add" => handle_externalproject_add(&args, ex, warnings),
        "cpmaddpackage" => handle_cpm_add_package(&args, ex, warnings),
        _ => {}
    }
}

// ── Command handlers ──────────────────────────────────────────────────────────

fn handle_project(args: &[&str], ex: &mut Extracted) {
    if args.is_empty() {
        return;
    }
    ex.name = args[0].to_string();
    let mut i = 1;
    while i < args.len() {
        if args[i].eq_ignore_ascii_case("VERSION") && i + 1 < args.len() {
            ex.version = args[i + 1].to_string();
            i += 2;
        } else {
            i += 1;
        }
    }
}

fn handle_set(args: &[&str], ex: &mut Extracted) {
    if args.is_empty() {
        return;
    }
    let var = args[0];
    // Values are args[1..], each already a parsed CMake argument value.
    // Store them as a list so multi-value variables are preserved correctly.
    let vals: Vec<String> = args[1..]
        .iter()
        .flat_map(|v| {
            // Expand any variable references in the values
            let expanded = expand_var(v, &ex.vars);
            // A value that was itself a list variable expands to space-separated words
            if expanded.contains(' ') {
                expanded
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            } else {
                vec![expanded]
            }
        })
        .filter(|v| !v.is_empty())
        .collect();

    ex.vars.insert(var.to_string(), vals.clone());

    // Handle special CMake variables that control the build
    match var {
        "CMAKE_CXX_STANDARD" => {
            if let Some(v) = vals.first() {
                ex.cxx_std = map_cxx_std(v);
            }
        }
        "CMAKE_C_STANDARD" => {
            if let Some(v) = vals.first() {
                ex.c_std = map_c_std(v);
            }
        }
        _ => {}
    }
}

fn handle_add_executable(args: &[&str], ex: &mut Extracted, warnings: &mut Vec<String>) {
    if args.is_empty() {
        return;
    }
    const SKIP: &[&str] = &[
        "IMPORTED",
        "ALIAS",
        "WIN32",
        "MACOSX_BUNDLE",
        "EXCLUDE_FROM_ALL",
    ];
    let name = expand_var(args[0], &ex.vars);
    if name.contains('$') {
        return;
    } // unresolvable variable
    if args.len() > 1 && args[1].eq_ignore_ascii_case("ALIAS") {
        return;
    }

    let srcs: Vec<String> = args[1..]
        .iter()
        .filter(|a| !SKIP.contains(&a.to_ascii_uppercase().as_str()))
        .flat_map(|s| expand_var_to_list(s, &ex.vars))
        .filter(|s| is_source_file(s))
        .collect();

    if srcs.is_empty() {
        warnings.push(format!(
            "add_executable({name}) has no recognisable source files — add them manually"
        ));
    }
    if !ex.bins.iter().any(|(n, _)| n == &name) {
        ex.bins.push((name, srcs));
    }
}

fn handle_add_library(args: &[&str], ex: &mut Extracted, warnings: &mut Vec<String>) {
    if args.is_empty() {
        return;
    }
    const SKIP: &[&str] = &["IMPORTED", "ALIAS", "OBJECT", "EXCLUDE_FROM_ALL", "GLOBAL"];
    let name = expand_var(args[0], &ex.vars);
    if name.contains('$') {
        return;
    }

    let mut kind = LibKind::Static;
    let mut src_start = 1;
    if args.len() > 1 {
        match args[1].to_uppercase().as_str() {
            "SHARED" | "MODULE" => {
                kind = LibKind::Shared;
                src_start = 2;
            }
            "STATIC" => {
                src_start = 2;
            }
            "INTERFACE" => {
                warnings.push(format!("add_library({name} INTERFACE …) — header-only"));
                if !ex.libs.iter().any(|(n, _, _)| n == &name) {
                    ex.libs.push((name, LibKind::Interface, vec![]));
                }
                return;
            }
            "ALIAS" => return,
            _ => {}
        }
    }

    let srcs: Vec<String> = args[src_start..]
        .iter()
        .filter(|a| !SKIP.contains(&a.to_ascii_uppercase().as_str()))
        .flat_map(|s| expand_var_to_list(s, &ex.vars))
        .filter(|s| is_source_file(s))
        .collect();

    if srcs.is_empty() {
        warnings.push(format!(
            "add_library({name}) has no recognisable source files — check for generated sources"
        ));
    }
    if !ex.libs.iter().any(|(n, _, _)| n == &name) {
        ex.libs.push((name, kind, srcs));
    }
}

fn handle_link_libraries(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    const VIS: &[&str] = &[
        "PUBLIC",
        "PRIVATE",
        "INTERFACE",
        "GENERAL",
        "OPTIMIZED",
        "DEBUG",
    ];
    // Skip first arg if it looks like a target name (not a visibility keyword or -l flag)
    let start = if !args.is_empty()
        && !VIS.contains(&args[0].to_ascii_uppercase().as_str())
        && !args[0].starts_with('-')
    {
        1
    } else {
        0
    };
    for arg in &args[start..] {
        if VIS.contains(&arg.to_ascii_uppercase().as_str()) {
            continue;
        }
        if let Some(lib) = extract_link_dep(arg) {
            if DRIVER_LINKED.contains(&lib.as_str()) {
                continue; // linked automatically by the compiler driver
            }
            // OS system libraries (pthread, m, ws2_32, …) → `[os.<os>] features`,
            // by their natural OS regardless of the surrounding if() scope.
            if let Some(feat_os) = system_lib_os(&lib) {
                ex.add_os_feature(lib, feat_os);
                continue;
            }
            match scope {
                Some(os) => ex.add_platform_dep(lib, os),
                None => {
                    if !ex.deps.contains(&lib) {
                        ex.deps.push(lib);
                    }
                }
            }
        }
    }
}

fn handle_find_package(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    if args.is_empty() {
        return;
    }
    const SKIP: &[&str] = &[
        "REQUIRED",
        "QUIET",
        "OPTIONAL_COMPONENTS",
        "COMPONENTS",
        "CONFIG",
        "MODULE",
        "NO_MODULE",
    ];
    let pkg = args[0];
    if SKIP.contains(&pkg.to_ascii_uppercase().as_str()) {
        return;
    }
    // `find_package(Threads)` + `Threads::Threads` is the CMake idiom for pthread;
    // route it to the unix pthread feature (the imported target has `::`, so it
    // never reaches the link-library classifier).
    if pkg.eq_ignore_ascii_case("Threads") {
        ex.add_os_feature("pthread".to_string(), "unix");
        return;
    }
    for m in map_find_package(pkg) {
        match scope {
            Some(os) => ex.add_platform_dep(m, os),
            None => {
                if !ex.find_packages.contains(&m) {
                    ex.find_packages.push(m);
                }
            }
        }
    }
}

fn handle_pkg_check_modules(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    if args.len() < 2 {
        return;
    }
    const SKIP: &[&str] = &["REQUIRED", "QUIET", "IMPORTED_TARGET", "GLOBAL"];
    let mut i = 1;
    while i < args.len() {
        let a = args[i];
        if SKIP.contains(&a.to_ascii_uppercase().as_str()) {
            i += 1;
            continue;
        }
        if matches!(a, ">=" | "<=" | ">" | "<" | "=" | "!=") {
            i += 2;
            continue;
        }
        let pkg = a.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
        if !pkg.is_empty() {
            let pkg = pkg.to_string();
            match scope {
                Some(os) => ex.add_platform_dep(pkg, os),
                None => {
                    if !ex.pkg_modules.contains(&pkg) {
                        ex.pkg_modules.push(pkg);
                    }
                }
            }
        }
        i += 1;
    }
}

fn handle_include_dirs(args: &[&str], ex: &mut Extracted, has_target: bool) {
    const SKIP: &[&str] = &[
        "PUBLIC",
        "PRIVATE",
        "INTERFACE",
        "SYSTEM",
        "BEFORE",
        "AFTER",
    ];
    let start = if has_target { 1 } else { 0 };
    for arg in &args[start..] {
        if SKIP.contains(&arg.to_ascii_uppercase().as_str()) {
            continue;
        }
        let expanded = expand_var(arg, &ex.vars);
        if SKIP.contains(&expanded.to_ascii_uppercase().as_str()) {
            continue;
        }
        if expanded.starts_with("$<") || expanded.starts_with("$") {
            continue;
        }
        if expanded.starts_with("/usr") || expanded.starts_with("/opt") {
            continue;
        }
        if !expanded.is_empty() && !ex.includes.contains(&expanded) {
            ex.includes.push(expanded);
        }
    }
}

fn handle_add_definitions(args: &[&str], ex: &mut Extracted, has_target: bool) {
    const SKIP: &[&str] = &["PUBLIC", "PRIVATE", "INTERFACE"];
    let start = if has_target { 1 } else { 0 };
    for arg in &args[start..] {
        if SKIP.contains(&arg.to_ascii_uppercase().as_str()) {
            continue;
        }
        let def = if let Some(rest) = arg.strip_prefix("-D") {
            rest.to_string()
        } else if arg.starts_with('$') {
            continue;
        } else {
            arg.to_string()
        };
        if !def.is_empty() && !ex.defines.contains(&def) {
            ex.defines.push(def);
        }
    }
}

/// Handle `add_compile_options(flags…)` and `target_compile_options(tgt … flags…)`.
///
/// Extracts `-std=` flags into the language standard and `-D` flags into defines.
/// Other compiler flags (-Wall, -O2, etc.) are silently ignored — freight manages
/// optimisation and warning levels through its own build profiles.
fn handle_compile_options(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    const VIS: &[&str] = &["PUBLIC", "PRIVATE", "INTERFACE", "BEFORE"];
    // For target_compile_options the first arg is the target name — skip it.
    let skip_first = args
        .first()
        .map(|a| {
            !a.starts_with('-')
                && !VIS.contains(&a.to_ascii_uppercase().as_str())
                && !a.starts_with("$<")
        })
        .unwrap_or(false);
    let start = if skip_first { 1 } else { 0 };

    for arg in &args[start..] {
        if VIS.contains(&arg.to_ascii_uppercase().as_str()) {
            continue;
        }
        let expanded = expand_var(arg, &ex.vars);
        if expanded.starts_with("$<") || expanded.starts_with('$') {
            continue; // generator expressions / unexpandable variables
        }
        if let Some(rest) = expanded.strip_prefix("-std=") {
            let std = rest.replace("gnu++", "c++").replace("gnu", "c");
            if std.contains("++") {
                if ex.cxx_std.is_none() {
                    ex.cxx_std = Some(std);
                }
            } else if ex.c_std.is_none() {
                ex.c_std = Some(std);
            }
        } else if let Some(def) = expanded.strip_prefix("-D") {
            if !def.is_empty() && scope.is_none() && !ex.defines.contains(&def.to_string()) {
                ex.defines.push(def.to_string());
            }
        }
    }
}

/// Handle `option(NAME "description" [ON|OFF])`.
fn handle_option(args: &[&str], ex: &mut Extracted) {
    if args.is_empty() {
        return;
    }
    let name = sanitize_name(&expand_var(args[0], &ex.vars));
    if name.is_empty() || name.contains('$') {
        return;
    }
    // Skip standard CMake options that don't map to freight features.
    const SKIP: &[&str] = &[
        "cmake_build_type",
        "cmake_install_prefix",
        "build_testing",
        "build_shared_libs",
        "cmake_verbose_makefile",
    ];
    if SKIP.contains(&name.to_ascii_lowercase().as_str()) {
        return;
    }
    let default_on = args
        .get(2)
        .map(|v| v.eq_ignore_ascii_case("ON"))
        .unwrap_or(false);
    if !ex.features.iter().any(|(n, _)| n == &name) {
        ex.features.push((name, default_on));
    }
}

/// Handle `cmake_dependent_option(NAME "description" ON|OFF "conditions" [FORCE])`.
fn handle_cmake_dependent_option(args: &[&str], ex: &mut Extracted) {
    if args.is_empty() {
        return;
    }
    let name = sanitize_name(&expand_var(args[0], &ex.vars));
    if name.is_empty() || name.contains('$') {
        return;
    }
    let default_on = args
        .get(2)
        .map(|v| v.eq_ignore_ascii_case("ON"))
        .unwrap_or(false);
    if !ex.features.iter().any(|(n, _)| n == &name) {
        ex.features.push((name, default_on));
    }
}

// ── FetchContent / ExternalProject / CPM handlers ────────────────────────────

fn handle_fetchcontent_declare(args: &[&str], ex: &mut Extracted, warnings: &mut Vec<String>) {
    if args.is_empty() {
        return;
    }
    let name = sanitize_name(&expand_var(args[0], &ex.vars));
    if name.is_empty() || name.contains('$') {
        return;
    }
    if ex.fetched_deps.iter().any(|d| d.name == name) {
        return; // already declared
    }
    if let Some(dep) = parse_fetch_kv(&name, &args[1..], warnings) {
        ex.fetched_deps.push(dep);
    }
}

fn handle_externalproject_add(args: &[&str], ex: &mut Extracted, warnings: &mut Vec<String>) {
    if args.is_empty() {
        return;
    }
    let name = sanitize_name(&expand_var(args[0], &ex.vars));
    if name.is_empty() || name.contains('$') {
        return;
    }
    if ex.fetched_deps.iter().any(|d| d.name == name) {
        return;
    }
    let kv = keyword_value_pairs(&args[1..]);
    if kv.contains_key("BUILD_COMMAND")
        || kv.contains_key("INSTALL_COMMAND")
        || kv.contains_key("PATCH_COMMAND")
    {
        warnings.push(format!(
            "ExternalProject_Add({name}): custom build/install/patch commands not migrated — review manually"
        ));
    }
    if let Some(dep) = parse_fetch_kv(&name, &args[1..], warnings) {
        ex.fetched_deps.push(dep);
    }
}

fn handle_cpm_add_package(args: &[&str], ex: &mut Extracted, warnings: &mut Vec<String>) {
    if args.is_empty() {
        return;
    }
    // Compact single-string form: "gh:user/repo#tag" / "gl:..." / "bb:..."
    if args.len() == 1 {
        match parse_cpm_compact(args[0]) {
            Some(dep) => {
                if !ex.fetched_deps.iter().any(|d| d.name == dep.name) {
                    ex.fetched_deps.push(dep);
                }
            }
            None => warnings.push(format!(
                "CPMAddPackage: unrecognised compact form '{}' — add dep manually",
                args[0]
            )),
        }
        return;
    }
    // Keyword form
    let kv = keyword_value_pairs(args);
    let name = match kv.get("NAME").map(|s| sanitize_name(s)) {
        Some(n) if !n.is_empty() && !n.contains('$') => n,
        _ => return,
    };
    if ex.fetched_deps.iter().any(|d| d.name == name) {
        return;
    }
    // Resolve git URL from various CPM source keys
    let git = kv
        .get("GITHUB_REPOSITORY")
        .map(|r| format!("https://github.com/{r}.git"))
        .or_else(|| {
            kv.get("GITLAB_REPOSITORY")
                .map(|r| format!("https://gitlab.com/{r}.git"))
        })
        .or_else(|| {
            kv.get("BITBUCKET_REPOSITORY")
                .map(|r| format!("https://bitbucket.org/{r}.git"))
        })
        .or_else(|| kv.get("GIT_REPOSITORY").cloned());

    let url = kv.get("URL").cloned();
    let sha256 = kv.get("URL_HASH").and_then(|h| {
        h.strip_prefix("SHA256=")
            .or_else(|| h.strip_prefix("sha256="))
            .map(str::to_string)
    });

    let git_tag_str = kv.get("GIT_TAG").map(|s| s.as_str());
    let (mut tag, branch, rev) = match git_tag_str {
        Some(t) => classify_git_ref(t),
        None => (None, None, None),
    };
    // VERSION without GIT_TAG → synthesise a "v{version}" tag
    if tag.is_none() && branch.is_none() && rev.is_none() {
        if let Some(v) = kv.get("VERSION") {
            if git.is_some() {
                tag = Some(format!("v{v}"));
            }
        }
    }

    if git.is_none() && url.is_none() {
        warnings.push(format!(
            "CPMAddPackage({name}): no repository or URL found — add dep manually"
        ));
        return;
    }
    ex.fetched_deps.push(FetchedDep {
        name,
        tag,
        branch,
        rev,
        url: git.or(url),
        sha256,
    });
}

// ── FetchContent / CPM helpers ────────────────────────────────────────────────

/// Parse GIT_REPOSITORY / URL / URL_HASH / GIT_TAG keyword-value pairs into a
/// `FetchedDep`.  Returns `None` and pushes a warning if no source is found.
fn parse_fetch_kv(name: &str, tail: &[&str], warnings: &mut Vec<String>) -> Option<FetchedDep> {
    let kv = keyword_value_pairs(tail);
    let git = kv.get("GIT_REPOSITORY").cloned();
    let url = kv.get("URL").cloned();
    let sha256 = kv
        .get("URL_HASH")
        .and_then(|h| {
            h.strip_prefix("SHA256=")
                .or_else(|| h.strip_prefix("sha256="))
                .map(str::to_string)
        })
        .or_else(|| kv.get("SHA256").cloned());

    let git_tag_str = kv.get("GIT_TAG").map(|s| s.as_str());
    let (tag, branch, rev) = match git_tag_str {
        Some(t) => classify_git_ref(t),
        None => (None, None, None),
    };

    if git.is_none() && url.is_none() {
        warnings.push(format!(
            "FetchContent_Declare / ExternalProject_Add({name}): no GIT_REPOSITORY or URL — add dep manually"
        ));
        return None;
    }
    Some(FetchedDep {
        name: name.to_string(),
        tag,
        branch,
        rev,
        url: git.or(url),
        sha256,
    })
}

/// Parse `CPMAddPackage("gh:user/repo#tag")` and its `gl:` / `bb:` variants.
fn parse_cpm_compact(s: &str) -> Option<FetchedDep> {
    let (prefix, host) = if s.starts_with("gh:") {
        ("gh:", "https://github.com/")
    } else if s.starts_with("gl:") {
        ("gl:", "https://gitlab.com/")
    } else if s.starts_with("bb:") {
        ("bb:", "https://bitbucket.org/")
    } else {
        return None;
    };
    let rest = s.strip_prefix(prefix)?;
    let (repo, ref_str) = rest.split_once('#').unwrap_or((rest, ""));
    let raw_name = repo.rsplit('/').next()?;
    let name = sanitize_name(raw_name);
    if name.is_empty() {
        return None;
    }
    let url = Some(format!("{host}{repo}.git"));
    let (tag, branch, rev) = if !ref_str.is_empty() {
        classify_git_ref(ref_str)
    } else {
        (None, None, None)
    };
    Some(FetchedDep {
        name,
        tag,
        branch,
        rev,
        url,
        sha256: None,
    })
}

/// Classify a `GIT_TAG` value as a tag, branch, or pinned revision.
///
/// 40-char hex → `rev`; version-like string → `tag`; otherwise → `branch`.
fn classify_git_ref(s: &str) -> (Option<String>, Option<String>, Option<String>) {
    if s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return (None, None, Some(s.to_string()));
    }
    let looks_like_version = s.starts_with('v') && s[1..].starts_with(|c: char| c.is_ascii_digit())
        || s.starts_with(|c: char| c.is_ascii_digit());
    if looks_like_version {
        return (Some(s.to_string()), None, None);
    }
    (None, Some(s.to_string()), None)
}

/// Extract keyword→value pairs from a CMake argument list.
/// Keywords are all-uppercase tokens with only `[A-Z0-9_]` characters.
fn keyword_value_pairs(args: &[&str]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut i = 0;
    while i + 1 < args.len() {
        let upper = args[i].to_ascii_uppercase();
        if !upper.is_empty()
            && upper
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            map.entry(upper).or_insert_with(|| args[i + 1].to_string());
            i += 2;
        } else {
            i += 1;
        }
    }
    map
}

/// Build a toml_edit `InlineTable` for a `FetchedDep` entry.
fn fetched_dep_inline(dep: &FetchedDep) -> InlineTable {
    let mut tbl = InlineTable::new();
    if let Some(url) = &dep.url {
        tbl.insert("url", toml_edit::Value::from(url.as_str()));
    }
    if let Some(tag) = &dep.tag {
        tbl.insert("tag", toml_edit::Value::from(tag.as_str()));
    } else if let Some(branch) = &dep.branch {
        tbl.insert("branch", toml_edit::Value::from(branch.as_str()));
    } else if let Some(rev) = &dep.rev {
        tbl.insert("rev", toml_edit::Value::from(rev.as_str()));
    }
    if let Some(url) = &dep.url {
        tbl.insert("url", toml_edit::Value::from(url.as_str()));
    }
    if let Some(sha256) = &dep.sha256 {
        tbl.insert("sha256", toml_edit::Value::from(sha256.as_str()));
    }
    tbl
}

// ── Variable expansion ────────────────────────────────────────────────────────

/// Expand `${VAR}` / `$ENV{VAR}` / `$CACHE{VAR}` references using the known
/// variable map.  Unknown variables are left as-is.
fn expand_var(s: &str, vars: &HashMap<String, Vec<String>>) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            // Find the closing '}'
            if let Some(end) = s[i + 2..].find('}') {
                let var_name = &s[i + 2..i + 2 + end];
                if let Some(vals) = vars.get(var_name) {
                    out.push_str(&vals.join(" "));
                } else {
                    out.push_str(&s[i..i + 2 + end + 1]); // preserve unknown ${VAR}
                }
                i += 2 + end + 1;
                continue;
            }
        } else if bytes[i] == b'$' && s[i..].starts_with("$ENV{") {
            if let Some(end) = s[i + 5..].find('}') {
                // Env vars: preserve verbatim (can't evaluate)
                out.push_str(&s[i..i + 5 + end + 1]);
                i += 5 + end + 1;
                continue;
            }
        } else if bytes[i] == b'$' && s[i..].starts_with("$CACHE{") {
            if let Some(end) = s[i + 7..].find('}') {
                out.push_str(&s[i..i + 7 + end + 1]);
                i += 7 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Expand a variable reference and split the result into multiple paths if
/// the variable held a list (space-separated).
fn expand_var_to_list(s: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    // If s is a single ${VAR} reference, return the list directly
    let trimmed = s.trim();
    if trimmed.starts_with("${") && trimmed.ends_with('}') {
        let var_name = &trimmed[2..trimmed.len() - 1];
        if let Some(vals) = vars.get(var_name) {
            return vals.clone();
        }
    }
    let expanded = expand_var(s, vars);
    if expanded.contains(' ') {
        expanded.split_whitespace().map(str::to_string).collect()
    } else {
        vec![expanded]
    }
}

// ── Classifier helpers ────────────────────────────────────────────────────────

fn is_source_file(s: &str) -> bool {
    if s.starts_with('$') || s.starts_with('@') {
        return false;
    }
    const KEYWORDS: &[&str] = &[
        "PUBLIC",
        "PRIVATE",
        "INTERFACE",
        "REQUIRED",
        "OPTIONAL",
        "BEFORE",
        "AFTER",
        "SYSTEM",
        "STATIC",
        "SHARED",
        "MODULE",
        "IMPORTED",
        "ALIAS",
        "GLOBAL",
    ];
    if KEYWORDS.contains(&s.to_ascii_uppercase().as_str()) {
        return false;
    }
    const SOURCE_EXTS: &[&str] = &[
        ".c", ".cc", ".cpp", ".cxx", ".c++", ".C", ".f", ".f90", ".f95", ".f03", ".f08", ".F",
        ".F90", ".cu", ".hip", ".cl", ".ispc", ".s", ".S", ".asm", ".d", ".adb", ".ads", ".m",
        ".mm",
    ];
    SOURCE_EXTS.iter().any(|ext| s.ends_with(ext))
}

fn extract_link_dep(s: &str) -> Option<String> {
    // Skip CMake imported target names (Foo::Bar), variables, path-based libs
    if s.contains("::") || s.starts_with('$') || s.starts_with("-L") {
        return None;
    }
    if s.contains('/') || s.contains('.') {
        return None;
    }
    let lib = if let Some(rest) = s.strip_prefix("-l") {
        rest
    } else {
        s
    };
    if lib.is_empty() || lib.starts_with('-') {
        return None;
    }
    Some(lib.to_string())
}

fn map_find_package(pkg: &str) -> Vec<String> {
    match pkg {
        "Threads" => vec![],
        "OpenSSL" => vec!["openssl".to_string()],
        "ZLIB" => vec!["zlib".to_string()],
        "CURL" => vec!["libcurl".to_string()],
        "Boost" => vec!["boost".to_string()],
        "fmt" | "FMT" => vec!["fmt".to_string()],
        "spdlog" => vec!["spdlog".to_string()],
        "GTest" | "GoogleTest" => vec!["gtest".to_string()],
        "SQLite3" | "SQLite" => vec!["sqlite3".to_string()],
        "LibXml2" => vec!["libxml-2.0".to_string()],
        "PNG" => vec!["libpng".to_string()],
        "JPEG" => vec!["libjpeg".to_string()],
        "SDL2" => vec!["sdl2".to_string()],
        "OpenGL" => vec!["gl".to_string()],
        "Protobuf" => vec!["protobuf".to_string()],
        "MPI" => vec!["mpi".to_string()],
        "HDF5" => vec!["hdf5".to_string()],
        "Python3" | "Python" | "LLVM" => vec![],
        _ => {
            let lower = pkg.to_lowercase();
            if lower.len() > 2 {
                vec![lower]
            } else {
                vec![]
            }
        }
    }
}

fn map_cxx_std(val: &str) -> Option<String> {
    match val.trim() {
        "98" | "03" => Some("c++98".to_string()),
        "11" => Some("c++11".to_string()),
        "14" => Some("c++14".to_string()),
        "17" => Some("c++17".to_string()),
        "20" => Some("c++20".to_string()),
        "23" => Some("c++23".to_string()),
        _ => None,
    }
}

fn map_c_std(val: &str) -> Option<String> {
    match val.trim() {
        "90" | "89" => Some("c99".to_string()),
        "99" => Some("c99".to_string()),
        "11" => Some("c11".to_string()),
        "17" => Some("c17".to_string()),
        "23" => Some("c23".to_string()),
        _ => None,
    }
}

use super::{system_lib_os, DRIVER_LINKED};

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .to_lowercase()
}

fn ensure_three_part(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    match parts.len() {
        1 => format!("{v}.0.0"),
        2 => format!("{v}.0"),
        _ => v.to_string(),
    }
}

// ── TOML emitter ──────────────────────────────────────────────────────────────

fn emit_toml(name: &str, version: &str, ex: &Extracted, warnings: &[String]) -> String {
    let mut doc = DocumentMut::new();

    let mut header =
        String::from("# Generated by freight migrate cmake — review before committing.\n");
    for w in warnings {
        header.push_str(&format!("# warning: {w}\n"));
    }

    let mut pkg = Table::new();
    pkg.insert("name", value(name));
    pkg.insert("version", value(version));
    doc.insert("package", Item::Table(pkg));

    // [features] — from option() / cmake_dependent_option()
    if !ex.features.is_empty() {
        let mut feat_tbl = Table::new();
        let default_names: Vec<&str> = ex
            .features
            .iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| n.as_str())
            .collect();
        if !default_names.is_empty() {
            let mut arr = Array::new();
            for f in &default_names {
                arr.push(*f);
            }
            feat_tbl.insert("default", Item::Value(arr.into()));
        }
        for (name, _) in &ex.features {
            if name != "default" {
                feat_tbl.insert(name.as_str(), Item::Value(Array::new().into()));
            }
        }
        doc.insert("features", Item::Table(feat_tbl));
    }

    if !ex.defines.is_empty() || !ex.includes.is_empty() {
        let mut compiler = Table::new();
        if !ex.defines.is_empty() {
            let mut arr = Array::new();
            for d in &ex.defines {
                arr.push(d.as_str());
            }
            compiler.insert("defines", Item::Value(arr.into()));
        }
        if !ex.includes.is_empty() {
            let mut arr = Array::new();
            for inc in &ex.includes {
                arr.push(inc.as_str());
            }
            compiler.insert("includes", Item::Value(arr.into()));
        }
        doc.insert("compiler", Item::Table(compiler));
    }

    // Fetched deps (FetchContent / ExternalProject / CPM) take priority over
    // system/find_package entries for the same name.
    let fetched_names: HashSet<&str> = ex.fetched_deps.iter().map(|d| d.name.as_str()).collect();
    let system_deps: Vec<&str> = {
        let mut seen = HashSet::new();
        let mut deps = Vec::new();
        for d in ex
            .deps
            .iter()
            .chain(ex.find_packages.iter())
            .chain(ex.pkg_modules.iter())
        {
            if !fetched_names.contains(d.as_str()) && seen.insert(d.as_str()) {
                deps.push(d.as_str());
            }
        }
        deps
    };
    if !ex.fetched_deps.is_empty() || !system_deps.is_empty() {
        let mut dep_tbl = Table::new();
        for dep in &ex.fetched_deps {
            dep_tbl.insert(
                &dep.name,
                Item::Value(toml_edit::Value::InlineTable(fetched_dep_inline(dep))),
            );
        }
        for d in &system_deps {
            dep_tbl.insert(d, super::system_dep_item(d));
        }
        doc.insert("dependencies", Item::Table(dep_tbl));
    }

    for (tgt_name, srcs) in &ex.bins {
        let mut bin_tbl = Table::new();
        bin_tbl.insert("name", value(tgt_name.as_str()));
        if !srcs.is_empty() {
            // Always emit `src` (entry point only). freight's source walker discovers
            // the remaining files in src/ automatically; emitting a `srcs` array
            // here would require BinTarget manifest support that doesn't yet exist.
            bin_tbl.insert("src", value(srcs[0].as_str()));
        }
        let entry = doc
            .entry("bin")
            .or_insert(Item::ArrayOfTables(Default::default()));
        if let Item::ArrayOfTables(aot) = entry {
            aot.push(bin_tbl);
        }
    }

    for (tgt_name, kind, srcs) in &ex.libs {
        let mut lib_tbl = Table::new();
        lib_tbl.insert("name", value(tgt_name.as_str()));
        match kind {
            LibKind::Shared => {
                lib_tbl.insert("type", value("shared"));
            }
            LibKind::Interface => {
                lib_tbl.insert("type", value("interface"));
            }
            LibKind::Static => {}
        }
        if !srcs.is_empty() {
            let mut arr = Array::new();
            for s in srcs {
                arr.push(s.as_str());
            }
            lib_tbl.insert("srcs", Item::Value(arr.into()));
        }
        let entry = doc
            .entry("lib")
            .or_insert(Item::ArrayOfTables(Default::default()));
        if let Item::ArrayOfTables(aot) = entry {
            aot.push(lib_tbl);
        }
    }

    // Language standards and platform deps appended as raw TOML to avoid empty headers.
    let mut extra = String::new();
    if let Some(std) = &ex.cxx_std {
        extra.push_str(&format!("\n[language.cpp]\nstd = \"{std}\"\n"));
    }
    if let Some(std) = &ex.c_std {
        extra.push_str(&format!("\n[language.c]\nstd = \"{std}\"\n"));
    }

    // Per-OS sections: system-lib `features` and platform-conditional dependencies
    // (sorted for deterministic output). `[os.<os>]` must precede its
    // `[os.<os>.dependencies]` sub-table.
    let mut os_keys: Vec<&str> = ex
        .os_features
        .keys()
        .chain(ex.platform_deps.keys())
        .map(String::as_str)
        .collect();
    os_keys.sort_unstable();
    os_keys.dedup();
    for os in os_keys {
        let feats = ex.os_features.get(os).filter(|v| !v.is_empty());
        let pdeps = ex.platform_deps.get(os).filter(|v| !v.is_empty());
        if feats.is_none() && pdeps.is_none() {
            continue;
        }
        if let Some(feats) = feats {
            let list = feats
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ");
            extra.push_str(&format!("\n[os.{os}]\nfeatures = [{list}]\n"));
        }
        if let Some(pdeps) = pdeps {
            extra.push_str(&format!("\n[os.{os}.dependencies]\n"));
            for d in pdeps {
                // pkg-config `--modversion` when available, else `*` (a draft
                // placeholder `freight build` flags so the user pins it).
                let v = crate::adaptors::pkg_config_version(d);
                let v = if v.is_empty() { "*" } else { &v };
                extra.push_str(&format!("{d} = \"{v}\"\n"));
            }
        }
    }

    format!("{header}{}{extra}", doc)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cmake_lossless::parse as cmake_parse;

    fn extract_src(src: &str) -> (Extracted, Vec<String>) {
        let file = cmake_parse(src).expect("valid cmake");
        let mut warnings = Vec::new();
        let ex = extract(&file, &mut warnings);
        (ex, warnings)
    }

    // ── Project / version ─────────────────────────────────────────────────────

    #[test]
    fn project_name_and_version() {
        let (ex, _) = extract_src("project(mylib VERSION 1.2.3 LANGUAGES CXX)");
        assert_eq!(ex.name, "mylib");
        assert_eq!(ex.version, "1.2.3");
    }

    #[test]
    fn project_name_only() {
        let (ex, _) = extract_src("project(mylib)");
        assert_eq!(ex.name, "mylib");
        assert!(ex.version.is_empty());
    }

    // ── C/C++ standard detection ──────────────────────────────────────────────

    #[test]
    fn cxx_standard_17() {
        let (ex, _) = extract_src("set(CMAKE_CXX_STANDARD 17)");
        assert_eq!(ex.cxx_std, Some("c++17".to_string()));
    }

    #[test]
    fn cxx_standard_20() {
        let (ex, _) = extract_src("set(CMAKE_CXX_STANDARD 20)");
        assert_eq!(ex.cxx_std, Some("c++20".to_string()));
    }

    #[test]
    fn c_standard_11() {
        let (ex, _) = extract_src("set(CMAKE_C_STANDARD 11)");
        assert_eq!(ex.c_std, Some("c11".to_string()));
    }

    #[test]
    fn c_standard_99() {
        let (ex, _) = extract_src("set(CMAKE_C_STANDARD 99)");
        assert_eq!(ex.c_std, Some("c99".to_string()));
    }

    #[test]
    fn unknown_standard_ignored() {
        let (ex, _) = extract_src("set(CMAKE_CXX_STANDARD 42)");
        assert!(ex.cxx_std.is_none());
    }

    // ── Variable expansion into source lists ──────────────────────────────────

    #[test]
    fn variable_stored_sources() {
        let src = "set(SRCS main.c util.c io.c)\nadd_executable(myapp ${SRCS})";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.bins.len(), 1);
        assert_eq!(ex.bins[0].0, "myapp");
        assert_eq!(ex.bins[0].1, vec!["main.c", "util.c", "io.c"]);
    }

    #[test]
    fn variable_stored_lib_sources() {
        let src = "set(LIB_SRCS a.c b.c c.c)\nadd_library(mylib STATIC ${LIB_SRCS})";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.libs.len(), 1);
        assert_eq!(ex.libs[0].2, vec!["a.c", "b.c", "c.c"]);
    }

    #[test]
    fn set_then_append_pattern() {
        // set(X a b); set(X ${X} c d) — common CMake list append idiom
        let src = "set(SRCS a.c b.c)\nset(SRCS ${SRCS} c.c d.c)\nadd_executable(app ${SRCS})";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.bins[0].1, vec!["a.c", "b.c", "c.c", "d.c"]);
    }

    // ── WIN32 block skipping ──────────────────────────────────────────────────

    #[test]
    fn win32_block_deps_excluded() {
        let src = "\
            add_executable(app main.c)\n\
            if(WIN32)\n\
              target_link_libraries(app ws2_32 crypt32)\n\
            else()\n\
              target_link_libraries(app z)\n\
            endif()";
        let (ex, _) = extract_src(src);
        assert!(
            !ex.deps.contains(&"ws2_32".to_string()),
            "ws2_32 should be excluded"
        );
        assert!(
            !ex.deps.contains(&"crypt32".to_string()),
            "crypt32 should be excluded"
        );
        assert!(
            ex.deps.contains(&"z".to_string()),
            "z should be included from else branch"
        );
    }

    #[test]
    fn msvc_block_excluded() {
        let src = "if(MSVC)\n  add_definitions(-D_CRT_SECURE_NO_WARNINGS)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(!ex.defines.contains(&"_CRT_SECURE_NO_WARNINGS".to_string()));
    }

    #[test]
    fn apple_block_excluded() {
        let src = "if(APPLE)\n  target_link_libraries(app \"-framework CoreFoundation\")\nendif()";
        let (ex, _) = extract_src(src);
        // Framework link flags contain '/' equivalent — just check unconditional deps is clean
        assert!(ex.deps.is_empty());
    }

    #[test]
    fn unix_block_not_excluded() {
        let src = "if(UNIX)\n  target_link_libraries(app z)\nendif()";
        let (ex, _) = extract_src(src);
        // UNIX is a recognised platform — goes to platform_deps["unix"], not dropped
        assert!(
            !ex.deps.contains(&"z".to_string()),
            "z should not be unconditional"
        );
        assert_eq!(
            ex.platform_deps.get("unix").map(Vec::as_slice),
            Some(&["z".to_string()][..])
        );
    }

    // ── Platform-conditional dep mapping ──────────────────────────────────────

    #[test]
    fn win32_syslib_in_os_features() {
        // ws2_32 is an OS system library → `[os.windows] features`, not a dep.
        let src = "if(WIN32)\n  target_link_libraries(app ws2_32)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.deps.is_empty());
        assert!(ex.platform_deps.get("windows").map_or(true, Vec::is_empty));
        assert_eq!(
            ex.os_features.get("windows").map(Vec::as_slice),
            Some(&["ws2_32".to_string()][..])
        );
    }

    #[test]
    fn msvc_syslib_in_os_features() {
        let src = "if(MSVC)\n  target_link_libraries(app dbghelp)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.deps.is_empty());
        assert_eq!(
            ex.os_features.get("windows").map(Vec::as_slice),
            Some(&["dbghelp".to_string()][..])
        );
    }

    #[test]
    fn apple_deps_in_macos_section() {
        let src = "if(APPLE)\n  find_package(OpenSSL REQUIRED)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.find_packages.is_empty());
        assert_eq!(
            ex.platform_deps.get("macos").map(Vec::as_slice),
            Some(&["openssl".to_string()][..])
        );
    }

    #[test]
    fn unix_syslibs_in_os_features() {
        let src = "if(UNIX)\n  target_link_libraries(app dl rt)\nendif()";
        let (ex, _) = extract_src(src);
        // dl and rt are OS system libraries → `[os.unix] features`, not deps.
        assert!(ex.platform_deps.get("unix").map_or(true, Vec::is_empty));
        let unix = ex.os_features.get("unix").cloned().unwrap_or_default();
        assert!(unix.contains(&"dl".to_string()));
        assert!(unix.contains(&"rt".to_string()));
    }

    #[test]
    fn if_else_splits_to_platform_and_unconditional() {
        let src = "\
            if(WIN32)\n\
              target_link_libraries(app ws2_32 crypt32)\n\
            else()\n\
              target_link_libraries(app z)\n\
            endif()";
        let (ex, _) = extract_src(src);
        // Windows-specific system libs → os_features, not the dep bucket.
        let win = ex.os_features.get("windows").cloned().unwrap_or_default();
        assert!(win.contains(&"ws2_32".to_string()));
        assert!(win.contains(&"crypt32".to_string()));
        // Unconditional (else) dep `z` is a real library → regular bucket.
        assert!(ex.deps.contains(&"z".to_string()));
        // Nothing in the wrong bucket
        assert!(!ex.deps.contains(&"ws2_32".to_string()));
        assert!(ex.platform_deps.get("windows").map_or(true, Vec::is_empty));
    }

    #[test]
    fn elseif_chain_splits_to_multiple_platforms() {
        let src = "\
            if(WIN32)\n\
              target_link_libraries(app ws2_32)\n\
            elseif(APPLE)\n\
              find_package(OpenSSL REQUIRED)\n\
            else()\n\
              target_link_libraries(app z)\n\
            endif()";
        let (ex, _) = extract_src(src);
        let win = ex.os_features.get("windows").cloned().unwrap_or_default();
        let mac = ex.platform_deps.get("macos").cloned().unwrap_or_default();
        assert!(
            win.contains(&"ws2_32".to_string()),
            "ws2_32 should be a windows-only feature"
        );
        assert!(
            mac.contains(&"openssl".to_string()),
            "openssl should be a macos-only dep"
        );
        assert!(
            ex.deps.contains(&"z".to_string()),
            "z should be unconditional"
        );
        assert!(!ex.deps.contains(&"ws2_32".to_string()));
        assert!(!ex.deps.contains(&"openssl".to_string()));
    }

    #[test]
    fn emit_toml_includes_platform_sections() {
        let src = "\
            project(myapp)\n\
            add_executable(myapp main.c)\n\
            if(WIN32)\n\
              target_link_libraries(myapp ws2_32)\n\
            else()\n\
              target_link_libraries(myapp z)\n\
            endif()";
        let (ex, w) = extract_src(src);
        let toml = emit_toml("myapp", "0.1.0", &ex, &w);
        assert!(
            toml.contains("[os.windows]"),
            "should have windows section"
        );
        assert!(
            toml.contains("features = [\"ws2_32\"]"),
            "ws2_32 should be a windows feature, got:\n{toml}"
        );
        assert!(
            toml.contains("[dependencies]"),
            "should have main deps section"
        );
        assert!(toml.contains("z ="), "should have z as a dep");
        assert!(
            !toml.contains("[os.linux.dependencies]"),
            "no spurious linux section"
        );
    }

    #[test]
    fn not_win32_branch_is_unix_platform() {
        // NOT WIN32 maps to "unix" via platform_condition — deps go to [os.unix.dependencies]
        let src = "if(NOT WIN32)\n  target_link_libraries(app z)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(
            ex.platform_deps
                .get("unix")
                .map_or(false, |d| d.contains(&"z".to_string())),
            "NOT WIN32 body should be mapped to unix platform deps"
        );
    }

    // ── find_package mapping ──────────────────────────────────────────────────

    #[test]
    fn find_package_openssl_mapped() {
        let (ex, _) = extract_src("find_package(OpenSSL REQUIRED)");
        assert!(ex.find_packages.contains(&"openssl".to_string()));
    }

    #[test]
    fn find_package_threads_excluded() {
        let (ex, _) = extract_src("find_package(Threads REQUIRED)");
        assert!(ex.find_packages.is_empty());
    }

    #[test]
    fn find_package_zlib_mapped() {
        let (ex, _) = extract_src("find_package(ZLIB REQUIRED)");
        assert!(ex.find_packages.contains(&"zlib".to_string()));
    }

    #[test]
    fn find_package_unknown_lowercased() {
        let (ex, _) = extract_src("find_package(SomeLongLib REQUIRED)");
        assert!(ex.find_packages.contains(&"somelonglib".to_string()));
    }

    // ── pkg_check_modules ─────────────────────────────────────────────────────

    #[test]
    fn pkg_check_modules_extracted() {
        let (ex, _) = extract_src("pkg_check_modules(LIBFOO REQUIRED libfoo >= 1.5)");
        assert!(ex.pkg_modules.contains(&"libfoo".to_string()));
    }

    #[test]
    fn pkg_check_modules_no_version() {
        let (ex, _) = extract_src("pkg_check_modules(GTK REQUIRED gtk+-3.0)");
        assert!(ex.pkg_modules.contains(&"gtk+-3.0".to_string()));
    }

    // ── target_link_libraries ─────────────────────────────────────────────────

    #[test]
    fn minus_l_flag_extracted() {
        let (ex, _) = extract_src("target_link_libraries(app -lz -lfoo)");
        assert!(ex.deps.contains(&"z".to_string()));
        assert!(ex.deps.contains(&"foo".to_string()));
    }

    #[test]
    fn auto_linked_libs_excluded() {
        let (ex, _) = extract_src("target_link_libraries(app pthread m dl rt)");
        assert!(
            ex.deps.is_empty(),
            "auto-linked libs should be excluded, got: {:?}",
            ex.deps
        );
    }

    #[test]
    fn cmake_imported_targets_excluded() {
        // Foo::Bar style CMake targets are not direct deps
        let (ex, _) = extract_src("target_link_libraries(app OpenSSL::SSL Boost::filesystem)");
        assert!(ex.deps.is_empty());
    }

    // ── Defines / includes ────────────────────────────────────────────────────

    #[test]
    fn add_definitions_extracted() {
        let (ex, _) = extract_src("add_definitions(-DFOO -DBAR=1)");
        assert!(ex.defines.contains(&"FOO".to_string()));
        assert!(ex.defines.contains(&"BAR=1".to_string()));
    }

    #[test]
    fn target_compile_definitions_extracted() {
        let (ex, _) = extract_src("target_compile_definitions(mylib PRIVATE MY_DEFINE=42)");
        assert!(ex.defines.contains(&"MY_DEFINE=42".to_string()));
    }

    #[test]
    fn include_directories_extracted() {
        let (ex, _) = extract_src("include_directories(include/ third_party/include/)");
        assert!(ex.includes.contains(&"include/".to_string()));
        assert!(ex.includes.contains(&"third_party/include/".to_string()));
    }

    #[test]
    fn system_include_dirs_excluded() {
        let (ex, _) = extract_src("include_directories(/usr/include /opt/local/include)");
        assert!(ex.includes.is_empty());
    }

    #[test]
    fn generator_expr_include_excluded() {
        let (ex, _) = extract_src(
            "target_include_directories(mylib PUBLIC $<BUILD_INTERFACE:${CMAKE_SOURCE_DIR}/include>)"
        );
        assert!(ex.includes.is_empty());
    }

    // ── add_subdirectory detection ────────────────────────────────────────────

    #[test]
    fn subdirectory_collected() {
        let (ex, _) =
            extract_src("add_subdirectory(lib)\nadd_subdirectory(app)\nadd_subdirectory(tests)");
        assert_eq!(ex.subdirs, vec!["lib", "app", "tests"]);
    }

    // ── Function/macro bodies skipped ─────────────────────────────────────────

    #[test]
    fn function_body_not_extracted() {
        let src = "function(setup TARGET)\n  add_library(${TARGET} STATIC stub.c)\nendfunction()\nadd_executable(myapp main.c)";
        let (ex, _) = extract_src(src);
        // The add_library inside the function should NOT appear in ex.libs
        assert!(ex.libs.is_empty(), "function body should be skipped");
        assert_eq!(ex.bins.len(), 1);
        assert_eq!(ex.bins[0].0, "myapp");
    }

    #[test]
    fn macro_body_not_extracted() {
        let src = "macro(add_test_exe NAME)\n  add_executable(${NAME} tests/${NAME}.cpp)\nendmacro()\nadd_library(mylib STATIC src/lib.cpp)";
        let (ex, _) = extract_src(src);
        assert!(ex.bins.is_empty());
        assert_eq!(ex.libs.len(), 1);
    }

    // ── Library kinds ─────────────────────────────────────────────────────────

    #[test]
    fn shared_library_detected() {
        let (ex, _) = extract_src("add_library(mylib SHARED src/lib.c)");
        assert!(matches!(ex.libs[0].1, LibKind::Shared));
    }

    #[test]
    fn static_library_detected() {
        let (ex, _) = extract_src("add_library(mylib STATIC src/lib.c)");
        assert!(matches!(ex.libs[0].1, LibKind::Static));
    }

    #[test]
    fn interface_library_detected() {
        let (ex, _) = extract_src("add_library(mylib INTERFACE)");
        assert!(matches!(ex.libs[0].1, LibKind::Interface));
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    #[test]
    fn sanitize_name_replaces_dots_and_spaces() {
        assert_eq!(sanitize_name("My Project 2.0"), "my-project-2-0");
    }

    #[test]
    fn ensure_three_part_pads_short_versions() {
        assert_eq!(ensure_three_part("1"), "1.0.0");
        assert_eq!(ensure_three_part("1.2"), "1.2.0");
        assert_eq!(ensure_three_part("1.2.3"), "1.2.3");
        assert_eq!(ensure_three_part("1.2.3.4"), "1.2.3.4");
    }

    #[test]
    fn version_with_unresolved_var_becomes_default() {
        // extract_src doesn't go through the full emit path, so test via emit_toml
        let (ex, warnings) = extract_src("project(foo VERSION ${COMPUTED_VERSION})");
        let toml = emit_toml("foo", "0.1.0", &ex, &warnings);
        assert!(toml.contains("version = \"0.1.0\""));
    }

    // ── End-to-end TOML emission ──────────────────────────────────────────────

    #[test]
    fn emits_bin_target() {
        let (ex, w) = extract_src("project(app)\nadd_executable(app src/main.c)");
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(toml.contains("[[bin]]"));
        assert!(toml.contains("name = \"app\""));
        assert!(toml.contains("src = \"src/main.c\""));
    }

    #[test]
    fn emits_lib_target_with_srcs() {
        let (ex, w) = extract_src("project(mylib)\nadd_library(mylib STATIC a.c b.c)");
        let toml = emit_toml("mylib", "0.1.0", &ex, &w);
        assert!(toml.contains("[[lib]]"));
        assert!(toml.contains("srcs = [\"a.c\", \"b.c\"]"));
    }

    #[test]
    fn emits_dependencies() {
        let (ex, w) = extract_src(
            "project(app)\nfind_package(OpenSSL REQUIRED)\nfind_package(ZLIB REQUIRED)",
        );
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(toml.contains("[dependencies]"));
        assert!(toml.contains("openssl"));
        assert!(toml.contains("zlib"));
    }

    #[test]
    fn emits_cxx_standard() {
        let (ex, w) =
            extract_src("set(CMAKE_CXX_STANDARD 17)\nproject(app)\nadd_executable(app main.cpp)");
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(toml.contains("[language.cpp]"));
        assert!(toml.contains("std = \"c++17\""));
    }

    #[test]
    fn no_empty_sections_emitted() {
        // A project with no deps should not emit [dependencies]
        let (ex, w) = extract_src("project(app)\nadd_executable(app main.c)");
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(!toml.contains("[dependencies]"));
        assert!(!toml.contains("[compiler]"));
    }

    // ── FetchContent ──────────────────────────────────────────────────────────

    #[test]
    fn fetchcontent_git_with_tag() {
        let src = r#"
FetchContent_Declare(
  fmt
  GIT_REPOSITORY https://github.com/fmtlib/fmt.git
  GIT_TAG        10.2.1
)
FetchContent_MakeAvailable(fmt)
"#;
        let (ex, w) = extract_src(src);
        assert!(w.is_empty(), "unexpected warnings: {w:?}");
        assert_eq!(ex.fetched_deps.len(), 1);
        let d = &ex.fetched_deps[0];
        assert_eq!(d.name, "fmt");
        assert_eq!(d.url.as_deref(), Some("https://github.com/fmtlib/fmt.git"));
        assert_eq!(d.tag.as_deref(), Some("10.2.1"));
        assert!(d.branch.is_none() && d.rev.is_none());
    }

    #[test]
    fn fetchcontent_git_hash_becomes_rev() {
        let src = "FetchContent_Declare(mylib GIT_REPOSITORY https://github.com/x/y.git GIT_TAG a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2)";
        let (ex, _) = extract_src(src);
        assert_eq!(
            ex.fetched_deps[0].rev.as_deref(),
            Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2")
        );
        assert!(ex.fetched_deps[0].tag.is_none());
    }

    #[test]
    fn fetchcontent_git_branch() {
        let src =
            "FetchContent_Declare(mylib GIT_REPOSITORY https://github.com/x/y.git GIT_TAG main)";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.fetched_deps[0].branch.as_deref(), Some("main"));
        assert!(ex.fetched_deps[0].tag.is_none() && ex.fetched_deps[0].rev.is_none());
    }

    #[test]
    fn fetchcontent_url_with_sha256_prefix() {
        let src = r#"
FetchContent_Declare(
  zlib
  URL      https://zlib.net/zlib-1.3.tar.gz
  URL_HASH SHA256=ff0ba4c292013dbc27530b3a81e1f9a813cd39de0fb13876d0e4ac6e8f11f0a7
)
"#;
        let (ex, _) = extract_src(src);
        let d = &ex.fetched_deps[0];
        assert_eq!(d.name, "zlib");
        assert_eq!(d.url.as_deref(), Some("https://zlib.net/zlib-1.3.tar.gz"));
        assert_eq!(
            d.sha256.as_deref(),
            Some("ff0ba4c292013dbc27530b3a81e1f9a813cd39de0fb13876d0e4ac6e8f11f0a7")
        );
        assert!(d.url.as_deref().map_or(true, |u| !u.ends_with(".git")));
    }

    #[test]
    fn fetchcontent_no_source_emits_warning() {
        let (ex, w) = extract_src("FetchContent_Declare(foo DOWNLOAD_EXTRACT_TIMESTAMP TRUE)");
        assert!(ex.fetched_deps.is_empty());
        assert!(w.iter().any(|s| s.contains("foo")));
    }

    #[test]
    fn fetchcontent_deduplicated() {
        let src = r#"
FetchContent_Declare(fmt GIT_REPOSITORY https://github.com/fmtlib/fmt.git GIT_TAG 10.2.1)
FetchContent_Declare(fmt GIT_REPOSITORY https://github.com/fmtlib/fmt.git GIT_TAG 11.0.0)
"#;
        let (ex, _) = extract_src(src);
        assert_eq!(
            ex.fetched_deps.len(),
            1,
            "duplicate declare should be ignored"
        );
        assert_eq!(ex.fetched_deps[0].tag.as_deref(), Some("10.2.1"));
    }

    #[test]
    fn fetchcontent_shadows_find_package() {
        // When a dep is declared via FetchContent it should appear as an inline
        // table, not as a bare "*" version even if find_package also references it.
        let src = r#"
FetchContent_Declare(fmt GIT_REPOSITORY https://github.com/fmtlib/fmt.git GIT_TAG 10.2.1)
find_package(fmt REQUIRED)
"#;
        let (ex, w) = extract_src(src);
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(
            toml.contains("url = "),
            "expected inline url dep, got:\n{toml}"
        );
        // Should not also appear as a bare-string system dep (`fmt = "…"`);
        // it's an inline `{ url = … }` fetched dep only.
        assert!(!toml.contains("fmt = \""));
    }

    // ── ExternalProject_Add ───────────────────────────────────────────────────

    #[test]
    fn externalproject_git() {
        let src = r#"
ExternalProject_Add(
  mylib
  GIT_REPOSITORY https://github.com/user/mylib.git
  GIT_TAG        v1.0.0
)
"#;
        let (ex, _) = extract_src(src);
        let d = &ex.fetched_deps[0];
        assert_eq!(d.name, "mylib");
        assert_eq!(d.url.as_deref(), Some("https://github.com/user/mylib.git"));
        assert_eq!(d.tag.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn externalproject_custom_commands_warned() {
        let src = r#"
ExternalProject_Add(
  mylib
  GIT_REPOSITORY https://github.com/user/mylib.git
  GIT_TAG        v1.0.0
  BUILD_COMMAND  make special
)
"#;
        let (_, w) = extract_src(src);
        assert!(w.iter().any(|s| s.contains("custom build")));
    }

    // ── CPMAddPackage ─────────────────────────────────────────────────────────

    #[test]
    fn cpm_keyword_github() {
        let src = r#"
CPMAddPackage(
  NAME fmt
  GITHUB_REPOSITORY fmtlib/fmt
  GIT_TAG 10.2.1
)
"#;
        let (ex, w) = extract_src(src);
        assert!(w.is_empty(), "unexpected warnings: {w:?}");
        let d = &ex.fetched_deps[0];
        assert_eq!(d.name, "fmt");
        assert_eq!(d.url.as_deref(), Some("https://github.com/fmtlib/fmt.git"));
        assert_eq!(d.tag.as_deref(), Some("10.2.1"));
    }

    #[test]
    fn cpm_keyword_version_synthesises_tag() {
        let src = "CPMAddPackage(NAME catch2 VERSION 3.4.0 GITHUB_REPOSITORY catchorg/Catch2)";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.fetched_deps[0].tag.as_deref(), Some("v3.4.0"));
    }

    #[test]
    fn cpm_compact_gh() {
        let (ex, w) = extract_src("CPMAddPackage(\"gh:fmtlib/fmt#10.2.1\")");
        assert!(w.is_empty(), "unexpected warnings: {w:?}");
        let d = &ex.fetched_deps[0];
        assert_eq!(d.name, "fmt");
        assert_eq!(d.url.as_deref(), Some("https://github.com/fmtlib/fmt.git"));
        assert_eq!(d.tag.as_deref(), Some("10.2.1"));
    }

    #[test]
    fn cpm_compact_gl() {
        let (ex, _) = extract_src("CPMAddPackage(\"gl:user/mylib#develop\")");
        let d = &ex.fetched_deps[0];
        assert_eq!(d.url.as_deref(), Some("https://gitlab.com/user/mylib.git"));
        assert_eq!(d.branch.as_deref(), Some("develop"));
    }

    // ── Emitter ───────────────────────────────────────────────────────────────

    #[test]
    fn emits_fetched_dep_as_inline_table() {
        let src = "FetchContent_Declare(fmt GIT_REPOSITORY https://github.com/fmtlib/fmt.git GIT_TAG 10.2.1)";
        let (ex, w) = extract_src(src);
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(toml.contains("[dependencies]"));
        assert!(toml.contains("url = \"https://github.com/fmtlib/fmt.git\""));
        assert!(toml.contains("tag = \"10.2.1\""));
    }

    #[test]
    fn emits_fetched_url_dep() {
        let src = "FetchContent_Declare(zlib URL https://zlib.net/zlib-1.3.tar.gz URL_HASH SHA256=abc123)";
        let (ex, w) = extract_src(src);
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(toml.contains("url = \"https://zlib.net/zlib-1.3.tar.gz\""));
        assert!(toml.contains("sha256 = \"abc123\""));
    }

    // ── add_compile_options / target_compile_options ──────────────────────────

    #[test]
    fn compile_options_extracts_std() {
        let src = "add_compile_options(-std=c++17 -Wall -O2)";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.cxx_std, Some("c++17".to_string()));
    }

    #[test]
    fn target_compile_options_skips_target_name() {
        let src = "target_compile_options(mylib PRIVATE -std=c++20 -DFOO)";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.cxx_std, Some("c++20".to_string()));
        assert!(ex.defines.contains(&"FOO".to_string()));
    }

    #[test]
    fn compile_options_gnu_std_normalised() {
        let src = "add_compile_options(-std=gnu++14)";
        let (ex, _) = extract_src(src);
        assert_eq!(ex.cxx_std, Some("c++14".to_string()));
    }

    #[test]
    fn compile_options_does_not_duplicate_std_from_set() {
        // CMAKE_CXX_STANDARD takes precedence (set earlier in Extracted::new state)
        let src = "set(CMAKE_CXX_STANDARD 17)\nadd_compile_options(-std=c++20)";
        let (ex, _) = extract_src(src);
        // First assignment wins
        assert_eq!(ex.cxx_std, Some("c++17".to_string()));
    }

    // ── option / cmake_dependent_option ──────────────────────────────────────

    #[test]
    fn option_off_by_default() {
        let src = "option(ENABLE_TLS \"Enable TLS support\" OFF)";
        let (ex, _) = extract_src(src);
        assert!(ex.features.iter().any(|(n, on)| n == "enable_tls" && !on));
    }

    #[test]
    fn option_on_by_default() {
        let src = "option(WITH_LOGGING \"Enable logging\" ON)";
        let (ex, _) = extract_src(src);
        assert!(ex.features.iter().any(|(n, on)| n == "with_logging" && *on));
    }

    #[test]
    fn cmake_dependent_option_captured() {
        let src = "cmake_dependent_option(ENABLE_SSL \"SSL support\" ON \"UNIX\" OFF)";
        let (ex, _) = extract_src(src);
        assert!(ex.features.iter().any(|(n, _)| n == "enable_ssl"));
    }

    #[test]
    fn cmake_internal_options_skipped() {
        let src =
            "option(BUILD_TESTING \"Build tests\" OFF)\noption(BUILD_SHARED_LIBS \"Shared\" ON)";
        let (ex, _) = extract_src(src);
        assert!(
            ex.features.is_empty(),
            "cmake internals must not appear in features"
        );
    }

    #[test]
    fn features_emitted_with_default() {
        let src = "option(LOGGING \"Enable logging\" ON)\noption(TLS \"TLS support\" OFF)";
        let (ex, w) = extract_src(src);
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(
            toml.contains("[features]"),
            "expected [features] section:\n{toml}"
        );
        assert!(
            toml.contains("logging"),
            "expected logging feature:\n{toml}"
        );
        assert!(toml.contains("default"), "expected default array:\n{toml}");
    }
}
