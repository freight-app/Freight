//! `build.freight` — optional Rhai pre-build script.
//!
//! If a `build.freight` file exists in the project root, it is evaluated once
//! *after* dependencies are compiled but *before* the project's own sources are
//! compiled. The script communicates back to the build engine through a set of
//! registered functions and map objects:
//!
//! ```rhai
//! // ── Context functions ────────────────────────────────────────────────────
//! let name = package_name();    // "myproject"
//! let ver  = package_version(); // "0.1.0"
//! let prof = profile();         // "dev" | "release"
//! let out  = out_dir();         // "target/dev/build/"
//! let src  = source_dir();      // project root
//!
//! // ── toolchain map (read-only) ────────────────────────────────────────────
//! let backend = toolchain["backend"]; // "gcc" | "clang" | "auto" | …
//! let arch    = toolchain["arch"];    // "x86_64" | "aarch64" | …
//! let os      = toolchain["os"];      // "linux" | "macos" | "windows" | …
//! let ver     = toolchain["version"]; // "13.2.0" | "" when unknown
//! let tgt     = toolchain["target"];  // "aarch64-linux-gnu" | "" for native
//!
//! // ── env map (read / write) ───────────────────────────────────────────────
//! let cc = env["CC"];                   // "" when unset
//! env["PKG_CONFIG_PATH"] = "/opt/lib/pkgconfig";
//!
//! // ── Output setters ───────────────────────────────────────────────────────
//! define("NDEBUG");               // → -DNDEBUG
//! define_value("VERSION", "1.2.3"); // → -DVERSION=1.2.3
//! add_include(out);               // extra -I path
//! add_flag("-march=native");      // raw compiler flag
//! add_link_lib("z");              // → -lz
//! add_link_flag("-L/opt/local/lib");
//! link_path("/opt/local/lib");    // convenience alias for add_link_flag("-L…")
//! add_source(out + "/generated.cpp"); // compile a generated source file
//!
//! // ── File helpers ─────────────────────────────────────────────────────────
//! write_file(out + "/stub.h", "#pragma once\n");  // no-op when unchanged
//! let content = read_file("config.txt");          // "" on error
//! let exists  = path_exists("include/foo.h");     // bool
//! let stem    = file_stem("proto/msgs.proto");    // "msgs"
//! let name    = file_name("proto/msgs.proto");    // "msgs.proto"
//! let dir     = file_dir("proto/msgs.proto");     // "proto"
//!
//! // ── File discovery ───────────────────────────────────────────────────────
//! //
//! // glob(pattern)           → all matching files (sorted), relative to project root
//! // changed_files(pattern)  → subset of glob that is newer than the last build stamp
//! //
//! // Patterns support * (within a directory), ** (across directories), and ?.
//! // Both functions return arrays of path strings.
//! //
//! // Typical use — re-run a code generator only for files that changed:
//!
//! for file in changed_files("proto/**/*.proto") {
//!     let r = run("protoc", ["--proto_path=proto", "--cpp_out=" + out, file]);
//!     if !r.ok { fail("protoc failed for " + file + ": " + r.stderr); }
//!     add_source(out + "/" + file_stem(file) + ".pb.cc");
//!     rerun_if(file);
//! }
//!
//! // Run cmake configure only when CMakeLists changed or build dir is absent:
//! if changed_files("CMakeLists.txt").len() > 0 || !path_exists(out + "/Makefile") {
//!     run("cmake", ["-S", source_dir(), "-B", out, "-DCMAKE_BUILD_TYPE=Release"]);
//! }
//!
//! // ── Diagnostics ──────────────────────────────────────────────────────────
//! warning("zlib not found, disabling compression");
//! fail("openssl not found — install libssl-dev");
//!
//! // ── packages map — resolved pkg-config deps (read-only) ─────────────────
//! //
//! // Declare deps in freight.toml:
//! //   [dependencies]
//! //   zlib = { pkg-config = "zlib", optional = true }
//! //
//! // freight resolves them before running this script; the script just
//! // checks the result and adds the feature define:
//!
//! if packages["zlib"].found {
//!     define("HAVE_ZLIB");
//!     // No pkg_config_apply needed — freight already injected -I and -l flags.
//! }
//!
//! // packages["name"].version → "1.2.8" (or "" when not found)
//!
//! // ── Environment probing ──────────────────────────────────────────────────
//! let git = run("git", ["rev-parse", "--short", "HEAD"]);
//! if git.ok { /* git.stdout, git.stderr, git.status */ }
//!
//! let cmake = find_tool("cmake"); // full path or ()
//!
//! // ── Incremental re-execution ─────────────────────────────────────────────
//! // Once rerun_if is called at least once, freight caches this script's output
//! // and skips re-execution on future builds unless a listed path (or
//! // build.freight itself) has changed.  changed_files() uses the same stamp
//! // as its reference time.
//! rerun_if("CMakeLists.txt");
//! ```

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope};
use serde::{Deserialize, Serialize};

use crate::error::FreightError;
use crate::manifest::types::Manifest;
use crate::toolchain::DetectedCompiler;
use super::compile::select_compiler;
use super::foreign::ResolvedPkgConfig;

// ── Public types ──────────────────────────────────────────────────────────────

pub const SCRIPT_NAME: &str = "build.freight";

/// Everything a `build.freight` script contributes to the build.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScriptOutput {
    /// Key + optional value pairs; rendered as `KEY` or `KEY=VALUE` (no `-D`).
    pub defines:       Vec<(String, Option<String>)>,
    /// Extra include directories.
    pub include_dirs:  Vec<PathBuf>,
    /// Raw compiler flags appended after the assembled flag set.
    pub extra_flags:   Vec<String>,
    /// System libraries to link (`-l{name}`).
    pub link_libs:     Vec<String>,
    /// Raw linker flags.
    pub link_flags:    Vec<String>,
    /// Environment variable overrides (applied to `run()` and compiler invocations).
    pub env_overrides: Vec<(String, String)>,
    /// Dynamically added source files (generated code, protoc output, etc.).
    /// The build engine compiles these alongside the project's own sources.
    pub extra_sources: Vec<PathBuf>,
    /// Non-fatal warnings emitted by `warning(msg)`.
    pub warnings:      Vec<String>,
}

impl ScriptOutput {
    /// Render `defines` as define strings WITHOUT the `-D` prefix.
    pub fn to_defines(&self) -> Vec<String> {
        self.defines.iter().map(|(k, v)| match v {
            Some(val) => format!("{k}={val}"),
            None      => k.clone(),
        }).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.extra_flags.is_empty()
            && self.link_libs.is_empty()
            && self.link_flags.is_empty()
            && self.env_overrides.is_empty()
            && self.extra_sources.is_empty()
            && self.warnings.is_empty()
    }
}

/// Absolute path of the `target/{profile}/build/` directory used by scripts.
pub fn out_dir(project_dir: &Path, profile: &str) -> PathBuf {
    project_dir.join("target").join(profile).join("build")
}

// ── Map types exposed to scripts ──────────────────────────────────────────────

#[derive(Clone)]
struct RhaiEnv;

/// Read-only toolchain info available as `toolchain["key"]` in scripts.
#[derive(Clone)]
struct RhaiToolchain {
    backend: String,
    version: String,   // "" when unknown
    target:  String,   // "" for native builds
}

// ── Thread-local accumulator ──────────────────────────────────────────────────

struct ScriptState {
    output:     ScriptOutput,
    rerun_deps: Vec<PathBuf>,
}

thread_local! {
    static STATE: RefCell<Option<ScriptState>> = RefCell::new(None);
}

fn with_state<F: FnOnce(&mut ScriptState)>(f: F) {
    STATE.with(|c| {
        if let Some(s) = c.borrow_mut().as_mut() { f(s); }
    });
}

// ── Stamp file — rerun_if caching ─────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct StampEntry {
    path:           PathBuf,
    modified_secs:  u64,
    modified_nanos: u32,
}

#[derive(Serialize, Deserialize)]
struct ScriptStamp {
    deps:   Vec<StampEntry>,
    output: ScriptOutput,
}

fn stamp_path(project_dir: &Path, profile: &str) -> PathBuf {
    out_dir(project_dir, profile).join(".script-stamp.json")
}

fn file_mtime(path: &Path) -> Option<(u64, u32)> {
    let mt = std::fs::metadata(path).ok()?.modified().ok()?;
    let dur = mt.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some((dur.as_secs(), dur.subsec_nanos()))
}

/// If a stamp exists and every listed dep (including `build.freight` itself) is
/// unchanged, return the cached `ScriptOutput`. Otherwise return `None`.
fn load_stamp_if_current(project_dir: &Path, profile: &str, script_path: &Path) -> Option<ScriptOutput> {
    let bytes = std::fs::read(stamp_path(project_dir, profile)).ok()?;
    let stamp: ScriptStamp = serde_json::from_slice(&bytes).ok()?;
    // No deps → script never called rerun_if → always re-run.
    if stamp.deps.is_empty() { return None; }
    for entry in &stamp.deps {
        let (secs, nanos) = file_mtime(&entry.path)?;
        if secs != entry.modified_secs || nanos != entry.modified_nanos {
            return None;
        }
    }
    // Also check the script file itself for changes.
    if let Some((secs, nanos)) = file_mtime(script_path) {
        let script_entry = stamp.deps.iter().find(|e| e.path == script_path);
        if let Some(e) = script_entry {
            if secs != e.modified_secs || nanos != e.modified_nanos {
                return None;
            }
        }
    }
    Some(stamp.output)
}

fn save_stamp(project_dir: &Path, profile: &str, script_path: &Path, deps: &[PathBuf], output: &ScriptOutput) {
    // Always include build.freight in the dep list.
    let mut all: Vec<&Path> = deps.iter().map(PathBuf::as_path).collect();
    if !all.contains(&script_path) {
        all.push(script_path);
    }
    let entries: Vec<StampEntry> = all.iter()
        .filter_map(|p| {
            let (secs, nanos) = file_mtime(p)?;
            Some(StampEntry { path: p.to_path_buf(), modified_secs: secs, modified_nanos: nanos })
        })
        .collect();
    let stamp = ScriptStamp { deps: entries, output: output.clone() };
    if let Ok(json) = serde_json::to_string_pretty(&stamp) {
        let _ = std::fs::write(stamp_path(project_dir, profile), json);
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Evaluate `build.freight` in `project_dir` and return what it contributed.
///
/// `detected` is used to expose `toolchain["version"]` to the script.
/// `pkg_configs` is the resolved list of pkg-config deps, exposed as the
/// read-only `packages` map (e.g. `packages["zlib"].found`, `.version`).
/// Returns an empty [`ScriptOutput`] (with `out_dir` already in `include_dirs`)
/// when no script is present.
pub fn run_build_script(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
    detected: &[DetectedCompiler],
    pkg_configs: &[ResolvedPkgConfig],
) -> Result<ScriptOutput, FreightError> {
    let script_path = project_dir.join(SCRIPT_NAME);
    if !script_path.is_file() {
        return Ok(ScriptOutput::default());
    }

    let out_d = out_dir(project_dir, profile);
    std::fs::create_dir_all(&out_d)?;

    // Skip re-execution if all rerun_if deps are unchanged.
    if let Some(mut cached) = load_stamp_if_current(project_dir, profile, &script_path) {
        use owo_colors::OwoColorize;
        println!("  {} {SCRIPT_NAME} (cached)", "Running".bold().cyan());
        if !cached.include_dirs.contains(&out_d) {
            cached.include_dirs.insert(0, out_d);
        }
        print_warnings(&cached.warnings);
        return Ok(cached);
    }

    use owo_colors::OwoColorize;
    println!("  {} {SCRIPT_NAME}", "Running".bold().cyan());

    let src = std::fs::read_to_string(&script_path)?;

    STATE.with(|c| *c.borrow_mut() = Some(ScriptState {
        output:     ScriptOutput::default(),
        rerun_deps: Vec::new(),
    }));

    // ── Toolchain info ────────────────────────────────────────────────────────

    // Pick the first detected compiler that matches the configured backend to
    // expose its version string.
    let primary_lang = manifest.language.keys().next().map(|s| s.as_str()).unwrap_or("cpp");
    let compiler_version = select_compiler(primary_lang, &manifest.compiler.backend, detected)
        .map(|c| c.version.clone())
        .unwrap_or_default();
    let compiler_target = manifest.compiler.target.as_deref().unwrap_or("").to_string();

    let mut engine = Engine::new();

    // ── Map types ─────────────────────────────────────────────────────────────

    engine.register_type_with_name::<RhaiEnv>("Env");
    engine.register_indexer_get(|_: &mut RhaiEnv, key: ImmutableString| -> String {
        std::env::var(key.as_str()).unwrap_or_default()
    });
    engine.register_indexer_set(|_: &mut RhaiEnv, key: ImmutableString, val: String| {
        with_state(|s| s.output.env_overrides.push((key.to_string(), val)));
    });

    engine.register_type_with_name::<RhaiToolchain>("Toolchain");
    engine.register_indexer_get(|t: &mut RhaiToolchain, key: ImmutableString| -> Dynamic {
        match key.as_str() {
            "backend" => Dynamic::from(t.backend.clone()),
            "arch"    => Dynamic::from(std::env::consts::ARCH.to_string()),
            "os"      => Dynamic::from(std::env::consts::OS.to_string()),
            "version" => Dynamic::from(t.version.clone()),
            "target"  => Dynamic::from(t.target.clone()),
            _         => Dynamic::UNIT,
        }
    });

    // ── Read-only context functions ───────────────────────────────────────────

    let s = manifest.package.name.clone();
    engine.register_fn("package_name", move || s.clone());

    let s = manifest.package.version.clone();
    engine.register_fn("package_version", move || s.clone());

    let s = profile.to_string();
    engine.register_fn("profile", move || s.clone());

    let s = out_d.to_string_lossy().into_owned();
    engine.register_fn("out_dir", move || s.clone());

    let s = project_dir.to_string_lossy().into_owned();
    engine.register_fn("source_dir", move || s.clone());

    // ── Output setters ────────────────────────────────────────────────────────

    engine.register_fn("define", |k: String| {
        with_state(|s| s.output.defines.push((k, None)));
    });
    engine.register_fn("define_value", |k: String, v: String| {
        with_state(|s| s.output.defines.push((k, Some(v))));
    });
    engine.register_fn("add_include", |p: String| {
        with_state(|s| s.output.include_dirs.push(PathBuf::from(p)));
    });
    engine.register_fn("add_flag", |f: String| {
        with_state(|s| s.output.extra_flags.push(f));
    });
    engine.register_fn("add_link_lib", |n: String| {
        with_state(|s| s.output.link_libs.push(n));
    });
    engine.register_fn("add_link_flag", |f: String| {
        with_state(|s| s.output.link_flags.push(f));
    });
    engine.register_fn("link_path", |dir: String| {
        with_state(|s| s.output.link_flags.push(format!("-L{dir}")));
    });
    engine.register_fn("add_source", |path: String| {
        with_state(|s| s.output.extra_sources.push(PathBuf::from(path)));
    });
    engine.register_fn("warning", |msg: String| {
        use owo_colors::OwoColorize;
        eprintln!("  {} {msg}", "warning:".yellow().bold());
        with_state(|s| s.output.warnings.push(msg));
    });

    // ── Incremental re-execution ──────────────────────────────────────────────

    engine.register_fn("rerun_if", |path: String| {
        with_state(|s| s.rerun_deps.push(PathBuf::from(path)));
    });

    // ── File generation ───────────────────────────────────────────────────────

    engine.register_fn("write_file",
        |path: String, content: String| -> Result<(), Box<rhai::EvalAltResult>> {
            let p = PathBuf::from(&path);
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
            }
            if std::fs::read_to_string(&p).ok().as_deref() == Some(content.as_str()) {
                return Ok(());
            }
            std::fs::write(&p, content).map_err(|e| e.to_string())?;
            Ok(())
        },
    );

    engine.register_fn("read_file", |path: String| -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    });

    engine.register_fn("path_exists", |path: String| -> bool {
        Path::new(&path).exists()
    });

    // ── pkg-config integration ────────────────────────────────────────────────

    // pkg_config_cflags("openssl") → "-I/usr/include/openssl …" or ""
    engine.register_fn("pkg_config_cflags", |name: String| -> String {
        pkg_config_query(&name, "--cflags").unwrap_or_default()
    });

    // pkg_config_libs("openssl") → "-lssl -lcrypto …" or ""
    engine.register_fn("pkg_config_libs", |name: String| -> String {
        pkg_config_query(&name, "--libs").unwrap_or_default()
    });

    // pkg_config_apply("openssl") — runs both queries and applies the results:
    //   cflags: -I → add_include, other → add_flag
    //   libs:   -l → add_link_lib, -L → link_flag, other → add_link_flag
    engine.register_fn("pkg_config_apply", |name: String| {
        let cflags = pkg_config_query(&name, "--cflags").unwrap_or_default();
        let libs   = pkg_config_query(&name, "--libs").unwrap_or_default();
        apply_pkg_config_output(&cflags, &libs);
    });

    // ── Environment probing ───────────────────────────────────────────────────

    engine.register_fn("find_tool", |name: String| -> Dynamic {
        which(&name)
            .map(|p| Dynamic::from(p.to_string_lossy().into_owned()))
            .unwrap_or(Dynamic::UNIT)
    });

    engine.register_fn("pkg_config_exists", |name: String| -> bool {
        std::process::Command::new("pkg-config")
            .args(["--exists", name.as_str()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });

    engine.register_fn("run", |cmd: String, args: Array| -> Map {
        let args: Vec<_> = args.into_iter()
            .filter_map(|v| v.try_cast::<String>())
            .collect();
        let mut command = std::process::Command::new(&cmd);
        command.args(&args);
        STATE.with(|c| {
            if let Some(s) = c.borrow().as_ref() {
                for (k, v) in &s.output.env_overrides {
                    command.env(k, v);
                }
            }
        });
        let result = command.output();
        let mut m = Map::new();
        match result {
            Ok(o) => {
                m.insert("ok".into(),     Dynamic::from(o.status.success()));
                m.insert("status".into(), Dynamic::from(o.status.code().unwrap_or(-1) as i64));
                m.insert("stdout".into(), Dynamic::from(String::from_utf8_lossy(&o.stdout).trim().to_owned()));
                m.insert("stderr".into(), Dynamic::from(String::from_utf8_lossy(&o.stderr).trim().to_owned()));
            }
            Err(e) => {
                m.insert("ok".into(),     Dynamic::from(false));
                m.insert("status".into(), Dynamic::from(-1_i64));
                m.insert("stdout".into(), Dynamic::from(String::new()));
                m.insert("stderr".into(), Dynamic::from(e.to_string()));
            }
        }
        m
    });

    engine.register_fn("fail",
        |msg: String| -> Result<(), Box<rhai::EvalAltResult>> { Err(msg.into()) },
    );

    // ── File discovery ────────────────────────────────────────────────────────

    // glob("proto/**/*.proto") → sorted array of matching paths relative to project root
    let proj_dir_g = project_dir.to_path_buf();
    engine.register_fn("glob", move |pattern: String| -> Array {
        glob_files(&proj_dir_g, &pattern)
            .into_iter()
            .map(|p| Dynamic::from(p.to_string_lossy().into_owned()))
            .collect()
    });

    // changed_files("proto/**/*.proto") → files newer than the last build stamp.
    // Returns all matching files on the first build (no stamp yet).
    let proj_dir_c  = project_dir.to_path_buf();
    let stamp_ref   = stamp_path(project_dir, profile);
    engine.register_fn("changed_files", move |pattern: String| -> Array {
        let ref_time = std::fs::metadata(&stamp_ref)
            .and_then(|m| m.modified())
            .ok();
        glob_files(&proj_dir_c, &pattern)
            .into_iter()
            .filter(|p| {
                let Some(ref_t) = ref_time else { return true }; // no stamp → all changed
                std::fs::metadata(proj_dir_c.join(p))
                    .and_then(|m| m.modified())
                    .map(|mt| mt > ref_t)
                    .unwrap_or(true)
            })
            .map(|p| Dynamic::from(p.to_string_lossy().into_owned()))
            .collect()
    });

    // ── Path helpers ──────────────────────────────────────────────────────────

    engine.register_fn("file_stem", |path: String| -> String {
        Path::new(&path).file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    });
    engine.register_fn("file_name", |path: String| -> String {
        Path::new(&path).file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    });
    engine.register_fn("file_dir", |path: String| -> String {
        Path::new(&path).parent()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_string()
    });

    // ── Scope ─────────────────────────────────────────────────────────────────

    let mut scope = Scope::new();
    scope.push("env", RhaiEnv);
    scope.push("toolchain", RhaiToolchain {
        backend: manifest.compiler.backend.0.clone(),
        version: compiler_version,
        target:  compiler_target,
    });

    // packages["zlib"].found / .version — read-only view of resolved pkg-config deps.
    let mut packages_map = Map::new();
    for pc in pkg_configs {
        let mut entry = Map::new();
        entry.insert("found".into(),   Dynamic::from(pc.found));
        entry.insert("version".into(), Dynamic::from(pc.version.clone()));
        packages_map.insert(pc.name.clone().into(), Dynamic::from_map(entry));
    }
    scope.push_constant("packages", packages_map);

    // ── Evaluate ──────────────────────────────────────────────────────────────

    engine.run_with_scope(&mut scope, &src).map_err(|e| {
        FreightError::BuildScriptFailed(script_path.display().to_string(), e.to_string())
    })?;

    let state = STATE.with(|c| c.borrow_mut().take().unwrap_or_else(|| ScriptState {
        output: ScriptOutput::default(), rerun_deps: vec![],
    }));

    let mut output  = state.output;
    let rerun_deps  = state.rerun_deps;

    // out_dir is always on the include path so generated headers are found.
    let out_d = out_dir(project_dir, profile);
    if !output.include_dirs.contains(&out_d) {
        output.include_dirs.insert(0, out_d);
    }

    // Persist stamp when the script declared at least one rerun_if dep.
    if !rerun_deps.is_empty() {
        save_stamp(project_dir, profile, &script_path, &rerun_deps, &output);
    }

    Ok(output)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let p = dir.join(name);
        if p.is_file() { return Some(p); }
    }
    None
}

/// Run `pkg-config <flag> <name>` and return trimmed output, or `None` on failure.
fn pkg_config_query(name: &str, flag: &str) -> Option<String> {
    let out = std::process::Command::new("pkg-config")
        .args([flag, name])
        .output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Parse `pkg-config --cflags` + `--libs` output and accumulate into the
/// thread-local script state.
fn apply_pkg_config_output(cflags: &str, libs: &str) {
    for token in cflags.split_whitespace() {
        if let Some(path) = token.strip_prefix("-I") {
            with_state(|s| s.output.include_dirs.push(PathBuf::from(path)));
        } else if let Some(rest) = token.strip_prefix("-D") {
            if let Some((k, v)) = rest.split_once('=') {
                let (k, v) = (k.to_string(), v.to_string());
                with_state(|s| s.output.defines.push((k, Some(v))));
            } else {
                let k = rest.to_string();
                with_state(|s| s.output.defines.push((k, None)));
            }
        } else if !token.is_empty() {
            let t = token.to_string();
            with_state(|s| s.output.extra_flags.push(t));
        }
    }
    for token in libs.split_whitespace() {
        if let Some(name) = token.strip_prefix("-l") {
            let n = name.to_string();
            with_state(|s| s.output.link_libs.push(n));
        } else if let Some(dir) = token.strip_prefix("-L") {
            let f = format!("-L{dir}");
            with_state(|s| s.output.link_flags.push(f));
        } else if !token.is_empty() {
            let f = token.to_string();
            with_state(|s| s.output.link_flags.push(f));
        }
    }
}

// ── File discovery (glob + changed_files) ────────────────────────────────────

/// Convert a glob pattern to a `regex::Regex` that matches relative paths.
///
/// Supported wildcards:
///   `**`  — any sequence of characters including `/`
///   `*`   — any sequence of characters except `/`
///   `?`   — any single character except `/`
fn glob_to_regex(pattern: &str) -> regex::Regex {
    let mut re = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                re.push_str(".*");
                // consume optional trailing slash after **
                if chars.peek() == Some(&'/') { chars.next(); }
            }
            '*'  => re.push_str("[^/]*"),
            '?'  => re.push_str("[^/]"),
            '.'  => re.push_str("\\."),
            '+'  | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                re.push('\\'); re.push(c);
            }
            _ => re.push(c),
        }
    }
    re.push('$');
    regex::Regex::new(&re).unwrap_or_else(|_| regex::Regex::new("^$").unwrap())
}

/// Walk `project_dir` and return all files whose path relative to
/// `project_dir` matches `pattern`.  Results are sorted for stability.
fn glob_files(project_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    // Narrow the walk to the subtree before the first wildcard.
    let base_prefix = pattern.split(['*', '?']).next().unwrap_or("");
    let base_dir = if base_prefix.is_empty() {
        project_dir.to_path_buf()
    } else {
        let p = Path::new(base_prefix.trim_end_matches('/'));
        project_dir.join(p.components().take_while(|c| {
            !matches!(c, std::path::Component::Normal(_) if {
                let s = c.as_os_str().to_string_lossy();
                s.contains('*') || s.contains('?')
            })
        }).collect::<PathBuf>())
    };

    let re = glob_to_regex(pattern);
    let mut results = Vec::new();

    let walker = walkdir::WalkDir::new(&base_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file());

    for entry in walker {
        let rel = entry.path()
            .strip_prefix(project_dir)
            .unwrap_or(entry.path());
        // Normalise to forward slashes for cross-platform pattern matching.
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if re.is_match(&rel_str) {
            results.push(rel.to_path_buf());
        }
    }

    results.sort();
    results
}

fn print_warnings(warnings: &[String]) {
    use owo_colors::OwoColorize;
    for w in warnings {
        eprintln!("  {} {w}", "warning:".yellow().bold());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── glob_to_regex ─────────────────────────────────────────────────────────

    #[test]
    fn glob_star_matches_within_dir() {
        let re = glob_to_regex("proto/*.proto");
        assert!( re.is_match("proto/messages.proto"));
        assert!( re.is_match("proto/service.proto"));
        assert!(!re.is_match("proto/sub/messages.proto")); // * doesn't cross /
        assert!(!re.is_match("other/messages.proto"));
    }

    #[test]
    fn glob_double_star_crosses_dirs() {
        let re = glob_to_regex("src/**/*.cpp");
        assert!( re.is_match("src/main.cpp"));
        assert!( re.is_match("src/foo/bar.cpp"));
        assert!( re.is_match("src/a/b/c/deep.cpp"));
        assert!(!re.is_match("src/main.c"));
        assert!(!re.is_match("other/main.cpp"));
    }

    #[test]
    fn glob_question_mark() {
        let re = glob_to_regex("src/?.c");
        assert!( re.is_match("src/a.c"));
        assert!(!re.is_match("src/ab.c"));
        assert!(!re.is_match("src/a/b.c"));
    }

    #[test]
    fn glob_literal_dot_not_any_char() {
        let re = glob_to_regex("*.proto");
        assert!( re.is_match("messages.proto"));
        assert!(!re.is_match("messagesXproto")); // dot must be literal
    }

    // ── glob_files ────────────────────────────────────────────────────────────

    #[test]
    fn glob_files_finds_matching() {
        let dir = tempfile::tempdir().unwrap();
        let proto_dir = dir.path().join("proto");
        std::fs::create_dir_all(&proto_dir).unwrap();
        std::fs::write(proto_dir.join("a.proto"), "").unwrap();
        std::fs::write(proto_dir.join("b.proto"), "").unwrap();
        std::fs::write(proto_dir.join("c.txt"),   "").unwrap();

        let files = glob_files(dir.path(), "proto/*.proto");
        let names: Vec<_> = files.iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, &["a.proto", "b.proto"]);
    }

    #[test]
    fn glob_files_double_star_recurses() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("src").join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.path().join("src").join("root.c"), "").unwrap();
        std::fs::write(sub.join("deep.c"), "").unwrap();
        std::fs::write(sub.join("skip.h"), "").unwrap();

        let files = glob_files(dir.path(), "src/**/*.c");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn glob_files_empty_when_no_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.cpp"), "").unwrap();
        let files = glob_files(dir.path(), "*.proto");
        assert!(files.is_empty());
    }
}
