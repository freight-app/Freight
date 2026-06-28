//! Build plugins — versioned dependencies that run a Rhai script during a
//! consuming project's build.
//!
//! A package becomes a plugin by declaring `[plugin]` in its `freight.toml`
//! (`entry`, `handles`, `tools`). When a project depends on such a package and
//! declares one of the plugin's `handles` sections, that section's config is
//! handed to the plugin's script, which has in scope:
//!
//! - `CFG`                       — the section's config (e.g. `[proto]`) as data
//! - `OUT_DIR` / `SRC_DIR` / …   — project path constants (per-plugin output dir, …)
//! - `HOST` / `TARGET`           — host/target characteristics (`.os`, `.arch`, …)
//! - `glob(pattern)`         — match input files under the project
//! - `run(tool, [args])`     — run an **allow-listed** tool (codegen)
//! - `add_source(path)`      — compile a generated source
//! - `add_include_dir(path)` — expose a generated header dir
//! - `define(name[, value])` — inject a `-D` define
//! - Python-flavoured, project-confined filesystem helpers (`read_text`,
//!   `write_text`, `append_text`, `copy`, `makedirs`, `listdir`, `exists`,
//!   `is_file`, `is_dir`) and pure path helpers (`join`, `basename`, `dirname`,
//!   `stem`, `ext`)
//!
//! This is how protobuf/Qt/flatbuffers/shader codegen are expressed without
//! baking any of them into freight's core.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position, Scope};

use crate::build::discover::SourceFile;
use crate::error::FreightError;
use crate::event::{BuildEvent, Progress};
use crate::manifest::load_manifest_cached;
use crate::project::{source_package_dirs, PackageKind};

/// Generated artifacts contributed by a plugin run.
#[derive(Default, Debug)]
struct RawOutput {
    /// Absolute paths to generated source files to compile.
    sources: Vec<PathBuf>,
    /// Include directories to add to the compile include path.
    include_dirs: Vec<PathBuf>,
    /// `-D` defines (`NAME` or `NAME=value`).
    defines: Vec<String>,
    /// Flags targeted at a specific tool: `(tool_selector, flag)`. The selector
    /// is a compiler name/alias/family, the catch-all `"compiler"`, or a role
    /// keyword (`"linker"` / `"archiver"`). See [`ToolFlag`].
    tool_flags: Vec<ToolFlag>,
    /// Install prefixes (a foreign dep's `CMAKE_INSTALL_PREFIX`, …) this plugin
    /// produced. Threaded into plugins that run after it as `CFG.prefixes` so a
    /// later foreign dep can resolve an earlier one (`find_package`, pkg-config).
    prefixes: Vec<PathBuf>,
}

/// A flag a plugin directed at one build tool. `tool` is matched against the
/// invoked compiler (by template `name`, `alias`, or `family`), the catch-all
/// `"compiler"`, or a role keyword (`"linker"`, `"archiver"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFlag {
    pub tool: String,
    pub flag: String,
}

/// Aggregated, build-ready plugin output for the pipeline.
#[derive(Default)]
pub struct PluginBuildOutput {
    pub sources: Vec<SourceFile>,
    pub include_dirs: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub tool_flags: Vec<ToolFlag>,
    /// Install prefixes produced by foreign-build plugins (see [`RawOutput`]).
    pub prefixes: Vec<PathBuf>,
}

/// Flags a plugin aimed at a compiler whose template has this `name` / `alias` /
/// `family`. Matches the catch-all `"compiler"` selector too.
pub fn compiler_tool_flags(
    tool_flags: &[ToolFlag],
    name: &str,
    alias: Option<&str>,
    family: &str,
) -> Vec<String> {
    tool_flags
        .iter()
        .filter(|tf| {
            tf.tool == "compiler"
                || tf.tool == name
                || Some(tf.tool.as_str()) == alias
                || (!family.is_empty() && tf.tool == family)
        })
        .map(|tf| tf.flag.clone())
        .collect()
}

/// Flags a plugin aimed at a build role keyword (`"linker"`, `"archiver"`).
pub fn role_tool_flags(tool_flags: &[ToolFlag], role: &str) -> Vec<String> {
    tool_flags
        .iter()
        .filter(|tf| tf.tool == role)
        .map(|tf| tf.flag.clone())
        .collect()
}

// ── Shared state behind the global plugin functions ─────────────────────────

struct CtxState {
    project_dir: PathBuf,
    allowed_tools: Vec<String>,
    tool_paths: Vec<PathBuf>,
    out: RawOutput,
    /// Build progress sink, so a tool's output (and `print`) surface in the
    /// build output. `silent()` in the LSP, so it never touches the JSON-RPC stream.
    progress: Progress,
}

/// Shared mutable state the plugin's global functions read/write.
type State = Rc<RefCell<CtxState>>;

/// Emit a captured tool stream to the build progress sink, one event per
/// non-empty line. No-op for empty output.
fn emit_tool_output(progress: &Progress, tool: &str, bytes: &[u8], is_err: bool) {
    if bytes.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        progress(BuildEvent::ScriptOutput {
            source: tool.to_string(),
            text: line.to_string(),
            is_err,
        });
    }
}

/// Run an allow-listed tool, capturing its output into the build stream. `cwd`,
/// when given, sets a project-confined working directory; otherwise the project
/// root is used. A non-zero exit aborts the build (stderr folded into the error).
fn do_run(s: &State, tool: &str, args: Array, cwd: Option<&str>) -> Result<(), Box<EvalAltResult>> {
    let (root, resolved, progress) = {
        let st = s.borrow();
        if !st.allowed_tools.iter().any(|t| t == tool) {
            return Err(rhai_err(format!(
                "plugin tried to run disallowed tool '{tool}' — add it to `[plugin] tools`"
            )));
        }
        (
            st.project_dir.clone(),
            resolve_tool(tool, &st.tool_paths),
            st.progress.clone(),
        )
    };
    let dir = match cwd {
        Some(c) => contained(&root, c)?,
        None => root,
    };
    let arg_strings: Vec<String> = args.iter().map(|d| d.to_string()).collect();
    let output = Command::new(&resolved)
        .args(&arg_strings)
        .current_dir(&dir)
        .output()
        .map_err(|e| rhai_err(format!("failed to run '{tool}': {e}")))?;
    emit_tool_output(&progress, tool, &output.stdout, false);
    emit_tool_output(&progress, tool, &output.stderr, true);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(rhai_err(format!(
            "tool '{tool}' exited with status {}{suffix}",
            output.status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

fn rhai_err(msg: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(msg.into()),
        Position::NONE,
    ))
}

/// Resolve `tool` to an absolute path, preferring the build-dep `tool_paths`
/// (so a project-pinned tool wins), else leaving it for PATH resolution.
fn resolve_tool(tool: &str, tool_paths: &[PathBuf]) -> PathBuf {
    for dir in tool_paths {
        let candidate = dir.join(tool);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(tool)
}

/// Register the plugin API as **global functions** (no receiver object). Each
/// closes over a clone of the shared `state`, so a script just calls
/// `add_source(...)`, `run(...)`, `define(...)`, etc. directly.
fn register_fns(engine: &mut Engine, state: &State) {
    // Project directories are exposed as `SCREAMING_CASE` constants
    // (`OUT_DIR`, `SRC_DIR`, …) in `run_script`, not functions.

    let s = state.clone();
    engine.register_fn("glob", move |pattern: &str| -> Array {
        let root = s.borrow().project_dir.clone();
        let full = root.join(pattern);
        let mut out = Array::new();
        if let Ok(paths) = glob::glob(&full.to_string_lossy()) {
            for entry in paths.flatten() {
                // Confine matches to the project — a pattern like `/etc/*` or
                // `../../*` can't reach outside.
                if is_within(&root, &entry) {
                    out.push(Dynamic::from(entry.to_string_lossy().into_owned()));
                }
            }
        }
        out
    });

    let s = state.clone();
    engine.register_fn(
        "run",
        move |tool: &str, args: Array| -> Result<(), Box<EvalAltResult>> {
            do_run(&s, tool, args, None)
        },
    );
    // `run(tool, args, cwd)` — run with the working directory set to a
    // project-confined `cwd` (for build systems like autotools that must run
    // inside an out-of-source build dir).
    let s = state.clone();
    engine.register_fn(
        "run",
        move |tool: &str, args: Array, cwd: &str| -> Result<(), Box<EvalAltResult>> {
            do_run(&s, tool, args, Some(cwd))
        },
    );

    let s = state.clone();
    engine.register_fn(
        "capture",
        move |tool: &str, args: Array| -> Result<Map, Box<EvalAltResult>> {
            let (project_dir, resolved) = {
                let st = s.borrow();
                if !st.allowed_tools.iter().any(|t| t == tool) {
                    return Err(rhai_err(format!(
                        "plugin tried to run disallowed tool '{tool}' — add it to `[plugin] tools`"
                    )));
                }
                (st.project_dir.clone(), resolve_tool(tool, &st.tool_paths))
            };
            let arg_strings: Vec<String> = args.iter().map(|d| d.to_string()).collect();
            // Unlike `run`, a non-zero exit is **not** fatal — the script gets the
            // code and output back and decides what to do.
            let output = Command::new(&resolved)
                .args(&arg_strings)
                .current_dir(&project_dir)
                .output()
                .map_err(|e| rhai_err(format!("failed to run '{tool}': {e}")))?;
            let mut m = Map::new();
            m.insert(
                "code".into(),
                Dynamic::from(output.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".into(),
                Dynamic::from(String::from_utf8_lossy(&output.stdout).into_owned()),
            );
            m.insert(
                "stderr".into(),
                Dynamic::from(String::from_utf8_lossy(&output.stderr).into_owned()),
            );
            Ok(m)
        },
    );

    let s = state.clone();
    engine.register_fn(
        "add_source",
        move |path: &str| -> Result<(), Box<EvalAltResult>> {
            let root = s.borrow().project_dir.clone();
            let abs = contained(&root, path)?;
            s.borrow_mut().out.sources.push(abs);
            Ok(())
        },
    );
    let s = state.clone();
    engine.register_fn(
        "add_sources",
        move |paths: Array| -> Result<(), Box<EvalAltResult>> {
            let root = s.borrow().project_dir.clone();
            for p in paths.iter() {
                let abs = contained(&root, &p.to_string())?;
                s.borrow_mut().out.sources.push(abs);
            }
            Ok(())
        },
    );
    let s = state.clone();
    engine.register_fn(
        "add_include_dir",
        move |path: &str| -> Result<(), Box<EvalAltResult>> {
            let root = s.borrow().project_dir.clone();
            let abs = contained(&root, path)?;
            s.borrow_mut().out.include_dirs.push(abs);
            Ok(())
        },
    );
    let s = state.clone();
    engine.register_fn("define", move |name: &str| {
        s.borrow_mut().out.defines.push(name.to_string());
    });
    let s = state.clone();
    engine.register_fn("define", move |name: &str, value: &str| {
        s.borrow_mut().out.defines.push(format!("{name}={value}"));
    });
    let s = state.clone();
    engine.register_fn("add_flag", move |tool: &str, flag: &str| {
        s.borrow_mut().out.tool_flags.push(ToolFlag {
            tool: tool.to_string(),
            flag: flag.to_string(),
        });
    });
    // Link a library: a bare name → `-l<name>`, a path/archive → passed straight
    // to the linker. Sugar over a `linker` tool-flag (already wired into linking).
    let s = state.clone();
    engine.register_fn("link_lib", move |lib: &str| {
        let is_path = lib.contains('/')
            || [".a", ".so", ".lib", ".dylib"]
                .iter()
                .any(|e| lib.ends_with(e));
        let flag = if is_path {
            lib.to_string()
        } else {
            format!("-l{lib}")
        };
        s.borrow_mut().out.tool_flags.push(ToolFlag {
            tool: "linker".into(),
            flag,
        });
    });
    // Add a library search directory (`-L<path>`).
    let s = state.clone();
    engine.register_fn("link_dir", move |path: &str| {
        s.borrow_mut().out.tool_flags.push(ToolFlag {
            tool: "linker".into(),
            flag: format!("-L{path}"),
        });
    });
    // Register an install prefix this plugin produced (e.g. a foreign dep's
    // `CMAKE_INSTALL_PREFIX`). Threaded to later plugins as `CFG.prefixes` so a
    // dep built afterwards can resolve this one via find_package / pkg-config.
    let s = state.clone();
    engine.register_fn(
        "add_prefix",
        move |path: &str| -> Result<(), Box<EvalAltResult>> {
            let root = s.borrow().project_dir.clone();
            let abs = contained(&root, path)?;
            s.borrow_mut().out.prefixes.push(abs);
            Ok(())
        },
    );

    register_io_fns(engine, state);
    register_path_fns(engine);
    register_regex_fns(engine);
}

/// Python `re`-flavoured regex helpers for pulling errors / filenames / versions
/// out of tool output. Pattern first, like Python; an invalid pattern raises.
fn register_regex_fns(engine: &mut Engine) {
    fn compile(pattern: &str) -> Result<regex::Regex, Box<EvalAltResult>> {
        regex::Regex::new(pattern).map_err(|e| rhai_err(format!("bad regex `{pattern}`: {e}")))
    }
    // Group `g` as a string (whole match = 0); missing/optional groups → "".
    fn group_strings(caps: &regex::Captures) -> Array {
        caps.iter()
            .map(|g| Dynamic::from(g.map(|m| m.as_str().to_string()).unwrap_or_default()))
            .collect()
    }

    // True if `pattern` matches anywhere in `text`.
    engine.register_fn(
        "re_test",
        |pattern: &str, text: &str| -> Result<bool, Box<EvalAltResult>> {
            Ok(compile(pattern)?.is_match(text))
        },
    );
    // First match as `[whole, group1, group2, …]`; empty array when no match.
    engine.register_fn(
        "re_find",
        |pattern: &str, text: &str| -> Result<Array, Box<EvalAltResult>> {
            Ok(compile(pattern)?
                .captures(text)
                .map(|c| group_strings(&c))
                .unwrap_or_default())
        },
    );
    // Every match as an array of group-arrays.
    engine.register_fn(
        "re_find_all",
        |pattern: &str, text: &str| -> Result<Array, Box<EvalAltResult>> {
            let re = compile(pattern)?;
            Ok(re
                .captures_iter(text)
                .map(|c| Dynamic::from_array(group_strings(&c)))
                .collect())
        },
    );
    // Replace all matches (supports `$1` / `${name}` in `repl`).
    engine.register_fn(
        "re_replace",
        |pattern: &str, text: &str, repl: &str| -> Result<String, Box<EvalAltResult>> {
            Ok(compile(pattern)?.replace_all(text, repl).into_owned())
        },
    );
}

/// Python-flavoured, sandboxed filesystem helpers. Every path argument is
/// resolved against the project root and **must stay inside it** (same
/// confinement as `glob` / `add_source`); writes create missing parent dirs so
/// scripts don't have to. Reads of a missing file raise (like Python).
fn register_io_fns(engine: &mut Engine, state: &State) {
    let s = state.clone();
    engine.register_fn(
        "read_text",
        move |path: &str| -> Result<String, Box<EvalAltResult>> {
            let abs = contained(&s.borrow().project_dir.clone(), path)?;
            std::fs::read_to_string(&abs).map_err(|e| rhai_err(format!("read_text('{path}'): {e}")))
        },
    );

    let s = state.clone();
    engine.register_fn(
        "write_text",
        move |path: &str, content: &str| -> Result<(), Box<EvalAltResult>> {
            let abs = contained(&s.borrow().project_dir.clone(), path)?;
            create_parents(&abs)?;
            std::fs::write(&abs, content)
                .map_err(|e| rhai_err(format!("write_text('{path}'): {e}")))
        },
    );

    let s = state.clone();
    engine.register_fn(
        "append_text",
        move |path: &str, content: &str| -> Result<(), Box<EvalAltResult>> {
            use std::io::Write;
            let abs = contained(&s.borrow().project_dir.clone(), path)?;
            create_parents(&abs)?;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&abs)
                .map_err(|e| rhai_err(format!("append_text('{path}'): {e}")))?;
            f.write_all(content.as_bytes())
                .map_err(|e| rhai_err(format!("append_text('{path}'): {e}")))
        },
    );

    let s = state.clone();
    engine.register_fn(
        "copy",
        move |src: &str, dst: &str| -> Result<(), Box<EvalAltResult>> {
            let root = s.borrow().project_dir.clone();
            let from = contained(&root, src)?;
            let to = contained(&root, dst)?;
            create_parents(&to)?;
            std::fs::copy(&from, &to)
                .map(|_| ())
                .map_err(|e| rhai_err(format!("copy('{src}', '{dst}'): {e}")))
        },
    );

    let s = state.clone();
    engine.register_fn(
        "makedirs",
        move |path: &str| -> Result<(), Box<EvalAltResult>> {
            let abs = contained(&s.borrow().project_dir.clone(), path)?;
            std::fs::create_dir_all(&abs).map_err(|e| rhai_err(format!("makedirs('{path}'): {e}")))
        },
    );

    let s = state.clone();
    engine.register_fn(
        "listdir",
        move |path: &str| -> Result<Array, Box<EvalAltResult>> {
            let abs = contained(&s.borrow().project_dir.clone(), path)?;
            let mut out = Array::new();
            let entries =
                std::fs::read_dir(&abs).map_err(|e| rhai_err(format!("listdir('{path}'): {e}")))?;
            for entry in entries.flatten() {
                out.push(Dynamic::from(
                    entry.file_name().to_string_lossy().into_owned(),
                ));
            }
            Ok(out)
        },
    );

    let s = state.clone();
    engine.register_fn("exists", move |path: &str| -> bool {
        contained(&s.borrow().project_dir.clone(), path)
            .map(|p| p.exists())
            .unwrap_or(false)
    });
    let s = state.clone();
    engine.register_fn("is_file", move |path: &str| -> bool {
        contained(&s.borrow().project_dir.clone(), path)
            .map(|p| p.is_file())
            .unwrap_or(false)
    });
    let s = state.clone();
    engine.register_fn("is_dir", move |path: &str| -> bool {
        contained(&s.borrow().project_dir.clone(), path)
            .map(|p| p.is_dir())
            .unwrap_or(false)
    });
}

/// Pure path-string helpers (no filesystem access, no confinement needed) named
/// after their Python `os.path` / `pathlib` equivalents.
fn register_path_fns(engine: &mut Engine) {
    engine.register_fn("join", |a: &str, b: &str| -> String {
        path_string(&Path::new(a).join(b))
    });
    engine.register_fn("join", |parts: Array| -> String {
        let mut p = PathBuf::new();
        for part in parts.iter() {
            p.push(part.to_string());
        }
        path_string(&p)
    });
    engine.register_fn("basename", |path: &str| -> String {
        Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    engine.register_fn("dirname", |path: &str| -> String {
        Path::new(path)
            .parent()
            .map(path_string)
            .unwrap_or_default()
    });
    // Filename without its final extension (`calc.y` → `calc`), like pathlib `.stem`.
    engine.register_fn("stem", |path: &str| -> String {
        Path::new(path)
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    // Final extension without the dot (`calc.y` → `y`), `""` when there is none.
    engine.register_fn("ext", |path: &str| -> String {
        Path::new(path)
            .extension()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    // Return a whitespace-trimmed *copy* (Python's `str.strip`). Rhai's built-in
    // `.trim()` mutates in place and returns `()`, which surprises people piping
    // `capture(...).stdout` — use `strip(...)` when you want a value back.
    engine.register_fn("strip", |s: &str| -> String { s.trim().to_string() });
    // Split text into an array of lines (handy for scanning tool output).
    engine.register_fn("lines", |s: &str| -> Array {
        s.lines().map(|l| Dynamic::from(l.to_string())).collect()
    });
}

/// Create the parent directories of `path` (used by the write helpers so scripts
/// don't have to `makedirs` first).
fn create_parents(path: &Path) -> Result<(), Box<EvalAltResult>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| rhai_err(format!("could not create '{}': {e}", parent.display())))?;
    }
    Ok(())
}

fn absolutize(root: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        root.join(p)
    }
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Path to the running `freight` executable, exposed to plugins as `FREIGHT_BIN`
/// so a build-system plugin can call freight back (e.g. the cmake dependency
/// provider runs `${FREIGHT_BIN} cmake-provide <name>` on demand). Falls back to
/// the bare name (resolved via PATH) if the current exe can't be determined.
fn freight_bin_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| path_string(&p))
        .unwrap_or_else(|| "freight".to_string())
}

/// Lexically collapse `.` / `..` (no filesystem access — works for not-yet-created
/// generated files).
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `candidate` (resolved against, and lexically normalized) stays inside
/// `root`. `root` is assumed already canonical.
fn is_within(root: &Path, candidate: &Path) -> bool {
    let abs = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    normalize_lexical(&abs).starts_with(root)
}

/// Resolve `path` against `root` and require it to stay within the project.
/// Rejected paths abort the build — a plugin may only add files inside the
/// project it's building for.
fn contained(root: &Path, path: &str) -> Result<PathBuf, Box<EvalAltResult>> {
    let abs = absolutize(root, path);
    if is_within(root, &abs) {
        Ok(abs)
    } else {
        Err(rhai_err(format!(
            "plugin path `{path}` escapes the project directory — plugins may only \
             add files inside the project being built"
        )))
    }
}

// ── TOML → Rhai ─────────────────────────────────────────────────────────────

fn toml_to_dynamic(value: &toml::Value) -> Dynamic {
    match value {
        toml::Value::String(s) => Dynamic::from(s.clone()),
        toml::Value::Integer(i) => Dynamic::from(*i),
        toml::Value::Float(f) => Dynamic::from(*f),
        toml::Value::Boolean(b) => Dynamic::from(*b),
        toml::Value::Datetime(d) => Dynamic::from(d.to_string()),
        toml::Value::Array(a) => Dynamic::from(a.iter().map(toml_to_dynamic).collect::<Array>()),
        toml::Value::Table(t) => {
            let mut map = Map::new();
            for (k, v) in t {
                map.insert(k.clone().into(), toml_to_dynamic(v));
            }
            Dynamic::from_map(map)
        }
    }
}

// ── Host / target environment exposed to scripts ────────────────────────────

/// The host and target characteristics a plugin script can branch on, surfaced
/// as the `HOST` and `TARGET` object constants. Mirrors [`crate::environment`]'s
/// resolved OS/arch (so the values match the manifest's `[os.*]` / `[arch.*]`
/// vocabulary), reduced to what a codegen plugin needs.
struct PluginEnv {
    host_os: String,
    host_arch: String,
    target_os: String,
    target_arch: String,
    /// The cross-compilation triple, or `None` for a native build.
    target_triple: Option<String>,
    /// The consuming project's package name (also the library's name).
    pkg_name: String,
    /// The project's `[lib]` target, if it builds one.
    lib: Option<LibInfo>,
    /// The project's `[[bin]]` targets.
    bins: Vec<BinInfo>,
    /// The project's declared dependencies and where their source lives.
    pkgs: Vec<PkgInfo>,
}

/// The consuming project's library target, surfaced to scripts as `LIB`.
struct LibInfo {
    lib_type: String,
    hdrs: Vec<String>,
    srcs: Vec<String>,
    link: String,
}

/// One of the consuming project's executables, surfaced in the `BINS` map.
struct BinInfo {
    name: String,
    src: String,
    required_features: Vec<String>,
}

/// A declared dependency, surfaced in the `PKGS` map. `dir` is where the
/// package's source lives (a `path` dep's directory, else the `.pkgs/<name>`
/// fetch cache) — it may not exist yet, so a plugin should `exists(...)`-check it.
struct PkgInfo {
    name: String,
    dir: String,
    version: String,
    /// Declared `external = true`: fetched but built by a plugin, not core.
    external: bool,
    /// Declared `source = true`: build from source even if a prebuilt exists.
    source: bool,
    /// Declared `debug = true`: fetch the debug prebuilt in debug builds.
    debug: bool,
}

impl PluginEnv {
    fn for_project(project_dir: &Path) -> Self {
        let env = crate::environment::Environment::for_project(project_dir);
        let manifest = load_manifest_cached(project_dir).ok();
        let pkg_name = manifest
            .as_ref()
            .map(|m| m.package.name.clone())
            .unwrap_or_default();
        let lib = manifest
            .as_ref()
            .and_then(|m| m.lib.as_ref())
            .map(|l| LibInfo {
                lib_type: match l.lib_type {
                    crate::manifest::types::LibType::Static => "static",
                    crate::manifest::types::LibType::Shared => "shared",
                    crate::manifest::types::LibType::Header => "header",
                }
                .to_string(),
                hdrs: l.hdrs.clone(),
                srcs: l.srcs.clone(),
                link: l.link.clone().unwrap_or_default(),
            });
        let bins = manifest
            .as_ref()
            .map(|m| {
                m.bins
                    .iter()
                    .map(|b| BinInfo {
                        name: b.name.clone(),
                        src: b.src.clone(),
                        required_features: b.required_features.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pkgs = manifest
            .as_ref()
            .map(|m| {
                use crate::manifest::types::Dependency;
                let mut v: Vec<PkgInfo> = m
                    .effective_dependencies()
                    .into_iter()
                    .map(|(name, dep)| {
                        let (version, dir, external, source, debug) = match &dep {
                            Dependency::Simple(ver) => (ver.clone(), None, false, false, false),
                            Dependency::Detailed(d) => (
                                d.version.clone().unwrap_or_default(),
                                d.path.clone(),
                                d.external,
                                d.source,
                                d.debug,
                            ),
                        };
                        let dir = match dir {
                            Some(p) => project_dir.join(p),
                            None => project_dir.join(".pkgs").join(&name),
                        };
                        PkgInfo {
                            name,
                            dir: path_string(&dir),
                            version,
                            external,
                            source,
                            debug,
                        }
                    })
                    .collect();
                v.sort_by(|a, b| a.name.cmp(&b.name));
                v
            })
            .unwrap_or_default();
        Self {
            host_os: env.host_os,
            host_arch: env.host_arch,
            target_os: env.target_os,
            target_arch: env.target_arch,
            target_triple: env.target_triple,
            pkg_name,
            lib,
            bins,
            pkgs,
        }
    }
}

/// Build the `PKGS` constant: an object map keyed by dependency name, each value
/// `#{ name, dir, version }`. A plugin building a foreign dep reads
/// `PKGS["libfoo"].dir` for its source location.
fn pkgs_map(env: &PluginEnv) -> Dynamic {
    let mut map = Map::new();
    for p in &env.pkgs {
        let mut m = Map::new();
        m.insert("name".into(), Dynamic::from(p.name.clone()));
        m.insert("dir".into(), Dynamic::from(p.dir.clone()));
        m.insert("version".into(), Dynamic::from(p.version.clone()));
        m.insert("external".into(), Dynamic::from(p.external));
        m.insert("source".into(), Dynamic::from(p.source));
        m.insert("debug".into(), Dynamic::from(p.debug));
        map.insert(p.name.clone().into(), Dynamic::from_map(m));
    }
    Dynamic::from_map(map)
}

/// Build the `LIB` constant: an object map describing the project's library, or
/// `()` (Rhai unit) when the project has no `[lib]`.
fn lib_object(env: &PluginEnv) -> Dynamic {
    let Some(lib) = &env.lib else {
        return Dynamic::UNIT;
    };
    let str_array = |v: &[String]| -> Dynamic {
        Dynamic::from_array(v.iter().map(|s| Dynamic::from(s.clone())).collect())
    };
    let mut m = Map::new();
    m.insert("name".into(), Dynamic::from(env.pkg_name.clone()));
    m.insert("type".into(), Dynamic::from(lib.lib_type.clone()));
    m.insert("hdrs".into(), str_array(&lib.hdrs));
    m.insert("srcs".into(), str_array(&lib.srcs));
    m.insert("link".into(), Dynamic::from(lib.link.clone()));
    Dynamic::from_map(m)
}

/// Build the `BINS` constant: an object map keyed by executable name (names are
/// unique), each value `#{ name, src, required_features }`. Look one up with
/// `BINS["cli"]`; iterate with `for b in BINS.values()` or `BINS.keys()`.
fn bins_map(env: &PluginEnv) -> Dynamic {
    let mut map = Map::new();
    for b in &env.bins {
        let mut m = Map::new();
        m.insert("name".into(), Dynamic::from(b.name.clone()));
        m.insert("src".into(), Dynamic::from(b.src.clone()));
        m.insert(
            "required_features".into(),
            Dynamic::from_array(
                b.required_features
                    .iter()
                    .map(|f| Dynamic::from(f.clone()))
                    .collect(),
            ),
        );
        map.insert(b.name.clone().into(), Dynamic::from_map(m));
    }
    Dynamic::from_map(map)
}

/// OS family, in the spirit of CMake's `WIN32` / `UNIX`: `"windows"` for Windows,
/// `"wasm"` for WebAssembly targets, `"unix"` for everything else (incl. macOS).
fn os_family(os: &str) -> &'static str {
    match os {
        "windows" => "windows",
        "wasi" | "emscripten" | "unknown" => "wasm",
        _ => "unix",
    }
}

/// Pointer width in bits, derived from the architecture name.
fn pointer_width(arch: &str) -> i64 {
    match arch {
        "x86_64" | "aarch64" | "riscv64" | "powerpc64" | "mips64" | "s390x" | "sparc64"
        | "wasm64" | "loongarch64" => 64,
        _ => 32,
    }
}

/// The `TOOLS` constant: every compiler tool freight knows about (from the
/// toolchain templates) plus the `linker` / `archiver` roles, so a script can
/// discover valid `add_flag` targets. Each entry is
/// `#{ name, family, kind }` (`kind` is `"compiler"` / `"linker"` / `"archiver"`).
fn tools_list() -> Dynamic {
    let mut arr = Array::new();
    for t in crate::toolchain::builtin::all_compiler_templates() {
        // Skip non-compiler templates (e.g. debuggers).
        if !t.kind.is_empty() {
            continue;
        }
        let mut m = Map::new();
        m.insert("name".into(), Dynamic::from(t.name.clone()));
        m.insert("family".into(), Dynamic::from(t.family.clone()));
        m.insert("kind".into(), Dynamic::from("compiler".to_string()));
        arr.push(Dynamic::from_map(m));
    }
    for role in ["linker", "archiver"] {
        let mut m = Map::new();
        m.insert("name".into(), Dynamic::from(role.to_string()));
        m.insert("family".into(), Dynamic::from(String::new()));
        m.insert("kind".into(), Dynamic::from(role.to_string()));
        arr.push(Dynamic::from_map(m));
    }
    Dynamic::from_array(arr)
}

/// Build a Rhai object map describing a platform (`HOST` / `TARGET`). `triple` is
/// included only for the target (`""` when native).
fn platform_map(os: &str, arch: &str, triple: Option<&str>) -> Dynamic {
    let mut m = Map::new();
    m.insert("os".into(), Dynamic::from(os.to_string()));
    m.insert("arch".into(), Dynamic::from(arch.to_string()));
    m.insert("family".into(), Dynamic::from(os_family(os).to_string()));
    m.insert("pointer_width".into(), Dynamic::from(pointer_width(arch)));
    if let Some(t) = triple {
        m.insert("triple".into(), Dynamic::from(t.to_string()));
    }
    Dynamic::from_map(m)
}

// ── Running a single plugin script ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_script(
    script_path: &Path,
    project_dir: &Path,
    section: &str,
    section_cfg: &toml::Value,
    allowed_tools: &[String],
    out_dir: &Path,
    tool_paths: &[PathBuf],
    env: &PluginEnv,
    progress: &Progress,
) -> Result<RawOutput, FreightError> {
    let script = std::fs::read_to_string(script_path).map_err(|e| {
        FreightError::BuildScriptFailed(
            script_path.display().to_string(),
            format!("cannot read plugin script: {e}"),
        )
    })?;

    // Canonical project root: the containment boundary for file functions.
    let root = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    // Rebase the output dir under the canonical root so files added from it pass
    // the containment check, and create it (it's where the plugin writes).
    let out_dir = root.join(out_dir.strip_prefix(project_dir).unwrap_or(out_dir));
    std::fs::create_dir_all(&out_dir).map_err(|e| {
        FreightError::BuildScriptFailed(
            script_path.display().to_string(),
            format!("cannot create plugin out dir: {e}"),
        )
    })?;
    let target_dir = out_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("target"));
    // The profile is the last component of `target/<profile>` — surfaced as the
    // `PROFILE` constant so a script can branch on debug vs release.
    let profile = target_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("debug")
        .to_string();

    let state: State = Rc::new(RefCell::new(CtxState {
        project_dir: root.clone(),
        allowed_tools: allowed_tools.to_vec(),
        tool_paths: tool_paths.to_vec(),
        out: RawOutput::default(),
        progress: progress.clone(),
    }));

    let mut scope = Scope::new();
    // Project directories + the matched section path, as `SCREAMING_CASE` constants.
    scope.push_constant("SECTION", section.to_string());
    scope.push_constant("PROJECT_DIR", path_string(&root));
    scope.push_constant("SRC_DIR", path_string(&root.join("src")));
    scope.push_constant("INCLUDE_DIR", path_string(&root.join("include")));
    scope.push_constant("TARGET_DIR", path_string(&target_dir));
    scope.push_constant("OUT_DIR", path_string(&out_dir));
    scope.push_constant("PROFILE", profile);
    scope.push_constant("FREIGHT_BIN", freight_bin_path());
    // The consuming project's own package name (so a plugin can recognise a
    // self-build: `[cmake] build = "<this package>"` → build PROJECT_DIR).
    scope.push_constant("PKG_NAME", env.pkg_name.clone());
    // Host/target characteristics as object maps (`HOST.os`, `TARGET.arch`, …).
    scope.push_constant("HOST", platform_map(&env.host_os, &env.host_arch, None));
    scope.push_constant(
        "TARGET",
        platform_map(
            &env.target_os,
            &env.target_arch,
            Some(env.target_triple.as_deref().unwrap_or("")),
        ),
    );
    // The tools available as `add_flag` targets.
    scope.push_constant("TOOLS", tools_list());
    // The consuming project's targets: `LIB` (object or `()`), `BINS` (map by name).
    scope.push_constant("LIB", lib_object(env));
    scope.push_constant("BINS", bins_map(env));
    // The project's dependencies, keyed by name (`PKGS["libfoo"].dir`).
    scope.push_constant("PKGS", pkgs_map(env));
    scope.push_constant("CFG", toml_to_dynamic(section_cfg));

    run_engine(
        &script,
        &script_path.display().to_string(),
        &state,
        &mut scope,
        progress,
    )
}

/// Create the sandboxed Rhai engine (print/debug routing, resource limits, the
/// plugin API), run `script` with `scope`, and return the collected output.
/// Shared by `run_script` (manifest-driven plugins) and `run_build_system`
/// (core building foreign source through a bundled build-system plugin).
fn run_engine(
    script: &str,
    label: &str,
    state: &State,
    scope: &mut Scope,
    progress: &Progress,
) -> Result<RawOutput, FreightError> {
    let mut engine = Engine::new();
    // Route `print` to the build output via the progress sink (so it shows when
    // building and is silent under the LSP), and `debug` to the log. Never write
    // straight to stdout — that would corrupt the LSP's JSON-RPC stream.
    let print_progress = progress.clone();
    engine.on_print(move |text| {
        print_progress(BuildEvent::ScriptOutput {
            source: "plugin".to_string(),
            text: text.to_string(),
            is_err: false,
        });
    });
    engine.on_debug(|text, source, pos| {
        tracing::debug!(target: "freight::plugin", "{text} ({source:?} @ {pos:?})")
    });
    // Bound runaway/malicious scripts: cap total operations and recursion depth
    // (codegen orchestration stays far under these; the real work is in `run`).
    engine.set_max_operations(100_000_000);
    engine.set_max_call_levels(256);
    register_fns(&mut engine, state);

    engine
        .run_with_scope(scope, script)
        .map_err(|e| FreightError::BuildScriptFailed(label.to_string(), e.to_string()))?;
    Ok(std::mem::take(&mut state.borrow_mut().out))
}

// ── Building foreign source via bundled build-system plugins ─────────────────

/// The build-system plugin scripts, embedded so core can build foreign source
/// itself — foreign-self packages and from-source build-deps — by running the
/// *same* plugin a consuming project would, with no on-disk `plugins/` needed at
/// runtime. Returns the script and its allowed tools.
fn embedded_build_system(backend: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match backend {
        "cmake" => (
            include_str!("../../plugins/cmake/cmake.freight"),
            &["cmake"],
        ),
        "make" => (include_str!("../../plugins/make/make.freight"), &["make"]),
        "meson" => (
            include_str!("../../plugins/meson/meson.freight"),
            &["meson"],
        ),
        "autotools" => (
            include_str!("../../plugins/autotools/autotools.freight"),
            &["sh", "make"],
        ),
        "scons" => (
            include_str!("../../plugins/scons/scons.freight"),
            &["scons"],
        ),
        "bazel" => (
            include_str!("../../plugins/bazel/bazel.freight"),
            &["bazel"],
        ),
        _ => return None,
    })
}

/// Build the foreign-source package `name` (living at `source_dir`) with the
/// bundled `backend` build-system plugin, into `out_dir`, and return the include
/// dirs + link flags it wires up. `root` is the containment boundary and must
/// contain both `source_dir` and `out_dir`. This is how core builds foreign
/// source — it runs exactly the plugin a project would, with a synthesized scope
/// (`PKGS` = just this package, `CFG.build` = its name).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn run_build_system(
    backend: &str,
    name: &str,
    source_dir: &Path,
    out_dir: &Path,
    root: &Path,
    profile: &str,
    defines: &[String],
    prefixes: &[PathBuf],
    tool_paths: &[PathBuf],
    progress: &Progress,
) -> Result<PluginBuildOutput, FreightError> {
    let (script, tools) = embedded_build_system(backend).ok_or_else(|| {
        FreightError::BuildScriptFailed(
            backend.to_string(),
            format!("no bundled build-system plugin for '{backend}'"),
        )
    })?;

    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let _ = std::fs::create_dir_all(out_dir);
    let target_dir = root.join("target").join(profile);
    let env = PluginEnv::for_project(&root);

    let state: State = Rc::new(RefCell::new(CtxState {
        project_dir: root.clone(),
        allowed_tools: tools.iter().map(|s| s.to_string()).collect(),
        tool_paths: tool_paths.to_vec(),
        out: RawOutput::default(),
        progress: progress.clone(),
    }));

    let mut scope = Scope::new();
    scope.push_constant("SECTION", backend.to_string());
    scope.push_constant("PROJECT_DIR", path_string(&root));
    scope.push_constant("SRC_DIR", path_string(&root.join("src")));
    scope.push_constant("INCLUDE_DIR", path_string(&root.join("include")));
    scope.push_constant("TARGET_DIR", path_string(&target_dir));
    scope.push_constant("OUT_DIR", path_string(out_dir));
    scope.push_constant("PROFILE", profile.to_string());
    scope.push_constant("FREIGHT_BIN", freight_bin_path());
    scope.push_constant("HOST", platform_map(&env.host_os, &env.host_arch, None));
    scope.push_constant(
        "TARGET",
        platform_map(
            &env.target_os,
            &env.target_arch,
            Some(env.target_triple.as_deref().unwrap_or("")),
        ),
    );
    scope.push_constant("TOOLS", tools_list());
    scope.push_constant("LIB", Dynamic::UNIT);
    scope.push_constant("BINS", Dynamic::from_map(Map::new()));

    // Synthesized PKGS — just the package being built.
    let mut pkg = Map::new();
    pkg.insert("name".into(), Dynamic::from(name.to_string()));
    pkg.insert("dir".into(), Dynamic::from(path_string(source_dir)));
    pkg.insert("version".into(), Dynamic::from(String::new()));
    pkg.insert("external".into(), Dynamic::from(true));
    pkg.insert("source".into(), Dynamic::from(true));
    pkg.insert("debug".into(), Dynamic::from(profile == "debug"));
    let mut pkgs = Map::new();
    pkgs.insert(name.into(), Dynamic::from_map(pkg));
    scope.push_constant("PKGS", Dynamic::from_map(pkgs));

    // Synthesized CFG — `build = name`, plus any configure defines.
    let mut cfg = Map::new();
    cfg.insert("build".into(), Dynamic::from(name.to_string()));
    cfg.insert(
        "defines".into(),
        Dynamic::from_array(defines.iter().map(|d| Dynamic::from(d.clone())).collect()),
    );
    if !prefixes.is_empty() {
        cfg.insert(
            "prefixes".into(),
            Dynamic::from_array(
                prefixes
                    .iter()
                    .map(|p| Dynamic::from(path_string(p)))
                    .collect(),
            ),
        );
    }
    scope.push_constant("CFG", Dynamic::from_map(cfg));

    let raw = run_engine(
        script,
        &format!("<bundled {backend} plugin>"),
        &state,
        &mut scope,
        progress,
    )?;

    let mut out = PluginBuildOutput {
        include_dirs: raw.include_dirs,
        defines: raw.defines,
        tool_flags: raw.tool_flags,
        prefixes: raw.prefixes,
        ..Default::default()
    };
    for abs in raw.sources {
        if let Some(lang_key) = lang_key_for(&abs) {
            let rel = abs
                .strip_prefix(&root)
                .map(Path::to_path_buf)
                .unwrap_or(abs);
            out.sources.push(SourceFile {
                path: rel,
                lang_key,
            });
        }
    }
    Ok(out)
}

// ── Discovery + orchestration ───────────────────────────────────────────────

/// Enumerate every plugin package available to the project at `project_dir`:
/// a `path` dependency **or** a package fetched into `.pkgs/` (via registry,
/// git, or archive URL) that declares `[plugin]`. Each entry is
/// `(package_dir, manifest)`, de-duplicated by canonical path.
///
/// Whether a plugin actually *runs* is still gated downstream by its `handles`
/// matching a section the consumer declares (plus `goals`/`profiles`), so it is
/// safe to surface every fetched plugin here — an unrelated one simply never
/// matches a section and does nothing.
fn plugin_packages(project_dir: &Path) -> Vec<(PathBuf, crate::manifest::Manifest)> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // Path dependencies (and workspace members, which can't be plugins but are
    // filtered out by the `[plugin]` check below).
    for (dep_dir, kind, _key) in source_package_dirs(project_dir) {
        if matches!(kind, PackageKind::PathDep) {
            candidates.push(dep_dir);
        }
    }

    // Build-dependency path deps. A build plugin (cmake, protoc, …) is a build
    // dependency, but `source_package_dirs` only walks runtime/dev deps (build
    // deps aren't linked/included), so scan them here too. Version build-dep
    // plugins are already covered by the `.pkgs/` scan below.
    if let Ok(m) = load_manifest_cached(project_dir) {
        for dep in m.build_dependencies.values() {
            if let crate::manifest::types::Dependency::Detailed(d) = dep {
                if let Some(rel) = &d.path {
                    let dir = project_dir.join(rel);
                    if dir.is_dir() {
                        candidates.push(dir);
                    }
                }
            }
        }
    }

    // Fetched packages in the project's `.pkgs/` cache.
    let pkgs = project_dir.join(".pkgs");
    if pkgs.is_dir() {
        for entry in std::fs::read_dir(&pkgs).into_iter().flatten().flatten() {
            let dir = entry.path();
            if dir.is_dir() {
                candidates.push(dir);
            }
        }
    }

    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in candidates {
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !seen.insert(canon) {
            continue;
        }
        if let Ok(m) = load_manifest_cached(&dir) {
            if m.plugin.is_some() {
                out.push((dir, m));
            }
        }
    }
    out
}

/// Run every active plugin for the project at `project_dir`: each plugin package
/// (path dependency or fetched into `.pkgs/`) that declares `[plugin]` and
/// `handles` a section the project declares. Returns the generated sources,
/// include dirs, and defines.
pub fn run_plugins(
    project_dir: &Path,
    profile: &str,
    goal: &str,
    tool_paths: &[PathBuf],
    seed_prefixes: &[PathBuf],
    progress: &Progress,
) -> Result<PluginBuildOutput, FreightError> {
    // The consumer's section tables (e.g. `[proto]`) are read from the raw
    // manifest — they aren't part of the typed `Manifest`.
    let raw = std::fs::read_to_string(project_dir.join("freight.toml"))
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok());
    let Some(raw) = raw else {
        return Ok(PluginBuildOutput::default());
    };

    // Every table path in the consumer manifest (dotted, e.g. `proto`,
    // `compiler.clang`, `language.zig`) that a plugin's `handles` may match.
    let section_paths = collect_section_paths(&raw);

    // Resolved host/target, computed once and shared by every plugin run.
    let env = PluginEnv::for_project(project_dir);

    let mut out = PluginBuildOutput::default();
    // Install prefixes available to the next plugin run: core-resolved deps seed
    // it, then each foreign-build plugin appends the prefixes it produced. Fed in
    // as `CFG.prefixes` so a dep built later can find_package an earlier one.
    let mut acc_prefixes: Vec<PathBuf> = seed_prefixes.to_vec();

    for (dep_dir, dep_manifest) in plugin_packages(project_dir) {
        let Some(plugin) = &dep_manifest.plugin else {
            continue;
        };

        // Activation: goal + profile gates (empty list = any).
        if !plugin.goals.is_empty() && !plugin.goals.iter().any(|g| g == goal) {
            continue;
        }
        if !plugin.profiles.is_empty() && !plugin.profiles.iter().any(|p| p == profile) {
            continue;
        }

        let patterns = if plugin.handles.is_empty() {
            vec![dep_manifest.package.name.clone()]
        } else {
            plugin.handles.clone()
        };

        // Match each declared section path against the plugin's patterns; run the
        // plugin once per matched section (de-duplicated).
        let mut ran: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (path, cfg) in &section_paths {
            if !patterns.iter().any(|pat| section_matches(pat, path)) {
                continue;
            }
            if !ran.insert(path.as_str()) {
                continue;
            }
            let script_path = dep_dir.join(&plugin.entry);
            let plugin_out_dir = project_dir
                .join("target")
                .join(profile)
                .join("plugin-gen")
                .join(path);

            // Thread the prefixes built so far into this plugin's `CFG.prefixes`.
            let cfg = inject_prefixes(cfg, &acc_prefixes);
            let cfg = &cfg;

            // Incremental: when `inputs` are declared, reuse the previous output
            // unless an input (or the cfg/script) changed.
            let raw_out = if plugin.inputs.is_empty() {
                progress(BuildEvent::RunningScript { cached: false });
                run_script(
                    &script_path,
                    project_dir,
                    path,
                    cfg,
                    &plugin.tools,
                    &plugin_out_dir,
                    tool_paths,
                    &env,
                    progress,
                )?
            } else {
                let fp = fingerprint(project_dir, &plugin.inputs, cfg, &script_path);
                match read_cache(&plugin_out_dir).filter(|c| c.fingerprint == fp) {
                    Some(cached) => {
                        progress(BuildEvent::RunningScript { cached: true });
                        cached.into_raw()
                    }
                    None => {
                        progress(BuildEvent::RunningScript { cached: false });
                        let o = run_script(
                            &script_path,
                            project_dir,
                            path,
                            cfg,
                            &plugin.tools,
                            &plugin_out_dir,
                            tool_paths,
                            &env,
                            progress,
                        )?;
                        write_cache(&plugin_out_dir, &fp, &o);
                        o
                    }
                }
            };

            // Prefixes this plugin produced become visible to later plugins.
            for p in &raw_out.prefixes {
                if !acc_prefixes.contains(p) {
                    acc_prefixes.push(p.clone());
                }
            }
            out.prefixes.extend(raw_out.prefixes);

            for abs in raw_out.sources {
                if let Some(lang_key) = lang_key_for(&abs) {
                    let rel = abs
                        .strip_prefix(project_dir)
                        .map(Path::to_path_buf)
                        .unwrap_or(abs);
                    out.sources.push(SourceFile {
                        path: rel,
                        lang_key,
                    });
                }
            }
            out.include_dirs.extend(raw_out.include_dirs);
            out.defines.extend(raw_out.defines);
            out.tool_flags.extend(raw_out.tool_flags);
        }
    }

    Ok(out)
}

/// The include directories active plugins expose, computed **without running**
/// any script (deterministic: each active plugin's `OUT_DIR`). The LSP and
/// `compile_commands.json` generators add these so generated headers (e.g.
/// `foo.pb.h`) resolve and aren't flagged as undeclared — even before a build
/// has populated them.
pub fn plugin_include_dirs(project_dir: &Path, profile: &str) -> Vec<PathBuf> {
    plugin_generated_dirs(project_dir, profile)
        .into_iter()
        .map(|d| d.out_dir)
        .collect()
}

/// A plugin-owned generated-output directory, annotated with the plugin that
/// produces it and the manifest section that triggered it. Same deterministic
/// computation as [`plugin_include_dirs`] (no script execution), but keeps the
/// provenance the LSP needs to label generated headers ("generated by …").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGenDir {
    /// The `OUT_DIR` the plugin writes into (`target/<profile>/plugin-gen/<section>`).
    pub out_dir: PathBuf,
    /// Package name of the plugin providing the codegen (e.g. `"proto"`).
    pub plugin_name: String,
    /// The manifest section that activated it (e.g. `"proto"`).
    pub section: String,
}

/// Compute every active plugin's generated-output directory with provenance,
/// without running any script. See [`plugin_include_dirs`] for the include-dir
/// projection used by the compile-command generators.
pub fn plugin_generated_dirs(project_dir: &Path, profile: &str) -> Vec<PluginGenDir> {
    let Some(raw) = std::fs::read_to_string(project_dir.join("freight.toml"))
        .ok()
        .and_then(|s| s.parse::<toml::Value>().ok())
    else {
        return Vec::new();
    };
    let section_paths = collect_section_paths(&raw);
    let mut dirs = Vec::new();
    for (_dep_dir, m) in plugin_packages(project_dir) {
        let Some(plugin) = &m.plugin else {
            continue;
        };
        let patterns = if plugin.handles.is_empty() {
            vec![m.package.name.clone()]
        } else {
            plugin.handles.clone()
        };
        let mut seen = std::collections::HashSet::new();
        for (path, _cfg) in &section_paths {
            if patterns.iter().any(|p| section_matches(p, path)) && seen.insert(path.clone()) {
                dirs.push(PluginGenDir {
                    out_dir: project_dir
                        .join("target")
                        .join(profile)
                        .join("plugin-gen")
                        .join(path),
                    plugin_name: m.package.name.clone(),
                    section: path.clone(),
                });
            }
        }
    }
    dirs
}

/// A plugin's advisory section schema: the sections it `handles` and the keys it
/// documents (`[plugin.schema]`). Surfaced to the LSP so a consumer editing a
/// handled section (e.g. `[proto]`) gets key completion and hover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSchema {
    /// Section patterns the plugin handles (e.g. `["proto"]`, `["compiler.*"]`).
    pub handles: Vec<String>,
    /// `(key, description)` pairs the plugin documents for its section.
    pub keys: Vec<(String, String)>,
    /// The plugin package's name (shown as the completion item source).
    pub plugin_name: String,
}

impl PluginSchema {
    /// Whether this plugin handles `section` (a dotted manifest path).
    pub fn handles_section(&self, section: &str) -> bool {
        self.handles.iter().any(|p| section_matches(p, section))
    }
}

/// Collect the advisory section schemas of every plugin package available to the
/// project (path deps + `.pkgs/`). Only plugins that declare a non-empty
/// `[plugin.schema]` contribute. No script execution.
pub fn plugin_schemas(project_dir: &Path) -> Vec<PluginSchema> {
    let mut out = Vec::new();
    for (_dir, m) in plugin_packages(project_dir) {
        let Some(plugin) = &m.plugin else { continue };
        if plugin.schema.is_empty() {
            continue;
        }
        let handles = if plugin.handles.is_empty() {
            vec![m.package.name.clone()]
        } else {
            plugin.handles.clone()
        };
        out.push(PluginSchema {
            handles,
            keys: plugin
                .schema
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            plugin_name: m.package.name.clone(),
        });
    }
    out
}

// ── Incremental cache ────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct PluginCache {
    fingerprint: String,
    sources: Vec<String>,
    include_dirs: Vec<String>,
    defines: Vec<String>,
    /// Persisted `(tool, flag)` pairs so a cached run still contributes them.
    #[serde(default)]
    tool_flags: Vec<(String, String)>,
    /// Persisted install prefixes so a cached run still exports them downstream.
    #[serde(default)]
    prefixes: Vec<String>,
}

impl PluginCache {
    fn into_raw(self) -> RawOutput {
        RawOutput {
            sources: self.sources.into_iter().map(PathBuf::from).collect(),
            include_dirs: self.include_dirs.into_iter().map(PathBuf::from).collect(),
            defines: self.defines,
            tool_flags: self
                .tool_flags
                .into_iter()
                .map(|(tool, flag)| ToolFlag { tool, flag })
                .collect(),
            prefixes: self.prefixes.into_iter().map(PathBuf::from).collect(),
        }
    }
}

fn cache_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".freight-plugin.json")
}

fn read_cache(out_dir: &Path) -> Option<PluginCache> {
    let data = std::fs::read_to_string(cache_path(out_dir)).ok()?;
    serde_json::from_str(&data).ok()
}

fn write_cache(out_dir: &Path, fingerprint: &str, out: &RawOutput) {
    let cache = PluginCache {
        fingerprint: fingerprint.to_string(),
        sources: out
            .sources
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        include_dirs: out
            .include_dirs
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        defines: out.defines.clone(),
        tool_flags: out
            .tool_flags
            .iter()
            .map(|tf| (tf.tool.clone(), tf.flag.clone()))
            .collect(),
        prefixes: out
            .prefixes
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = std::fs::create_dir_all(out_dir);
        let _ = std::fs::write(cache_path(out_dir), json);
    }
}

/// Hash of the plugin's declared inputs (paths + mtimes), its `cfg`, and the
/// script file — changes to any of these re-trigger the plugin.
fn fingerprint(
    project_dir: &Path,
    inputs: &[String],
    cfg: &toml::Value,
    script_path: &Path,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(format!("{cfg:?}").as_bytes());
    if let Ok(mtime) = std::fs::metadata(script_path).and_then(|m| m.modified()) {
        hasher.update(format!("{mtime:?}").as_bytes());
    }
    let mut files: Vec<(String, std::time::SystemTime)> = Vec::new();
    for pat in inputs {
        let full = project_dir.join(pat);
        if let Ok(paths) = glob::glob(&full.to_string_lossy()) {
            for entry in paths.flatten() {
                let mtime = std::fs::metadata(&entry)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                files.push((entry.to_string_lossy().into_owned(), mtime));
            }
        }
    }
    files.sort();
    for (p, mtime) in files {
        hasher.update(p.as_bytes());
        hasher.update(format!("{mtime:?}").as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Return a copy of a section's config table with `prefixes` set to the given
/// install-prefix paths (as a string array). Lets the pipeline hand each plugin
/// the prefixes of everything built before it via `CFG.prefixes`. A plugin that
/// already declares `prefixes` keeps its own value (explicit wins).
fn inject_prefixes(cfg: &toml::Value, prefixes: &[PathBuf]) -> toml::Value {
    let mut table = cfg.as_table().cloned().unwrap_or_default();
    if !prefixes.is_empty() && !table.contains_key("prefixes") {
        let arr = prefixes
            .iter()
            .map(|p| toml::Value::String(p.to_string_lossy().into_owned()))
            .collect();
        table.insert("prefixes".to_string(), toml::Value::Array(arr));
    }
    toml::Value::Table(table)
}

/// All table paths in a manifest, dotted (e.g. `proto`, `compiler.clang`).
/// Arrays-of-tables (`[[bin]]`) are not section targets and are skipped.
fn collect_section_paths(root: &toml::Value) -> Vec<(String, &toml::Value)> {
    let mut out = Vec::new();
    fn walk<'a>(prefix: &str, table: &'a toml::Value, out: &mut Vec<(String, &'a toml::Value)>) {
        let Some(map) = table.as_table() else {
            return;
        };
        for (key, value) in map {
            if !value.is_table() {
                continue;
            }
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            walk(&path, value, out);
            out.push((path, value));
        }
    }
    walk("", root, &mut out);
    out
}

/// Match a `handles` pattern against a dotted section path. Segments are split
/// on `.`; `*` matches exactly one segment and `**` matches one or more.
/// e.g. `proto` → `proto`; `compiler.*` → `compiler.clang`;
/// `language.**` → `language.zig` and `language.zig.foo` (but not bare `language`).
pub(crate) fn section_matches(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('.').collect();
    let seg: Vec<&str> = path.split('.').collect();
    matches_segs(&pat, &seg)
}

fn matches_segs(pat: &[&str], seg: &[&str]) -> bool {
    match pat.first() {
        None => seg.is_empty(),
        Some(&"**") => (1..=seg.len()).any(|i| matches_segs(&pat[1..], &seg[i..])),
        Some(&"*") => !seg.is_empty() && matches_segs(&pat[1..], &seg[1..]),
        Some(&lit) => !seg.is_empty() && seg[0] == lit && matches_segs(&pat[1..], &seg[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    /// A deterministic native host/target for `run_script` unit tests (doesn't
    /// read the machine's real config the way `PluginEnv::for_project` does).
    fn test_env() -> PluginEnv {
        PluginEnv {
            host_os: "linux".into(),
            host_arch: "x86_64".into(),
            target_os: "linux".into(),
            target_arch: "x86_64".into(),
            target_triple: None,
            pkg_name: "testpkg".into(),
            lib: None,
            bins: vec![],
            pkgs: vec![],
        }
    }

    #[test]
    fn script_collects_sources_includes_and_defines() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write(
            tmp.path(),
            "p.freight",
            r#"add_source("gen/a.cpp");
               add_include_dir("gen");
               define("FOO");
               define("BAR", "1");"#,
        );
        let out = run_script(
            &script,
            tmp.path(),
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &tmp.path().join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert_eq!(out.sources, vec![tmp.path().join("gen/a.cpp")]);
        assert_eq!(out.include_dirs, vec![tmp.path().join("gen")]);
        assert_eq!(out.defines, vec!["FOO".to_string(), "BAR=1".to_string()]);
    }

    #[test]
    fn cfg_section_is_readable_in_script() {
        let tmp = tempfile::tempdir().unwrap();
        let script = write(
            tmp.path(),
            "p.freight",
            r#"if CFG.enabled { define("ON"); } define(CFG.name);"#,
        );
        let cfg: toml::Value = "enabled = true\nname = \"proto\"\n".parse().unwrap();
        let out = run_script(
            &script,
            tmp.path(),
            "proto",
            &cfg,
            &[],
            &tmp.path().join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert!(out.defines.contains(&"ON".to_string()));
        assert!(out.defines.contains(&"proto".to_string()));
    }

    #[test]
    fn disallowed_tool_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // `rm` is not in the allow-list — must error before executing anything.
        let script = write(tmp.path(), "p.freight", r#"run("rm", ["-rf", "x"]);"#);
        let err = run_script(
            &script,
            tmp.path(),
            "codegen",
            &toml::Value::Table(Default::default()),
            &["protoc".to_string()],
            &tmp.path().join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        );
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("disallowed tool 'rm'"));
    }

    #[test]
    fn directory_constants_expose_project_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"define("PROJ=" + PROJECT_DIR);
               define("SRC=" + SRC_DIR);
               define("INC=" + INCLUDE_DIR);
               define("OUT=" + OUT_DIR);
               define("TARGET=" + TARGET_DIR);
               define("SEC=" + SECTION);"#,
        );
        let out_dir = proj.join("target/debug/plugin-gen/codegen");
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &out_dir,
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        let has = |k: &str, v: String| out.defines.contains(&format!("{k}={v}"));
        assert!(has("PROJ", path_string(&proj)));
        assert!(has("SRC", path_string(&proj.join("src"))));
        assert!(has("INC", path_string(&proj.join("include"))));
        assert!(has("OUT", path_string(&out_dir)));
        assert!(has("TARGET", path_string(&proj.join("target/debug"))));
        assert!(out.defines.contains(&"SEC=codegen".to_string()));
    }

    #[test]
    fn host_and_target_objects_expose_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"define("HOS=" + HOST.os);
               define("HARCH=" + HOST.arch);
               define("HFAM=" + HOST.family);
               define("HPW=" + HOST.pointer_width);
               define("TOS=" + TARGET.os);
               define("TARCH=" + TARGET.arch);
               define("TFAM=" + TARGET.family);
               define("TRIPLE=" + TARGET.triple);"#,
        );
        // A cross target: aarch64 windows, exercising family + pointer width.
        let env = PluginEnv {
            host_os: "linux".into(),
            host_arch: "x86_64".into(),
            target_os: "windows".into(),
            target_arch: "aarch64".into(),
            target_triple: Some("aarch64-pc-windows-msvc".into()),
            pkg_name: "testpkg".into(),
            lib: None,
            bins: vec![],
            pkgs: vec![],
        };
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("target/debug/plugin-gen/codegen"),
            &[],
            &env,
            &crate::event::silent(),
        )
        .unwrap();
        let has = |s: &str| out.defines.contains(&s.to_string());
        assert!(has("HOS=linux"));
        assert!(has("HARCH=x86_64"));
        assert!(has("HFAM=unix"));
        assert!(has("HPW=64"));
        assert!(has("TOS=windows"));
        assert!(has("TARCH=aarch64"));
        assert!(has("TFAM=windows"));
        assert!(has("TRIPLE=aarch64-pc-windows-msvc"));
    }

    #[test]
    fn native_target_triple_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"define("T=[" + TARGET.triple + "]");"#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("target/debug/plugin-gen/codegen"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert!(out.defines.contains(&"T=[]".to_string()));
    }

    #[test]
    fn python_like_io_and_path_helpers() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"
            // write creates parent dirs; read gets it back
            write_text("gen/sub/hello.txt", "hi");
            define("READ=" + read_text("gen/sub/hello.txt"));
            append_text("gen/sub/hello.txt", "!");
            define("APP=" + read_text("gen/sub/hello.txt"));
            define("EX=" + exists("gen/sub/hello.txt"));
            define("ISF=" + is_file("gen/sub/hello.txt"));
            define("ISD=" + is_dir("gen/sub"));
            copy("gen/sub/hello.txt", "gen/copy.txt");
            define("COPIED=" + read_text("gen/copy.txt"));
            makedirs("gen/deep/er");
            define("MK=" + is_dir("gen/deep/er"));
            // path helpers (pure)
            define("BASE=" + basename("a/b/calc.y"));
            define("DIR=" + dirname("a/b/calc.y"));
            define("STEM=" + stem("a/b/calc.y"));
            define("EXT=" + ext("a/b/calc.y"));
            define("JOIN=" + join("a", "b"));
            "#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("target/debug/plugin-gen/codegen"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        let has = |s: &str| out.defines.contains(&s.to_string());
        assert!(has("READ=hi"));
        assert!(has("APP=hi!"));
        assert!(has("EX=true"));
        assert!(has("ISF=true"));
        assert!(has("ISD=true"));
        assert!(has("COPIED=hi!"));
        assert!(has("MK=true"));
        assert!(has("BASE=calc.y"));
        assert!(has(&format!("DIR={}", "a/b")));
        assert!(has("STEM=calc"));
        assert!(has("EXT=y"));
        assert!(has(&format!("JOIN={}", "a/b")));
    }

    #[test]
    fn io_outside_project_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let script = write(&proj, "p.freight", r#"write_text("../escape.txt", "x");"#);
        let err = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        );
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("escapes the project"));
    }

    #[test]
    fn capture_returns_output_and_respects_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"let r = capture("echo", ["hello"]);
               define("OUT=" + strip(r.stdout));
               define("CODE=" + r.code);"#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &["echo".to_string()],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert!(out.defines.contains(&"OUT=hello".to_string()));
        assert!(out.defines.contains(&"CODE=0".to_string()));
    }

    #[test]
    fn run_with_cwd_executes_in_that_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(proj.join("sub")).unwrap();
        // `sh -c 'pwd > marker'` run with cwd="sub" writes the marker inside sub/.
        let script = write(
            &proj,
            "p.freight",
            r#"run("sh", ["-c", "pwd > pwd.txt"], "sub");
               define("OK=" + exists("sub/pwd.txt"));
               define("PWD=" + strip(read_text("sub/pwd.txt")).ends_with("/sub"));"#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &["sh".to_string()],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert!(out.defines.contains(&"OK=true".to_string()));
        assert!(out.defines.contains(&"PWD=true".to_string()));
    }

    #[test]
    fn capture_disallowed_tool_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(&proj, "p.freight", r#"capture("rm", ["-rf", "x"]);"#);
        let err = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &["echo".to_string()],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        );
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("disallowed tool 'rm'"));
    }

    #[test]
    fn run_surfaces_tool_output_via_progress() {
        use std::sync::{Arc, Mutex};
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(&proj, "p.freight", r#"run("echo", ["hi"]);"#);
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let progress: Progress = Arc::new(move |e| {
            if let BuildEvent::ScriptOutput {
                source,
                text,
                is_err,
            } = e
            {
                sink.lock()
                    .unwrap()
                    .push(format!("{source}|{text}|{is_err}"));
            }
        });
        run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &["echo".to_string()],
            &proj.join("out"),
            &[],
            &test_env(),
            &progress,
        )
        .unwrap();
        let ev = events.lock().unwrap();
        assert!(ev.iter().any(|s| s == "echo|hi|false"), "events={ev:?}");
    }

    #[test]
    fn regex_helpers_extract_and_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"let line = "src/main.cpp:42: error: boom";
               define("TEST=" + re_test("error", line));
               let m = re_find("(\\S+):(\\d+): error: (.*)", line);
               define("FILE=" + m[1]);
               define("LINE=" + m[2]);
               define("MSG=" + m[3]);
               let all = re_find_all("(\\d+)", "a1 b22 c333");
               define("COUNT=" + all.len());
               define("REPL=" + re_replace("\\d+", "x9y", "N"));"#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        let has = |s: &str| out.defines.contains(&s.to_string());
        assert!(has("TEST=true"));
        assert!(has("FILE=src/main.cpp"));
        assert!(has("LINE=42"));
        assert!(has("MSG=boom"));
        assert!(has("COUNT=3"));
        assert!(has("REPL=xNy"));
    }

    #[test]
    fn lib_and_bin_objects_expose_project_targets() {
        // A consuming project with both a library and two executables, plus a
        // plugin that reads LIB/BIN. Exercised through run_plugins so PluginEnv
        // is built from the real manifest.
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().canonicalize().unwrap();
        let plug = app.join(".pkgs/inspect");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"inspect\"\nversion=\"0.1.0\"\n[plugin]\nentry=\"p.freight\"\nhandles=[\"inspect\"]\n",
        )
        .unwrap();
        std::fs::write(
            plug.join("p.freight"),
            r#"define("LIBNAME=" + LIB.name);
               define("LIBTYPE=" + LIB.type);
               define("NBINS=" + BINS.len());
               define("CLI=" + BINS["cli"].src);
               let total = 0;
               for b in BINS.values() { total += 1; }
               define("ITER=" + total);"#,
        )
        .unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\ninspect=\"0.1.0\"\n[inspect]\n\
             [lib]\ntype=\"shared\"\nhdrs=[\"include/app.h\"]\n\
             [[bin]]\nname=\"cli\"\nsrc=\"src/cli.cpp\"\n\
             [[bin]]\nname=\"daemon\"\nsrc=\"src/daemon.cpp\"\n",
        )
        .unwrap();

        let p = crate::event::silent();
        let out = run_plugins(&app, "debug", "build", &[], &[], &p).unwrap();
        let has = |s: &str| out.defines.contains(&s.to_string());
        assert!(has("LIBNAME=app"));
        assert!(has("LIBTYPE=shared"));
        assert!(has("NBINS=2"));
        assert!(has("CLI=src/cli.cpp"));
        assert!(has("ITER=2"));
    }

    #[test]
    fn profile_constant_reflects_the_build_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().canonicalize().unwrap();
        let plug = app.join(".pkgs/p");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[plugin]\nentry=\"p.freight\"\nhandles=[\"p\"]\n",
        )
        .unwrap();
        std::fs::write(
            plug.join("p.freight"),
            r#"if PROFILE == "release" { define("REL"); } else { define("DBG"); }
               define("P=" + PROFILE);"#,
        )
        .unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n[dependencies]\np=\"0.1.0\"\n[p]\n",
        )
        .unwrap();

        let prog = crate::event::silent();
        let dbg = run_plugins(&app, "debug", "build", &[], &[], &prog).unwrap();
        assert!(dbg.defines.contains(&"DBG".to_string()));
        assert!(dbg.defines.contains(&"P=debug".to_string()));
        let rel = run_plugins(&app, "release", "build", &[], &[], &prog).unwrap();
        assert!(rel.defines.contains(&"REL".to_string()));
        assert!(rel.defines.contains(&"P=release".to_string()));
    }

    #[test]
    fn pkgs_map_exposes_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().canonicalize().unwrap();
        let plug = app.join(".pkgs/inspect");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"inspect\"\nversion=\"0.1.0\"\n[plugin]\nentry=\"p.freight\"\nhandles=[\"inspect\"]\n",
        )
        .unwrap();
        std::fs::write(
            plug.join("p.freight"),
            r#"define("ZLIBVER=" + PKGS["zlib"].version);
               define("ZLIBDIR_OK=" + PKGS["zlib"].dir.ends_with(".pkgs/zlib"));
               define("LOCALDIR_OK=" + PKGS["local"].dir.ends_with("vendor/local"));"#,
        )
        .unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\ninspect=\"0.1.0\"\nzlib=\"1.3\"\nlocal={path=\"vendor/local\"}\n\
             [inspect]\n",
        )
        .unwrap();

        let p = crate::event::silent();
        let out = run_plugins(&app, "debug", "build", &[], &[], &p).unwrap();
        let has = |s: &str| out.defines.contains(&s.to_string());
        assert!(has("ZLIBVER=1.3"));
        assert!(has("ZLIBDIR_OK=true"));
        assert!(has("LOCALDIR_OK=true"));
    }

    #[test]
    fn run_build_system_builds_a_cmake_project() {
        let cmake = std::process::Command::new("cmake")
            .arg("--version")
            .output();
        if cmake.map(|o| !o.status.success()).unwrap_or(true) {
            eprintln!("skipping run_build_system cmake test: cmake not installed");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let src = root.join("vendor/mylib");
        std::fs::create_dir_all(src.join("include")).unwrap();
        std::fs::create_dir_all(src.join("src")).unwrap();
        std::fs::write(
            src.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.10)\nproject(mylib C)\n\
             find_package(Threads)\n\
             add_library(mylib STATIC src/mylib.c)\n\
             target_include_directories(mylib PUBLIC include)\n\
             install(TARGETS mylib ARCHIVE DESTINATION lib)\n\
             install(DIRECTORY include/ DESTINATION include)\n",
        )
        .unwrap();
        std::fs::write(src.join("include/mylib.h"), "int f(void);\n").unwrap();
        std::fs::write(src.join("src/mylib.c"), "int f(void){return 1;}\n").unwrap();

        let out_dir = root.join("target/bs");
        let p = crate::event::silent();
        let out = run_build_system(
            "cmake",
            "mylib",
            &src,
            &out_dir,
            &root,
            "release",
            &[],
            &[],
            &[],
            &p,
        )
        .unwrap();
        // The plugin installed headers + a static lib and wired them in.
        assert!(
            out.include_dirs
                .iter()
                .any(|d| d.ends_with("install/include")),
            "include dirs: {:?}",
            out.include_dirs
        );
        assert!(
            out.tool_flags
                .iter()
                .any(|t| t.tool == "linker" && t.flag.contains("libmylib.a")),
            "tool_flags: {:?}",
            out.tool_flags
        );
        // Freight.cmake's dependency provider recorded the find_package() calls
        // (method name is upper-cased, as CMake passes it to the provider).
        let report = out_dir.join("mylib/freight-report.txt");
        let recorded = std::fs::read_to_string(&report).unwrap_or_default();
        assert!(
            recorded.contains("FIND_PACKAGE Threads"),
            "report should record find_package(Threads): {recorded:?}"
        );
    }

    #[test]
    fn add_flag_records_tool_targeted_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"add_flag("clang", "-fno-rtti");
               add_flag("linker", "-Wl,--gc-sections");
               // TOOLS lists discoverable targets, incl. the linker role.
               define("HAS_LINKER=" + TOOLS.some(|t| t.name == "linker"));
               define("HAS_GCC=" + TOOLS.some(|t| t.name == "gcc"));"#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("target/debug/plugin-gen/codegen"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert_eq!(
            out.tool_flags,
            vec![
                ToolFlag {
                    tool: "clang".into(),
                    flag: "-fno-rtti".into()
                },
                ToolFlag {
                    tool: "linker".into(),
                    flag: "-Wl,--gc-sections".into()
                },
            ]
        );
        assert!(out.defines.contains(&"HAS_LINKER=true".to_string()));
        assert!(out.defines.contains(&"HAS_GCC=true".to_string()));
    }

    #[test]
    fn link_lib_and_link_dir_emit_linker_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().canonicalize().unwrap();
        let script = write(
            &proj,
            "p.freight",
            r#"link_lib("z");
               link_lib("/abs/libfoo.a");
               link_dir("/abs/lib");"#,
        );
        let out = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        )
        .unwrap();
        assert_eq!(
            out.tool_flags,
            vec![
                ToolFlag {
                    tool: "linker".into(),
                    flag: "-lz".into()
                },
                ToolFlag {
                    tool: "linker".into(),
                    flag: "/abs/libfoo.a".into()
                },
                ToolFlag {
                    tool: "linker".into(),
                    flag: "-L/abs/lib".into()
                },
            ]
        );
    }

    #[test]
    fn tool_flag_matching_by_name_alias_family_and_role() {
        let flags = vec![
            ToolFlag {
                tool: "clang".into(),
                flag: "-a".into(),
            },
            ToolFlag {
                tool: "llvm".into(),
                flag: "-b".into(),
            },
            ToolFlag {
                tool: "compiler".into(),
                flag: "-c".into(),
            },
            ToolFlag {
                tool: "gcc".into(),
                flag: "-d".into(),
            },
            ToolFlag {
                tool: "linker".into(),
                flag: "-e".into(),
            },
        ];
        // clang++ has name "clang++", alias "clang", family "llvm".
        let m = compiler_tool_flags(&flags, "clang++", Some("clang"), "llvm");
        assert_eq!(m, vec!["-a", "-b", "-c"]); // alias, family, catch-all (not gcc)
                                               // role selector
        assert_eq!(role_tool_flags(&flags, "linker"), vec!["-e"]);
        assert!(role_tool_flags(&flags, "archiver").is_empty());
    }

    #[test]
    fn add_source_outside_project_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let script = write(&proj, "p.freight", r#"add_source("../escape.c");"#);
        let err = run_script(
            &script,
            &proj,
            "codegen",
            &toml::Value::Table(Default::default()),
            &[],
            &proj.join("out"),
            &[],
            &test_env(),
            &crate::event::silent(),
        );
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("escapes the project"));
    }

    #[test]
    fn is_within_blocks_escapes() {
        let root = Path::new("/proj");
        assert!(is_within(root, Path::new("/proj/src/a.c")));
        assert!(is_within(root, Path::new("gen/a.c"))); // relative → under root
        assert!(!is_within(root, Path::new("/etc/passwd")));
        assert!(!is_within(root, Path::new("/proj/../secret")));
    }

    #[test]
    fn plugin_include_dirs_lists_active_out_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let plug = root.join("plug");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"plug\"\nversion=\"0.1.0\"\n[plugin]\nentry=\"p.freight\"\nhandles=[\"proto\"]\n",
        )
        .unwrap();
        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\nplug={path=\"../plug\"}\n[proto]\nx=1\n",
        )
        .unwrap();

        let dirs = plugin_include_dirs(&app, "debug");
        assert_eq!(dirs, vec![app.join("target/debug/plugin-gen/proto")]);

        // No matching section → no dirs.
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n[dependencies]\nplug={path=\"../plug\"}\n",
        )
        .unwrap();
        assert!(plugin_include_dirs(&app, "debug").is_empty());
    }

    #[test]
    fn fetched_plugin_in_pkgs_is_discovered_and_run() {
        // A plugin distributed via the registry/git/url lands in `.pkgs/<name>`
        // rather than as a path dependency. It must still be discovered and run
        // when the consumer declares a section it handles.
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().canonicalize().unwrap();
        let plug = app.join(".pkgs/proto");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"proto\"\nversion=\"0.1.0\"\n\
             [plugin]\nentry=\"p.freight\"\nhandles=[\"proto\"]\n",
        )
        .unwrap();
        std::fs::write(plug.join("p.freight"), r#"define("RAN");"#).unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\nproto=\"0.1.0\"\n[proto]\nx=1\n",
        )
        .unwrap();

        // Discovery (no execution) sees the fetched plugin's out dir.
        let dirs = plugin_include_dirs(&app, "debug");
        assert_eq!(dirs, vec![app.join("target/debug/plugin-gen/proto")]);

        // And it actually runs.
        let p = crate::event::silent();
        let out = run_plugins(&app, "debug", "build", &[], &[], &p).unwrap();
        assert_eq!(out.defines, vec!["RAN".to_string()]);
    }

    #[test]
    fn inject_prefixes_adds_array_without_clobbering_explicit() {
        let cfg: toml::Value = toml::from_str("build = \"zlib\"\n").unwrap();
        let augmented = inject_prefixes(&cfg, &[PathBuf::from("/a"), PathBuf::from("/b")]);
        let arr = augmented
            .get("prefixes")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("/a"));

        // An explicit `prefixes` in the manifest is preserved (explicit wins).
        let cfg: toml::Value = toml::from_str("prefixes = [\"/x\"]\n").unwrap();
        let augmented = inject_prefixes(&cfg, &[PathBuf::from("/a")]);
        let arr = augmented
            .get("prefixes")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].as_str(), Some("/x"));
    }

    #[test]
    fn seed_prefixes_reach_cfg_and_add_prefix_is_exported() {
        // A plugin reads CFG.prefixes (seeded by the pipeline) and registers its
        // own install prefix via add_prefix — which run_plugins surfaces so a
        // later plugin can resolve it.
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().canonicalize().unwrap();
        let plug = app.join(".pkgs/dep");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"dep\"\nversion=\"0.1.0\"\n\
             [plugin]\nentry=\"p.freight\"\nhandles=[\"dep\"]\n",
        )
        .unwrap();
        // For each seeded prefix define a marker, then export OUT_DIR as a prefix.
        std::fs::write(
            plug.join("p.freight"),
            r#"for p in CFG.prefixes { define("SEEN_" + basename(p)); }
               add_prefix(OUT_DIR);"#,
        )
        .unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\ndep=\"0.1.0\"\n[dep]\nx=1\n",
        )
        .unwrap();

        let seed = app.join("seedprefix");
        let p = crate::event::silent();
        let out = run_plugins(&app, "debug", "build", &[], &[seed.clone()], &p).unwrap();

        assert!(
            out.defines.contains(&"SEEN_seedprefix".to_string()),
            "seed prefix reached CFG.prefixes: {:?}",
            out.defines
        );
        assert!(
            out.prefixes
                .contains(&app.join("target/debug/plugin-gen/dep")),
            "add_prefix exported the plugin's OUT_DIR: {:?}",
            out.prefixes
        );
    }

    #[test]
    fn goal_gating_activates_only_for_listed_goals() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let plug = root.join("plug");
        std::fs::create_dir_all(&plug).unwrap();
        std::fs::write(
            plug.join("freight.toml"),
            "[package]\nname=\"plug\"\nversion=\"0.1.0\"\n\
             [plugin]\nentry=\"p.freight\"\nhandles=[\"codegen\"]\ngoals=[\"test\"]\n",
        )
        .unwrap();
        std::fs::write(plug.join("p.freight"), r#"define("RAN");"#).unwrap();

        let app = root.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n\
             [dependencies]\nplug={path=\"../plug\"}\n[codegen]\nx=1\n",
        )
        .unwrap();

        let p = crate::event::silent();
        let build = run_plugins(&app, "debug", "build", &[], &[], &p).unwrap();
        assert!(
            build.defines.is_empty(),
            "gated off for `build`: {:?}",
            build.defines
        );
        let test = run_plugins(&app, "debug", "test", &[], &[], &p).unwrap();
        assert_eq!(test.defines, vec!["RAN".to_string()]);
    }

    #[test]
    fn fingerprint_changes_and_cache_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path();
        std::fs::create_dir_all(proj.join("inp")).unwrap();
        std::fs::write(proj.join("inp/a.txt"), "1").unwrap();
        let script = proj.join("s.freight");
        std::fs::write(&script, "").unwrap();
        let cfg = toml::Value::Table(Default::default());
        let inputs = vec!["inp/*.txt".to_string()];

        let f1 = fingerprint(proj, &inputs, &cfg, &script);
        std::fs::write(proj.join("inp/b.txt"), "x").unwrap(); // new input file
        let f2 = fingerprint(proj, &inputs, &cfg, &script);
        assert_ne!(f1, f2, "adding an input changes the fingerprint");

        let out = RawOutput {
            sources: vec![proj.join("g.cpp")],
            include_dirs: vec![proj.join("inc")],
            defines: vec!["D".to_string()],
            tool_flags: vec![ToolFlag {
                tool: "clang".into(),
                flag: "-X".into(),
            }],
            prefixes: vec![proj.join("prefix")],
        };
        let od = proj.join("out");
        write_cache(&od, "fp", &out);
        let cached = read_cache(&od).unwrap();
        assert_eq!(cached.fingerprint, "fp");
        let raw = cached.into_raw();
        assert_eq!(raw.sources, vec![proj.join("g.cpp")]);
        assert_eq!(raw.defines, vec!["D".to_string()]);
        assert_eq!(raw.tool_flags, out.tool_flags);
        assert_eq!(raw.prefixes, vec![proj.join("prefix")]);
    }

    #[test]
    fn handles_patterns_match_nested_and_wildcards() {
        // exact top-level
        assert!(section_matches("proto", "proto"));
        assert!(!section_matches("proto", "compiler.proto"));
        // exact nested
        assert!(section_matches("compiler.clang", "compiler.clang"));
        // single-segment wildcard
        assert!(section_matches("compiler.*", "compiler.clang"));
        assert!(!section_matches("compiler.*", "compiler.clang.opt")); // * = exactly one
        assert!(!section_matches("compiler.*", "compiler")); // needs a subsegment
                                                             // recursive wildcard = one or more (not the bare parent)
        assert!(section_matches("language.**", "language.zig"));
        assert!(section_matches("language.**", "language.zig.flags"));
        assert!(!section_matches("language.**", "language"));
    }

    #[test]
    fn collect_section_paths_finds_nested_tables() {
        let raw: toml::Value =
            "[proto]\nout=\"x\"\n[compiler.clang]\nopt=3\n[language.zig]\nstd=\"x\"\n"
                .parse()
                .unwrap();
        let paths: Vec<String> = collect_section_paths(&raw)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert!(paths.contains(&"proto".to_string()));
        assert!(paths.contains(&"compiler".to_string()));
        assert!(paths.contains(&"compiler.clang".to_string()));
        assert!(paths.contains(&"language.zig".to_string()));
    }
}

/// Map a generated file's extension to the language key that compiles it.
fn lang_key_for(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    let key = match ext.as_str() {
        "cc" | "cpp" | "cxx" | "c++" | "cppm" | "ixx" => "cpp",
        "c" => "c",
        "cu" => "cuda",
        "m" => "objc",
        "mm" => "objcpp",
        _ => return None,
    };
    Some(key.to_string())
}
