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
//!
//! // ── env map (read / write) ───────────────────────────────────────────────
//! let cc = env["CC"];                   // "" when unset
//! env["PKG_CONFIG_PATH"] = "/opt/lib/pkgconfig"; // override for run() + compiler
//!
//! // ── Output setters ───────────────────────────────────────────────────────
//! set_define("VERSION", "1.2.3"); // → -DVERSION=1.2.3
//! add_define("NDEBUG");           // → -DNDEBUG
//! add_include(out);               // extra -I path
//! add_flag("-march=native");      // raw compiler flag
//! add_link_lib("z");              // → -lz
//! add_link_flag("-L/opt/local/lib");
//!
//! // ── File generation ──────────────────────────────────────────────────────
//! // write_file is a no-op when content unchanged → avoids spurious rebuilds.
//! // String values belong here; use set_define/add_define for booleans/numbers.
//! write_file(out + "/version.h",
//!     "#pragma once\n#define VERSION \"" + package_version() + "\"\n");
//!
//! // ── Environment probing ──────────────────────────────────────────────────
//! let git = run("git", ["rev-parse", "--short", "HEAD"]);
//! if git.ok { /* git.stdout, git.stderr, git.status */ }
//!
//! let has_ssl = pkg_config_exists("openssl");
//! if !has_ssl { fail("openssl not found — install libssl-dev"); }
//!
//! let cmake = find_tool("cmake"); // full path or ()
//!
//! // Hint incremental builds (not yet used for skipping, reserved for v2)
//! rerun_if("version.txt");
//! ```

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope};

use crate::error::FreightError;
use crate::manifest::types::Manifest;

// ── Public types ──────────────────────────────────────────────────────────────

pub const SCRIPT_NAME: &str = "build.freight";

/// Everything a `build.freight` script contributes to the build.
#[derive(Debug, Default)]
pub struct ScriptOutput {
    /// Key + optional value pairs; rendered as `KEY` or `KEY=VALUE` (no `-D`).
    pub defines:      Vec<(String, Option<String>)>,
    /// Extra include directories (in addition to those from the manifest).
    /// `out_dir()` is always prepended automatically.
    pub include_dirs: Vec<PathBuf>,
    /// Raw compiler flags appended after the assembled flag set.
    pub extra_flags:  Vec<String>,
    /// System libraries to link (`-l{name}` on GCC/Clang).
    pub link_libs:    Vec<String>,
    /// Raw linker flags.
    pub link_flags:   Vec<String>,
    /// Environment variable overrides set via `env["KEY"] = "value"`.
    /// Applied to `run()` child processes and to compiler invocations.
    pub env_overrides: Vec<(String, String)>,
}

impl ScriptOutput {
    /// Render `defines` as define strings (WITHOUT `-D` prefix) for `BuildSettings.defines`.
    /// The `-D` prefix is added by `assemble_flags` via the compiler template.
    pub fn to_defines(&self) -> Vec<String> {
        self.defines.iter().map(|(k, v)| match v {
            Some(val) => format!("{k}={val}"),
            None      => k.clone(),
        }).collect()
    }

    /// `true` when the script produced no output at all.
    pub fn is_empty(&self) -> bool {
        self.defines.is_empty()
            && self.include_dirs.is_empty()
            && self.extra_flags.is_empty()
            && self.link_libs.is_empty()
            && self.link_flags.is_empty()
            && self.env_overrides.is_empty()
    }
}

/// Absolute path of the `target/{profile}/build/` directory used by scripts.
pub fn out_dir(project_dir: &Path, profile: &str) -> PathBuf {
    project_dir.join("target").join(profile).join("build")
}

// ── Map types exposed to scripts ──────────────────────────────────────────────

/// `env` — read/write access to environment variables.
///
/// - `env["KEY"]`         → current value of `KEY` (`""` when unset)
/// - `env["KEY"] = "val"` → stored in [`ScriptOutput::env_overrides`]; applied
///   to every subsequent `run()` call and to compiler invocations.
#[derive(Clone)]
struct RhaiEnv;

/// `toolchain` — read-only info about the active compiler configuration.
///
/// - `toolchain["backend"]` — compiler backend name (`"gcc"`, `"clang"`, `"auto"`, …)
/// - `toolchain["arch"]`    — host CPU architecture (`"x86_64"`, `"aarch64"`, …)
/// - `toolchain["os"]`      — host OS (`"linux"`, `"macos"`, `"windows"`, …)
#[derive(Clone)]
struct RhaiToolchain {
    backend: String,
}

// ── Thread-local accumulator ──────────────────────────────────────────────────

thread_local! {
    static STATE: RefCell<Option<ScriptOutput>> = RefCell::new(None);
}

fn with_state<F: FnOnce(&mut ScriptOutput)>(f: F) {
    STATE.with(|c| {
        if let Some(s) = c.borrow_mut().as_mut() { f(s); }
    });
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Evaluate `build.freight` in `project_dir` and return what it contributed.
///
/// Returns an empty [`ScriptOutput`] (with `out_dir` already in `include_dirs`)
/// when no script is present.
pub fn run_build_script(
    project_dir: &Path,
    manifest: &Manifest,
    profile: &str,
) -> Result<ScriptOutput, FreightError> {
    let script_path = project_dir.join(SCRIPT_NAME);
    if !script_path.is_file() {
        return Ok(ScriptOutput::default());
    }

    use owo_colors::OwoColorize;
    println!("  {} {SCRIPT_NAME}", "Running".bold().cyan());

    let src = std::fs::read_to_string(&script_path)?;
    let out_d = out_dir(project_dir, profile);
    std::fs::create_dir_all(&out_d)?;

    STATE.with(|c| *c.borrow_mut() = Some(ScriptOutput::default()));

    let mut engine = Engine::new();

    // ── Map types ─────────────────────────────────────────────────────────────

    engine.register_type_with_name::<RhaiEnv>("Env");
    engine.register_indexer_get(|_: &mut RhaiEnv, key: ImmutableString| -> String {
        std::env::var(key.as_str()).unwrap_or_default()
    });
    engine.register_indexer_set(|_: &mut RhaiEnv, key: ImmutableString, val: String| {
        with_state(|s| s.env_overrides.push((key.to_string(), val)));
    });

    engine.register_type_with_name::<RhaiToolchain>("Toolchain");
    engine.register_indexer_get(|t: &mut RhaiToolchain, key: ImmutableString| -> Dynamic {
        match key.as_str() {
            "backend" => Dynamic::from(t.backend.clone()),
            "arch"    => Dynamic::from(std::env::consts::ARCH.to_string()),
            "os"      => Dynamic::from(std::env::consts::OS.to_string()),
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

    engine.register_fn("set_define", |k: String, v: String| {
        with_state(|s| s.defines.push((k, Some(v))));
    });
    engine.register_fn("add_define", |k: String| {
        with_state(|s| s.defines.push((k, None)));
    });
    engine.register_fn("add_include", |p: String| {
        with_state(|s| s.include_dirs.push(PathBuf::from(p)));
    });
    engine.register_fn("add_flag", |f: String| {
        with_state(|s| s.extra_flags.push(f));
    });
    engine.register_fn("add_link_lib", |n: String| {
        with_state(|s| s.link_libs.push(n));
    });
    engine.register_fn("add_link_flag", |f: String| {
        with_state(|s| s.link_flags.push(f));
    });

    // Accepted for forward-compatibility; incremental skip logic is v2.
    engine.register_fn("rerun_if", |_path: String| {});

    // ── File generation ───────────────────────────────────────────────────────

    engine.register_fn("write_file",
        |path: String, content: String| -> Result<(), Box<rhai::EvalAltResult>> {
            let p = PathBuf::from(&path);
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
            }
            // Only write when content changed — avoids invalidating dependents.
            if std::fs::read_to_string(&p).ok().as_deref() == Some(content.as_str()) {
                return Ok(());
            }
            std::fs::write(&p, content).map_err(|e| e.to_string())?;
            Ok(())
        },
    );

    // ── Environment probing ───────────────────────────────────────────────────

    // find_tool("cmake") → "/usr/bin/cmake" | ()
    engine.register_fn("find_tool", |name: String| -> Dynamic {
        which(&name)
            .map(|p| Dynamic::from(p.to_string_lossy().into_owned()))
            .unwrap_or(Dynamic::UNIT)
    });

    // pkg_config_exists("openssl") → bool
    engine.register_fn("pkg_config_exists", |name: String| -> bool {
        std::process::Command::new("pkg-config")
            .args(["--exists", name.as_str()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });

    // run("git", ["rev-parse", "--short", "HEAD"]) → #{ok, status, stdout, stderr}
    // Applies env overrides that have been written to `env[...]` so far.
    engine.register_fn("run", |cmd: String, args: Array| -> Map {
        let args: Vec<_> = args.into_iter()
            .filter_map(|v| v.try_cast::<String>())
            .collect();
        let mut command = std::process::Command::new(&cmd);
        command.args(&args);
        STATE.with(|c| {
            if let Some(s) = c.borrow().as_ref() {
                for (k, v) in &s.env_overrides {
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

    // fail("openssl not found") — abort the build with a clear message
    engine.register_fn("fail",
        |msg: String| -> Result<(), Box<rhai::EvalAltResult>> { Err(msg.into()) },
    );

    // ── Scope ─────────────────────────────────────────────────────────────────

    let mut scope = Scope::new();
    scope.push("env", RhaiEnv);
    scope.push("toolchain", RhaiToolchain {
        backend: manifest.compiler.backend.0.clone(),
    });

    // ── Evaluate ──────────────────────────────────────────────────────────────

    engine.run_with_scope(&mut scope, &src).map_err(|e| {
        FreightError::BuildScriptFailed(script_path.display().to_string(), e.to_string())
    })?;

    let mut output = STATE.with(|c| c.borrow_mut().take().unwrap_or_default());

    // out_dir is always on the include path so generated headers are found.
    if !output.include_dirs.contains(&out_d) {
        output.include_dirs.insert(0, out_d);
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
