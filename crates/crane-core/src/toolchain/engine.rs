use std::cell::RefCell;
use std::collections::HashMap;

use rhai::{Array, Dynamic, Engine, Map, Scope};

use crate::error::CraneError;

// ── Builder structs ───────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub(super) struct ToolchainDef {
    pub name: String,
    /// Primary binary — used for detection (PATH search) and as the linker.
    /// Set via `set_binary(...)`. Overridden per-role by `toolset`.
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    pub extensions: Vec<String>,
    pub standards: HashMap<String, String>,
    /// category → { key → flag }
    /// Known categories: "opt", "debug", "warnings", "lto", "strip",
    ///   "sanitize" (key "template"), "cpu_ext" (key "template")
    pub flags: HashMap<String, HashMap<String, String>>,
    /// structure key → template string
    pub structure: HashMap<String, String>,
    /// "gcc" | "clang" | "none" / ""
    pub module_style: String,
    pub module_params: HashMap<String, String>,
    /// (lang_key, params) — ordered so scripts declare in reading order
    pub linking: Vec<(String, LinkingParams)>,
    pub passthrough_enabled: bool,
    pub passthrough_prefix: String,
    pub always_flags: Vec<String>,
    /// arch key → flag string (e.g. "x86_64.linux" → "-f elf64")
    pub arch_flags: HashMap<String, String>,
    /// role → binary (future role-based dispatch: "cc", "cxx", "ld", "ar", "strip")
    pub toolset: HashMap<String, String>,
    /// Extra flags accumulated during `load()` — role → flags
    pub load_flags: HashMap<String, Vec<String>>,
    /// Host architectures this toolchain is available on (`std::env::consts::ARCH` values).
    /// Empty = no restriction (works on every architecture).
    pub supported_archs: Vec<String>,
    /// Host operating systems this toolchain is available on (`std::env::consts::OS` values).
    /// Empty = no restriction.
    pub supported_os: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct LinkingParams {
    pub abi: String,
    pub compatible: Vec<String>,
    pub extensions: Vec<String>,
    pub compile_binary: Option<String>,
    pub linker: String,
}

// ── Thread-local builder state ────────────────────────────────────────────────

thread_local! {
    static CURRENT: RefCell<Option<ToolchainDef>> = RefCell::new(None);
}

fn with_def<F: FnOnce(&mut ToolchainDef)>(f: F) {
    CURRENT.with(|cell| {
        if let Some(d) = cell.borrow_mut().as_mut() {
            f(d);
        }
    });
}

/// RAII guard that clears the thread-local builder on drop (handles panics).
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        CURRENT.with(|c| *c.borrow_mut() = None);
    }
}

// ── Script evaluation ─────────────────────────────────────────────────────────

/// Evaluate a `.rhai` toolchain script and return the populated `ToolchainDef`.
///
/// Top-level statements are executed first (registration phase). Then `load()`
/// is called if it exists, with `arch` and `os` available as variables.
pub(super) fn eval_script(src: &str) -> Result<ToolchainDef, CraneError> {
    let engine = make_engine();

    let ast = engine
        .compile(src)
        .map_err(|e| CraneError::TemplateError(format!("script compile error: {e}")))?;

    CURRENT.with(|c| *c.borrow_mut() = Some(ToolchainDef::default()));
    let _guard = Guard;

    // Registration phase — top-level calls like set_name(), set_flag(), etc.
    let mut scope = Scope::new();
    engine
        .run_ast_with_scope(&mut scope, &ast)
        .map_err(|e| CraneError::TemplateError(format!("script error: {e}")))?;

    // Load phase — optional `fn load() { ... }` can call add_flags() etc.
    let mut load_scope = Scope::new();
    load_scope.push("arch", std::env::consts::ARCH.to_string());
    load_scope.push("os", std::env::consts::OS.to_string());
    let _ = engine.call_fn::<()>(&mut load_scope, &ast, "load", ());

    let def = CURRENT
        .with(|c| c.borrow_mut().take())
        .ok_or_else(|| CraneError::TemplateError("builder lost after script eval".into()))?;

    if def.name.is_empty() {
        return Err(CraneError::TemplateError(
            "toolchain script must call set_name(\"...\")".into(),
        ));
    }

    Ok(def)
}

// ── Engine factory ────────────────────────────────────────────────────────────

fn make_engine() -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(100_000);

    // ── identity / metadata ──────────────────────────────────────────────────
    e.register_fn("set_name",          |s: String| with_def(|d| d.name = s));
    e.register_fn("set_homepage",      |_: String| {}); // informational only
    e.register_fn("set_binary",        |s: String| with_def(|d| d.binary = s));
    e.register_fn("set_version_arg",   |s: String| with_def(|d| d.version_arg = s));
    e.register_fn("set_version_regex", |s: String| with_def(|d| d.version_regex = s));

    // ── extensions ───────────────────────────────────────────────────────────
    e.register_fn("set_extensions", |arr: Array| {
        with_def(|d| {
            d.extensions = arr
                .into_iter()
                .filter_map(|v| v.try_cast::<String>())
                .collect();
        });
    });

    // ── flag maps ─────────────────────────────────────────────────────────────
    // set_flag(category, key, value)
    // e.g. set_flag("opt", "2", "-O2")
    //      set_flag("sanitize", "template", "-fsanitize={values}")
    e.register_fn("set_flag", |cat: String, key: String, val: String| {
        with_def(|d| {
            d.flags.entry(cat).or_default().insert(key, val);
        });
    });

    // ── standards ─────────────────────────────────────────────────────────────
    e.register_fn("set_standard", |key: String, flag: String| {
        with_def(|d| { d.standards.insert(key, flag); });
    });

    // ── structure templates ───────────────────────────────────────────────────
    // set_structure(key, template_string)
    // Known keys: "include_dir", "define", "define_value", "output", "output_obj",
    //   "output_bin", "compile_only", "dep_file", "dep_file_mode", "system_lib",
    //   "target", "sysroot"
    e.register_fn("set_structure", |key: String, val: String| {
        with_def(|d| { d.structure.insert(key, val); });
    });

    // ── arch flags ────────────────────────────────────────────────────────────
    // set_arch_flag("x86_64.linux", "-f elf64")
    e.register_fn("set_arch_flag", |key: String, val: String| {
        with_def(|d| { d.arch_flags.insert(key, val); });
    });

    // ── always flags ─────────────────────────────────────────────────────────
    e.register_fn("add_always_flag", |flag: String| {
        with_def(|d| d.always_flags.push(flag));
    });

    // ── passthrough ───────────────────────────────────────────────────────────
    e.register_fn("set_passthrough", |enabled: bool, prefix: String| {
        with_def(|d| {
            d.passthrough_enabled = enabled;
            d.passthrough_prefix = prefix;
        });
    });

    // ── module style ──────────────────────────────────────────────────────────
    // set_module_style("gcc", #{ enable_flag: "...", compile_miu: "...",
    //                            import_module: "...", header_unit: "..." })
    // set_module_style("clang", #{ precompile: "...", import_module: "...",
    //                              header_unit: "..." })
    e.register_fn("set_module_style", |style: String, params: Map| {
        with_def(|d| {
            d.module_style = style;
            d.module_params = params
                .into_iter()
                .filter_map(|(k, v)| v.try_cast::<String>().map(|s| (k.to_string(), s)))
                .collect();
        });
    });
    // Overload: set_module_style("none")
    e.register_fn("set_module_style", |style: String| {
        with_def(|d| {
            d.module_style = style;
            d.module_params.clear();
        });
    });

    // ── linking ───────────────────────────────────────────────────────────────
    // set_linking("cpp", #{ abi: "c++", compatible: ["c"], extensions: [".cpp"],
    //                        compile_binary: "gcc", linker: "" })
    e.register_fn("set_linking", |lang: String, params: Map| {
        let lp = extract_linking(params);
        with_def(|d| d.linking.push((lang, lp)));
    });

    // ── toolset roles (forward-compat; full dispatch in a later phase) ────────
    // set_toolset("cc", "gcc")
    e.register_fn("set_toolset", |role: String, binary: String| {
        with_def(|d| { d.toolset.insert(role, binary); });
    });

    // ── load-time flag additions (callable inside fn load()) ─────────────────
    e.register_fn("add_flags", |role: String, flags: Array| {
        with_def(|d| {
            let v: Vec<String> = flags.into_iter().filter_map(|x| x.try_cast()).collect();
            d.load_flags.entry(role).or_default().extend(v);
        });
    });
    e.register_fn("add_flags", |role: String, flag: String| {
        with_def(|d| d.load_flags.entry(role).or_default().push(flag));
    });

    // ── arch / OS constraints ─────────────────────────────────────────────────
    // set_supported_archs(["x86_64", "x86"]) — hide toolchain on other hosts.
    // Empty list (default) = available on all architectures.
    e.register_fn("set_supported_archs", |arr: Array| {
        with_def(|d| {
            d.supported_archs = arr
                .into_iter()
                .filter_map(|v| v.try_cast::<String>())
                .collect();
        });
    });

    // set_supported_os(["linux", "windows"]) — hide toolchain on other OSes.
    // Empty list (default) = available on all operating systems.
    e.register_fn("set_supported_os", |arr: Array| {
        with_def(|d| {
            d.supported_os = arr
                .into_iter()
                .filter_map(|v| v.try_cast::<String>())
                .collect();
        });
    });

    // ── utilities ─────────────────────────────────────────────────────────────
    // find_tool("g++") → "/usr/bin/g++" | ()
    e.register_fn("find_tool", |name: String| -> Dynamic {
        find_binary(&name)
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    });

    // arch() → "x86_64" | "aarch64" | "arm" | …  (std::env::consts::ARCH)
    e.register_fn("arch", || std::env::consts::ARCH.to_string());

    // os() → "linux" | "macos" | "windows" | …  (std::env::consts::OS)
    e.register_fn("os", || std::env::consts::OS.to_string());

    // env("INCLUDE") → "..." | ()
    e.register_fn("env", |key: String| -> Dynamic {
        std::env::var(&key)
            .ok()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    });

    e
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_linking(params: Map) -> LinkingParams {
    let mut lp = LinkingParams::default();
    for (k, v) in params {
        match k.as_str() {
            "abi"    => lp.abi    = v.try_cast::<String>().unwrap_or_default(),
            "linker" => lp.linker = v.try_cast::<String>().unwrap_or_default(),
            "compile_binary" => {
                lp.compile_binary = v.try_cast::<String>().filter(|s| !s.is_empty());
            }
            "compatible" => {
                if let Some(arr) = v.try_cast::<Array>() {
                    lp.compatible = arr.into_iter().filter_map(|x| x.try_cast()).collect();
                }
            }
            "extensions" => {
                if let Some(arr) = v.try_cast::<Array>() {
                    lp.extensions = arr.into_iter().filter_map(|x| x.try_cast()).collect();
                }
            }
            _ => {}
        }
    }
    lp
}

fn find_binary(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = candidate.metadata() {
                if meta.permissions().mode() & 0o111 != 0 {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}
