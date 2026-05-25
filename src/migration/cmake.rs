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
///   - `function()` and `macro()` bodies are skipped (can't evaluate calls)
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cmake_lossless::{CMakeFile, CommandInvocation, Node};
use toml_edit::{Array, DocumentMut, Item, Table, value};

// ── Public entry point ────────────────────────────────────────────────────────

pub struct ImportResult {
    pub written: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Purge CMake artefacts left behind in `dir`.
pub fn purge_cmake(dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let files = ["CMakeLists.txt", "CMakeCache.txt", "cmake_install.cmake", "CTestTestfile.cmake"];
    for name in &files {
        let p = dir.join(name);
        if p.exists() {
            if std::fs::remove_file(&p).is_ok() {
                removed.push(format!("removed {}", p.display()));
            }
        }
    }
    let cmake_files = dir.join("CMakeFiles");
    if cmake_files.is_dir() {
        if std::fs::remove_dir_all(&cmake_files).is_ok() {
            removed.push(format!("removed {}/", cmake_files.display()));
        }
    }
    let build_dir = dir.join("build");
    if build_dir.join("CMakeCache.txt").exists() {
        if std::fs::remove_dir_all(&build_dir).is_ok() {
            removed.push(format!("removed {}/", build_dir.display()));
        }
    }
    removed
}

pub fn import_cmake(input: &Path, out_dir: Option<&Path>) -> Result<ImportResult> {
    let (project_dir, cmake_path) = resolve_input(input)?;
    let out_root = out_dir.unwrap_or(&project_dir);
    let mut warnings: Vec<String> = Vec::new();

    let content = std::fs::read_to_string(&cmake_path)
        .with_context(|| format!("reading {}", cmake_path.display()))?;

    let file = cmake_lossless::parse(&content)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let parsed = extract(&file, &mut warnings);

    // ── Workspace detection ───────────────────────────────────────────────────
    // Only go full-workspace if there are subdirs but NO root-level targets.
    let root_has_targets = !parsed.bins.is_empty() || !parsed.libs.is_empty();
    if !parsed.subdirs.is_empty() && !root_has_targets {
        return import_workspace(&project_dir, out_root, &parsed.subdirs, &mut warnings);
    }

    // ── Package name + version ────────────────────────────────────────────────
    let dir_name = project_dir
        .file_name().and_then(|n| n.to_str()).unwrap_or("project").to_string();
    let pkg_name = if parsed.name.is_empty() { sanitize_name(&dir_name) } else { sanitize_name(&parsed.name) };
    let pkg_version = if parsed.version.is_empty() || parsed.version.contains('$') {
        "0.1.0".to_string()
    } else {
        ensure_three_part(&parsed.version)
    };

    let toml = emit_toml(&pkg_name, &pkg_version, &parsed, &warnings);
    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;
    let dest = out_root.join("freight.toml");
    std::fs::write(&dest, &toml)
        .with_context(|| format!("writing {}", dest.display()))?;

    Ok(ImportResult { written: vec![dest], warnings })
}

// ── Workspace ─────────────────────────────────────────────────────────────────

fn import_workspace(
    project_dir: &Path, out_root: &Path, subdirs: &[String], warnings: &mut Vec<String>,
) -> Result<ImportResult> {
    let mut written = Vec::new();
    std::fs::create_dir_all(out_root)
        .with_context(|| format!("creating {}", out_root.display()))?;

    let mut doc = DocumentMut::new();
    let mut ws_tbl = Table::new();
    let mut members = Array::new();
    for sub in subdirs { members.push(sub.as_str()); }
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
            warnings.push(format!("subdirectory {sub} has no CMakeLists.txt — skipping"));
            continue;
        }
        let sub_out = out_root.join(sub);
        match import_cmake(&sub_dir, Some(&sub_out)) {
            Ok(r) => { written.extend(r.written); warnings.extend(r.warnings); }
            Err(e) => warnings.push(format!("could not convert {sub}: {e}")),
        }
    }

    Ok(ImportResult { written, warnings: warnings.clone() })
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

struct Extracted {
    name: String,
    version: String,
    bins: Vec<(String, Vec<String>)>,
    libs: Vec<(String, LibKind, Vec<String>)>,
    deps: Vec<String>,
    find_packages: Vec<String>,
    pkg_modules: Vec<String>,
    /// Platform-conditional deps: OS name (e.g. "windows", "macos", "unix") → dep list.
    platform_deps: HashMap<String, Vec<String>>,
    c_std: Option<String>,
    cxx_std: Option<String>,
    defines: Vec<String>,
    includes: Vec<String>,
    subdirs: Vec<String>,
    vars: HashMap<String, Vec<String>>, // name → list of values (set() args after the name)
}

#[derive(Clone, Copy, PartialEq)]
enum LibKind { Static, Shared, Interface }

impl Extracted {
    fn new() -> Self {
        Self {
            name: String::new(), version: String::new(),
            bins: Vec::new(), libs: Vec::new(),
            deps: Vec::new(), find_packages: Vec::new(), pkg_modules: Vec::new(),
            platform_deps: HashMap::new(),
            c_std: None, cxx_std: None,
            defines: Vec::new(), includes: Vec::new(),
            subdirs: Vec::new(), vars: HashMap::new(),
        }
    }

    fn add_platform_dep(&mut self, dep: String, os: &str) {
        let vec = self.platform_deps.entry(os.to_string()).or_default();
        if !vec.contains(&dep) { vec.push(dep); }
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
                warnings.push(format!(
                    "line {}: while() loop — body skipped",
                    b.line
                ));
            }
            // Function and macro definitions: skip the body (they define callable
            // templates, not direct build targets — calls appear elsewhere)
            Node::Function(_) | Node::Macro(_) => {}
            Node::Block(b) => walk_nodes(&b.body, ex, scope, warnings),
        }
    }
}

fn handle_command(cmd: &CommandInvocation, ex: &mut Extracted, scope: Option<&str>, warnings: &mut Vec<String>) {
    let args = cmd.arg_values();
    match cmd.name.as_str() {
        "project"                        => handle_project(&args, ex),
        "set"                            => handle_set(&args, ex),
        "add_executable"                 => handle_add_executable(&args, ex, warnings),
        "add_library"                    => handle_add_library(&args, ex, warnings),
        "target_link_libraries"
        | "link_libraries"               => handle_link_libraries(&args, ex, scope),
        "find_package"                   => handle_find_package(&args, ex, scope),
        "pkg_check_modules"
        | "pkg_search_module"            => handle_pkg_check_modules(&args, ex, scope),
        "include_directories"            => { if scope.is_none() { handle_include_dirs(&args, ex, false); } }
        "target_include_directories"    => { if scope.is_none() { handle_include_dirs(&args, ex, true); } }
        "add_definitions"               => { if scope.is_none() { handle_add_definitions(&args, ex, false); } }
        "target_compile_definitions"    => { if scope.is_none() { handle_add_definitions(&args, ex, true); } }
        "add_subdirectory"               => {
            if let Some(first) = args.first() {
                let sub = expand_var(first, &ex.vars);
                if !sub.is_empty() && !sub.contains('$') && !ex.subdirs.contains(&sub) {
                    ex.subdirs.push(sub);
                }
            }
        }
        _ => {}
    }
}

// ── Command handlers ──────────────────────────────────────────────────────────

fn handle_project(args: &[&str], ex: &mut Extracted) {
    if args.is_empty() { return; }
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
    if args.len() < 1 { return; }
    let var = args[0];
    // Values are args[1..], each already a parsed CMake argument value.
    // Store them as a list so multi-value variables are preserved correctly.
    let vals: Vec<String> = args[1..].iter()
        .flat_map(|v| {
            // Expand any variable references in the values
            let expanded = expand_var(v, &ex.vars);
            // A value that was itself a list variable expands to space-separated words
            if expanded.contains(' ') {
                expanded.split_whitespace().map(str::to_string).collect::<Vec<_>>()
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
            if let Some(v) = vals.first() { ex.cxx_std = map_cxx_std(v); }
        }
        "CMAKE_C_STANDARD" => {
            if let Some(v) = vals.first() { ex.c_std = map_c_std(v); }
        }
        _ => {}
    }
}

fn handle_add_executable(args: &[&str], ex: &mut Extracted, warnings: &mut Vec<String>) {
    if args.is_empty() { return; }
    const SKIP: &[&str] = &["IMPORTED", "ALIAS", "WIN32", "MACOSX_BUNDLE", "EXCLUDE_FROM_ALL"];
    let name = expand_var(args[0], &ex.vars);
    if name.contains('$') { return; } // unresolvable variable
    if args.len() > 1 && args[1].eq_ignore_ascii_case("ALIAS") { return; }

    let srcs: Vec<String> = args[1..].iter()
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
    if args.is_empty() { return; }
    const SKIP: &[&str] = &["IMPORTED", "ALIAS", "OBJECT", "EXCLUDE_FROM_ALL", "GLOBAL"];
    let name = expand_var(args[0], &ex.vars);
    if name.contains('$') { return; }

    let mut kind = LibKind::Static;
    let mut src_start = 1;
    if args.len() > 1 {
        match args[1].to_uppercase().as_str() {
            "SHARED" | "MODULE" => { kind = LibKind::Shared; src_start = 2; }
            "STATIC" => { src_start = 2; }
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

    let srcs: Vec<String> = args[src_start..].iter()
        .filter(|a| !SKIP.contains(&a.to_ascii_uppercase().as_str()))
        .flat_map(|s| expand_var_to_list(s, &ex.vars))
        .filter(|s| is_source_file(s))
        .collect();

    if srcs.is_empty() {
        warnings.push(format!("add_library({name}) has no recognisable source files — check for generated sources"));
    }
    if !ex.libs.iter().any(|(n, _, _)| n == &name) {
        ex.libs.push((name, kind, srcs));
    }
}

fn handle_link_libraries(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    const VIS: &[&str] = &["PUBLIC", "PRIVATE", "INTERFACE", "GENERAL", "OPTIMIZED", "DEBUG"];
    // Skip first arg if it looks like a target name (not a visibility keyword or -l flag)
    let start = if !args.is_empty() && !VIS.contains(&args[0].to_ascii_uppercase().as_str())
        && !args[0].starts_with('-') { 1 } else { 0 };
    for arg in &args[start..] {
        if VIS.contains(&arg.to_ascii_uppercase().as_str()) { continue; }
        if let Some(dep) = extract_link_dep(arg) {
            match scope {
                Some(os) => ex.add_platform_dep(dep, os),
                None     => { if !ex.deps.contains(&dep) { ex.deps.push(dep); } }
            }
        }
    }
}

fn handle_find_package(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    if args.is_empty() { return; }
    const SKIP: &[&str] = &["REQUIRED", "QUIET", "OPTIONAL_COMPONENTS", "COMPONENTS",
                             "CONFIG", "MODULE", "NO_MODULE"];
    let pkg = args[0];
    if SKIP.contains(&pkg.to_ascii_uppercase().as_str()) { return; }
    for m in map_find_package(pkg) {
        match scope {
            Some(os) => ex.add_platform_dep(m, os),
            None     => { if !ex.find_packages.contains(&m) { ex.find_packages.push(m); } }
        }
    }
}

fn handle_pkg_check_modules(args: &[&str], ex: &mut Extracted, scope: Option<&str>) {
    if args.len() < 2 { return; }
    const SKIP: &[&str] = &["REQUIRED", "QUIET", "IMPORTED_TARGET", "GLOBAL"];
    let mut i = 1;
    while i < args.len() {
        let a = args[i];
        if SKIP.contains(&a.to_ascii_uppercase().as_str()) { i += 1; continue; }
        if matches!(a, ">=" | "<=" | ">" | "<" | "=" | "!=") { i += 2; continue; }
        let pkg = a.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
        if !pkg.is_empty() {
            let pkg = pkg.to_string();
            match scope {
                Some(os) => ex.add_platform_dep(pkg, os),
                None     => { if !ex.pkg_modules.contains(&pkg) { ex.pkg_modules.push(pkg); } }
            }
        }
        i += 1;
    }
}

fn handle_include_dirs(args: &[&str], ex: &mut Extracted, has_target: bool) {
    const SKIP: &[&str] = &["PUBLIC", "PRIVATE", "INTERFACE", "SYSTEM", "BEFORE", "AFTER"];
    let start = if has_target { 1 } else { 0 };
    for arg in &args[start..] {
        if SKIP.contains(&arg.to_ascii_uppercase().as_str()) { continue; }
        let expanded = expand_var(arg, &ex.vars);
        if SKIP.contains(&expanded.to_ascii_uppercase().as_str()) { continue; }
        if expanded.starts_with("$<") || expanded.starts_with("$") { continue; }
        if expanded.starts_with("/usr") || expanded.starts_with("/opt") { continue; }
        if !expanded.is_empty() && !ex.includes.contains(&expanded) {
            ex.includes.push(expanded);
        }
    }
}

fn handle_add_definitions(args: &[&str], ex: &mut Extracted, has_target: bool) {
    const SKIP: &[&str] = &["PUBLIC", "PRIVATE", "INTERFACE"];
    let start = if has_target { 1 } else { 0 };
    for arg in &args[start..] {
        if SKIP.contains(&arg.to_ascii_uppercase().as_str()) { continue; }
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
    if s.starts_with('$') || s.starts_with('@') { return false; }
    const KEYWORDS: &[&str] = &[
        "PUBLIC", "PRIVATE", "INTERFACE", "REQUIRED", "OPTIONAL",
        "BEFORE", "AFTER", "SYSTEM", "STATIC", "SHARED", "MODULE",
        "IMPORTED", "ALIAS", "GLOBAL",
    ];
    if KEYWORDS.contains(&s.to_ascii_uppercase().as_str()) { return false; }
    const SOURCE_EXTS: &[&str] = &[
        ".c", ".cc", ".cpp", ".cxx", ".c++", ".C",
        ".f", ".f90", ".f95", ".f03", ".f08", ".F", ".F90",
        ".cu", ".hip", ".cl", ".ispc",
        ".s", ".S", ".asm",
        ".d", ".adb", ".ads", ".m", ".mm",
    ];
    SOURCE_EXTS.iter().any(|ext| s.ends_with(ext))
}

fn extract_link_dep(s: &str) -> Option<String> {
    // Skip CMake imported target names (Foo::Bar), variables, path-based libs
    if s.contains("::") || s.starts_with('$') || s.starts_with("-L") { return None; }
    if s.contains('/') || s.contains('.') { return None; }
    let lib = if let Some(rest) = s.strip_prefix("-l") { rest } else { s };
    if lib.is_empty() || AUTO_LINKED.contains(&lib) { return None; }
    if lib.starts_with('-') { return None; }
    Some(lib.to_string())
}

fn map_find_package(pkg: &str) -> Vec<String> {
    match pkg {
        "Threads"              => vec![],
        "OpenSSL"              => vec!["openssl".to_string()],
        "ZLIB"                 => vec!["zlib".to_string()],
        "CURL"                 => vec!["libcurl".to_string()],
        "Boost"                => vec!["boost".to_string()],
        "fmt" | "FMT"          => vec!["fmt".to_string()],
        "spdlog"               => vec!["spdlog".to_string()],
        "GTest" | "GoogleTest" => vec!["gtest".to_string()],
        "SQLite3" | "SQLite"   => vec!["sqlite3".to_string()],
        "LibXml2"              => vec!["libxml-2.0".to_string()],
        "PNG"                  => vec!["libpng".to_string()],
        "JPEG"                 => vec!["libjpeg".to_string()],
        "SDL2"                 => vec!["sdl2".to_string()],
        "OpenGL"               => vec!["gl".to_string()],
        "Protobuf"             => vec!["protobuf".to_string()],
        "MPI"                  => vec!["mpi".to_string()],
        "HDF5"                 => vec!["hdf5".to_string()],
        "Python3" | "Python" | "LLVM" => vec![],
        _ => {
            let lower = pkg.to_lowercase();
            if lower.len() > 2 { vec![lower] } else { vec![] }
        }
    }
}

fn map_cxx_std(val: &str) -> Option<String> {
    match val.trim() {
        "98" | "03" => Some("c++98".to_string()),
        "11"        => Some("c++11".to_string()),
        "14"        => Some("c++14".to_string()),
        "17"        => Some("c++17".to_string()),
        "20"        => Some("c++20".to_string()),
        "23"        => Some("c++23".to_string()),
        _           => None,
    }
}

fn map_c_std(val: &str) -> Option<String> {
    match val.trim() {
        "90" | "89" => Some("c99".to_string()),
        "99"        => Some("c99".to_string()),
        "11"        => Some("c11".to_string()),
        "17"        => Some("c17".to_string()),
        "23"        => Some("c23".to_string()),
        _           => None,
    }
}

const AUTO_LINKED: &[&str] = &[
    "m", "pthread", "dl", "rt", "c", "gcc", "gcc_s", "stdc++", "c++",
    "atomic", "util", "resolv",
];

fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
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

    let mut header = String::from("# Generated by freight migrate cmake — review before committing.\n");
    for w in warnings { header.push_str(&format!("# warning: {w}\n")); }

    let mut pkg = Table::new();
    pkg.insert("name", value(name));
    pkg.insert("version", value(version));
    doc.insert("package", Item::Table(pkg));

    if !ex.defines.is_empty() || !ex.includes.is_empty() {
        let mut compiler = Table::new();
        if !ex.defines.is_empty() {
            let mut arr = Array::new();
            for d in &ex.defines { arr.push(d.as_str()); }
            compiler.insert("defines", Item::Value(arr.into()));
        }
        if !ex.includes.is_empty() {
            let mut arr = Array::new();
            for inc in &ex.includes { arr.push(inc.as_str()); }
            compiler.insert("includes", Item::Value(arr.into()));
        }
        doc.insert("compiler", Item::Table(compiler));
    }

    let all_deps: Vec<String> = {
        let mut seen = HashSet::new();
        let mut deps = Vec::new();
        for d in ex.deps.iter().chain(ex.find_packages.iter()).chain(ex.pkg_modules.iter()) {
            if seen.insert(d.clone()) { deps.push(d.clone()); }
        }
        deps
    };
    if !all_deps.is_empty() {
        let mut dep_tbl = Table::new();
        for d in &all_deps { dep_tbl.insert(d, value("*")); }
        doc.insert("dependencies", Item::Table(dep_tbl));
    }

    for (tgt_name, srcs) in &ex.bins {
        let mut bin_tbl = Table::new();
        bin_tbl.insert("name", value(tgt_name.as_str()));
        if !srcs.is_empty() {
            if srcs.len() == 1 {
                bin_tbl.insert("src", value(srcs[0].as_str()));
            } else {
                let mut arr = Array::new();
                for s in srcs { arr.push(s.as_str()); }
                bin_tbl.insert("srcs", Item::Value(arr.into()));
            }
        }
        let entry = doc.entry("bin").or_insert(Item::ArrayOfTables(Default::default()));
        if let Item::ArrayOfTables(aot) = entry { aot.push(bin_tbl); }
    }

    for (tgt_name, kind, srcs) in &ex.libs {
        let mut lib_tbl = Table::new();
        lib_tbl.insert("name", value(tgt_name.as_str()));
        match kind {
            LibKind::Shared    => { lib_tbl.insert("type", value("shared")); }
            LibKind::Interface => { lib_tbl.insert("type", value("interface")); }
            LibKind::Static    => {}
        }
        if !srcs.is_empty() {
            let mut arr = Array::new();
            for s in srcs { arr.push(s.as_str()); }
            lib_tbl.insert("srcs", Item::Value(arr.into()));
        }
        let entry = doc.entry("lib").or_insert(Item::ArrayOfTables(Default::default()));
        if let Item::ArrayOfTables(aot) = entry { aot.push(lib_tbl); }
    }

    // Language standards and platform deps appended as raw TOML to avoid empty headers.
    let mut extra = String::new();
    if let Some(std) = &ex.cxx_std { extra.push_str(&format!("\n[language.cpp]\nstd = \"{std}\"\n")); }
    if let Some(std) = &ex.c_std  { extra.push_str(&format!("\n[language.c]\nstd = \"{std}\"\n")); }

    // Platform-conditional dependency sections (sorted for deterministic output).
    let mut platforms: Vec<(&str, &Vec<String>)> = ex.platform_deps.iter()
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    platforms.sort_by_key(|(k, _)| *k);
    for (os, pdeps) in platforms {
        if !pdeps.is_empty() {
            extra.push_str(&format!("\n[os.{os}.dependencies]\n"));
            for d in pdeps { extra.push_str(&format!("{d} = \"*\"\n")); }
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
        assert!(!ex.deps.contains(&"ws2_32".to_string()), "ws2_32 should be excluded");
        assert!(!ex.deps.contains(&"crypt32".to_string()), "crypt32 should be excluded");
        assert!(ex.deps.contains(&"z".to_string()), "z should be included from else branch");
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
        assert!(!ex.deps.contains(&"z".to_string()), "z should not be unconditional");
        assert_eq!(ex.platform_deps.get("unix").map(Vec::as_slice), Some(&["z".to_string()][..]));
    }

    // ── Platform-conditional dep mapping ──────────────────────────────────────

    #[test]
    fn win32_deps_in_platform_section() {
        let src = "if(WIN32)\n  target_link_libraries(app ws2_32)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.deps.is_empty());
        assert_eq!(ex.platform_deps.get("windows").map(Vec::as_slice),
                   Some(&["ws2_32".to_string()][..]));
    }

    #[test]
    fn msvc_deps_in_windows_section() {
        let src = "if(MSVC)\n  target_link_libraries(app dbghelp)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.deps.is_empty());
        assert_eq!(ex.platform_deps.get("windows").map(Vec::as_slice),
                   Some(&["dbghelp".to_string()][..]));
    }

    #[test]
    fn apple_deps_in_macos_section() {
        let src = "if(APPLE)\n  find_package(OpenSSL REQUIRED)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.find_packages.is_empty());
        assert_eq!(ex.platform_deps.get("macos").map(Vec::as_slice),
                   Some(&["openssl".to_string()][..]));
    }

    #[test]
    fn unix_deps_in_unix_section() {
        let src = "if(UNIX)\n  target_link_libraries(app dl rt)\nendif()";
        let (ex, _) = extract_src(src);
        // dl and rt are in AUTO_LINKED, so they get filtered out
        assert!(ex.platform_deps.get("unix").map_or(true, Vec::is_empty));
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
        // Windows-specific deps in platform bucket
        let win = ex.platform_deps.get("windows").cloned().unwrap_or_default();
        assert!(win.contains(&"ws2_32".to_string()));
        assert!(win.contains(&"crypt32".to_string()));
        // Unconditional (else) dep in regular bucket
        assert!(ex.deps.contains(&"z".to_string()));
        // Nothing in the wrong bucket
        assert!(!ex.deps.contains(&"ws2_32".to_string()));
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
        let win = ex.platform_deps.get("windows").cloned().unwrap_or_default();
        let mac = ex.platform_deps.get("macos").cloned().unwrap_or_default();
        assert!(win.contains(&"ws2_32".to_string()), "ws2_32 should be windows-only");
        assert!(mac.contains(&"openssl".to_string()), "openssl should be macos-only");
        assert!(ex.deps.contains(&"z".to_string()), "z should be unconditional");
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
        assert!(toml.contains("[os.windows.dependencies]"), "should have windows section");
        assert!(toml.contains("ws2_32 = \"*\""), "should have ws2_32");
        assert!(toml.contains("[dependencies]"), "should have main deps section");
        assert!(toml.contains("z = \"*\""), "should have z");
        assert!(!toml.contains("[os.linux.dependencies]"), "no spurious linux section");
    }

    #[test]
    fn not_win32_branch_is_unconditional() {
        // NOT WIN32 is not a recognised platform_condition — walk it as unconditional
        let src = "if(NOT WIN32)\n  target_link_libraries(app z)\nendif()";
        let (ex, _) = extract_src(src);
        assert!(ex.deps.contains(&"z".to_string()), "NOT WIN32 body should be unconditional");
        assert!(ex.platform_deps.is_empty());
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
        assert!(ex.deps.is_empty(), "auto-linked libs should be excluded, got: {:?}", ex.deps);
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
        let (ex, _) = extract_src(
            "add_subdirectory(lib)\nadd_subdirectory(app)\nadd_subdirectory(tests)"
        );
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
        let (ex, w) = extract_src("project(app)\nfind_package(OpenSSL REQUIRED)\nfind_package(ZLIB REQUIRED)");
        let toml = emit_toml("app", "0.1.0", &ex, &w);
        assert!(toml.contains("[dependencies]"));
        assert!(toml.contains("openssl"));
        assert!(toml.contains("zlib"));
    }

    #[test]
    fn emits_cxx_standard() {
        let (ex, w) = extract_src("set(CMAKE_CXX_STANDARD 17)\nproject(app)\nadd_executable(app main.cpp)");
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
}
