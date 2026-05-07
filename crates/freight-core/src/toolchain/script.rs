use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope};

use crate::error::FreightError;

// ── Builder structs ───────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub(super) struct ToolchainDef {
    pub name: String,
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    pub extensions: Vec<String>,
    pub standards: HashMap<String, String>,
    pub flags_opt:      HashMap<String, String>,
    pub flags_debug:    HashMap<String, String>,
    pub flags_warnings: HashMap<String, String>,
    pub flags_lto:      HashMap<String, String>,
    pub flags_lto_link: HashMap<String, String>,
    pub flags_stdlib:   HashMap<String, String>,
    pub flags_runtime:  HashMap<String, String>,
    pub sanitize:       String,
    pub cpu_ext:        String,
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
    /// role → binary
    pub toolset: HashMap<String, String>,
    /// Extra flags accumulated during `load()` — role → flags
    pub load_flags: HashMap<String, Vec<String>>,
    pub supported_archs: Vec<String>,
    pub supported_os: Vec<String>,
    pub required_tools: Vec<String>,
    pub required_env: Vec<String>,
    pub min_version: Option<String>,
    pub requires_toolchain: Vec<String>,
    pub family: String,
    pub sanitizer_options: Vec<String>,
    /// PCH params: "compile" flag, "use" template, "extension" (e.g. ".pch" / ".gch")
    pub pch: HashMap<String, String>,
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
        if let Some(d) = cell.borrow_mut().as_mut() { f(d); }
    });
}

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) { CURRENT.with(|c| *c.borrow_mut() = None); }
}

// ── Map types exposed to templates ────────────────────────────────────────────
//
// Per-category flag maps (opt, debug, warnings, lto, lto_link, stdlib, runtime)
// are plain rhai::Map scope variables — native indexer-set works directly.
// sanitize and cpu_ext are plain String scope variables.
//
// Custom map types (with thread-local write-back) are used for open-ended keyed
// data that must reach ToolchainDef during script evaluation:
//   standards   — standards["c++20"] = "-std=c++20"
//   linking     — linking["cpp"] = #{ abi: "c++", ... }
//   modules     — modules["style"] = "gcc"; modules["compile_miu"] = "..."
//   toolset     — toolset["cc"] = "gcc"
//   load_flags  — load_flags["cc"] += ["-m64"]   (in fn load())
//   arch_flags  — arch_flags["x86_64.linux"] = "-f elf64"
//   env         — env["CC"]  (read-only)

/// `standards` — `standards["c++20"] = "-std=c++20"`.
#[derive(Clone)] struct StandardsMap;

/// `modules` — `modules["style"] = "gcc"` etc.
#[derive(Clone)] struct ModulesMap;

/// `linking` — `linking["cpp"] = #{ abi: "c++", compatible: [...], ... }`.
#[derive(Clone)] struct LinkingMap;

/// `toolset` — `toolset["cc"] = "gcc"`.
#[derive(Clone)] struct ToolsetMap;

/// `load_flags` — `load_flags["cc"] += ["-m64"]` (inside `fn load()`).
/// get returns Array copy so `+=` write-back works correctly.
#[derive(Clone)] struct LoadFlagsMap;

/// `arch_flags` — `arch_flags["x86_64.linux"] = "-f elf64"`.
#[derive(Clone)] struct ArchFlagsMap;

/// `pch` — `pch["compile"] = "-x c++-header"` etc.
#[derive(Clone)] struct PchMap;

/// `env` — read-only host environment: `env["ONEAPI_ROOT"]` → string or `()`.
#[derive(Clone)] struct EnvMap;

// ── Include preprocessor ──────────────────────────────────────────────────────

/// Resolve `include("path")` directives by inlining the referenced file's
/// content at the call site. Paths are relative to `dir`; `.rhai` is appended
/// when the path has no extension. Nested includes are resolved recursively.
pub(super) fn resolve_includes(src: &str, dir: &Path) -> Result<String, FreightError> {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        if let Some(path_str) = parse_include(line) {
            let file = if path_str.ends_with(".rhai") {
                PathBuf::from(path_str)
            } else {
                PathBuf::from(format!("{path_str}.rhai"))
            };
            let full = dir.join(&file);
            let inc = std::fs::read_to_string(&full).map_err(|e| {
                FreightError::TemplateError(format!("include \"{path_str}\": {e}"))
            })?;
            let resolved = resolve_includes(&inc, full.parent().unwrap_or(dir))?;
            out.push_str(&resolved);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

fn parse_include(line: &str) -> Option<&str> {
    let s = line.trim().strip_prefix("include(")?;
    let s = s.strip_prefix('"')?;
    let end = s.find('"')?;
    let path = &s[..end];
    let rest = s[end + 1..].trim().strip_prefix(')')?.trim().trim_start_matches(';').trim();
    if rest.is_empty() { Some(path) } else { None }
}

// ── Script evaluation ─────────────────────────────────────────────────────────

/// Evaluate a Rhai toolchain script. When `dir` is provided, `include("path")`
/// directives in the script are resolved relative to that directory first.
pub(super) fn eval_script(src: &str, dir: Option<&Path>) -> Result<ToolchainDef, FreightError> {
    let resolved;
    let src = if let Some(d) = dir {
        resolved = resolve_includes(src, d)?;
        resolved.as_str()
    } else {
        src
    };

    let engine = make_engine();

    let ast = engine
        .compile(src)
        .map_err(|e| FreightError::TemplateError(format!("script compile error: {e}")))?;

    CURRENT.with(|c| *c.borrow_mut() = Some(ToolchainDef::default()));
    let _guard = Guard;

    let mut scope = Scope::new();

    // ── Plain variables — identity & constraints ───────────────────────────
    for key in &[
        "name", "homepage", "binary", "version_arg", "version_regex",
        "passthrough_prefix", "min_version", "family",
    ] {
        scope.push(*key, String::new());
    }
    for key in &[
        "extensions", "always_flags",
        "supported_archs", "supported_os",
        "required_tools", "required_env", "requires_toolchain",
        "sanitizer_options"
    ] {
        scope.push(*key, Array::new());
    }
    scope.push("passthrough", false);

    // ── Plain variables — compiler flag structure ──────────────────────────
    // Fixed schema: every toolchain declares exactly these slots (empty = unsupported).
    for key in &[
        "include_dir", "define", "define_value",
        "output", "compile_only", "dep_file",
        "target", "sysroot",
        // Extended structure slots (MSVC and other toolchains that differ from the defaults).
        "output_obj", "output_bin", "dep_file_mode", "system_lib",
    ] {
        scope.push(*key, String::new());
    }

    // ── Per-category flag maps (plain Rhai Map — native indexer-set works) ─
    for cat in &["opt", "dbg", "warnings", "lto", "lto_link", "stdlib", "runtime"] {
        scope.push(*cat, Map::new());
    }
    // sanitize and cpu_ext are single template strings, grouped near sanitizers.
    scope.push("sanitize", String::new());
    scope.push("cpu_ext",  String::new());

    // ── Other map objects ──────────────────────────────────────────────────
    scope.push("standards",  StandardsMap);
    scope.push("modules",    ModulesMap);
    scope.push("linking",    LinkingMap);
    scope.push("toolset",    ToolsetMap);
    scope.push("load_flags", LoadFlagsMap);
    scope.push("arch_flags", ArchFlagsMap);
    scope.push("pch",        PchMap);
    scope.push("env",        EnvMap);

    // ── Host info ──────────────────────────────────────────────────────────
    scope.push("arch", std::env::consts::ARCH.to_string());
    scope.push("os",   std::env::consts::OS.to_string());

    engine
        .run_ast_with_scope(&mut scope, &ast)
        .map_err(|e| FreightError::TemplateError(format!("script error: {e}")))?;

    // fn load() can append to load_flags via the map type (immediate side-effects).
    let _ = engine.call_fn::<()>(&mut scope, &ast, "load", ());

    // ── Collect map data from thread-local state ───────────────────────────
    let mut def = CURRENT
        .with(|c| c.borrow_mut().take())
        .ok_or_else(|| FreightError::TemplateError("builder lost after eval".into()))?;

    // ── Read plain variables from scope ────────────────────────────────────
    macro_rules! str  { ($k:expr) => { scope.get_value::<String>($k).unwrap_or_default() }; }
    macro_rules! arr  { ($k:expr) => {
        scope.get_value::<Array>($k).unwrap_or_default()
            .into_iter().filter_map(|v| v.try_cast::<String>()).collect::<Vec<_>>()
    }; }
    macro_rules! bool { ($k:expr) => { scope.get_value::<bool>($k).unwrap_or_default() }; }

    def.name                = str!("name");
    def.family              = str!("family");
    def.binary              = str!("binary");
    def.version_arg         = str!("version_arg");
    def.version_regex       = str!("version_regex");
    def.extensions          = arr!("extensions");
    def.always_flags        = arr!("always_flags");
    def.passthrough_enabled = bool!("passthrough");
    def.passthrough_prefix  = str!("passthrough_prefix");
    def.supported_archs     = arr!("supported_archs");
    def.supported_os        = arr!("supported_os");
    def.required_tools      = arr!("required_tools");
    def.required_env        = arr!("required_env");
    def.requires_toolchain  = arr!("requires_toolchain");
    def.sanitizer_options   = arr!("sanitizer_options");
    def.sanitize            = str!("sanitize");
    def.cpu_ext             = str!("cpu_ext");

    macro_rules! flag_map { ($k:expr) => {
        scope.get_value::<Map>($k).unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| v.try_cast::<String>().map(|s| (k.to_string(), s)))
            .collect::<HashMap<String, String>>()
    }; }
    def.flags_opt      = flag_map!("opt");
    def.flags_debug    = flag_map!("dbg");
    def.flags_warnings = flag_map!("warnings");
    def.flags_lto      = flag_map!("lto");
    def.flags_lto_link = flag_map!("lto_link");
    def.flags_stdlib   = flag_map!("stdlib");
    def.flags_runtime  = flag_map!("runtime");

    let mv = str!("min_version");
    if !mv.is_empty() { def.min_version = Some(mv); }

    // Structure: insert every slot (empty string = unsupported/unused).
    for key in &[
        "include_dir", "define", "define_value",
        "output", "compile_only", "dep_file",
        "target", "sysroot",
        // Extended structure slots (MSVC and other toolchains that differ from the defaults).
        "output_obj", "output_bin", "dep_file_mode", "system_lib",
    ] {
        def.structure.insert(key.to_string(), str!(key));
    }

    if def.name.is_empty() {
        return Err(FreightError::TemplateError(
            "toolchain script must set `name = \"...\"`".into(),
        ));
    }

    Ok(def)
}

// ── Engine factory ────────────────────────────────────────────────────────────

fn make_engine() -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(100_000);

    // ── standards map ─────────────────────────────────────────────────────────
    e.register_type_with_name::<StandardsMap>("StandardsMap");
    e.register_indexer_set(|_: &mut StandardsMap, key: ImmutableString, val: String| {
        with_def(|d| { d.standards.insert(key.to_string(), val); });
    });
    e.register_indexer_get(|_: &mut StandardsMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── modules map ───────────────────────────────────────────────────────────
    e.register_type_with_name::<ModulesMap>("ModulesMap");
    e.register_indexer_set(|_: &mut ModulesMap, key: ImmutableString, val: String| {
        with_def(|d| {
            if key.as_str() == "style" { d.module_style = val; }
            else { d.module_params.insert(key.to_string(), val); }
        });
    });
    e.register_indexer_get(|_: &mut ModulesMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── linking map ───────────────────────────────────────────────────────────
    e.register_type_with_name::<LinkingMap>("LinkingMap");
    e.register_indexer_set(|_: &mut LinkingMap, lang: ImmutableString, val: Dynamic| {
        if let Some(m) = val.try_cast::<Map>() {
            with_def(|d| d.linking.push((lang.to_string(), extract_linking(m))));
        }
    });
    e.register_indexer_get(|_: &mut LinkingMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── toolset map ───────────────────────────────────────────────────────────
    e.register_type_with_name::<ToolsetMap>("ToolsetMap");
    e.register_indexer_set(|_: &mut ToolsetMap, role: ImmutableString, bin: String| {
        with_def(|d| { d.toolset.insert(role.to_string(), bin); });
    });
    e.register_indexer_get(|_: &mut ToolsetMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── load_flags map ────────────────────────────────────────────────────────
    // get returns Array copy so `load_flags["cc"] += ["-m64"]` write-back works:
    //   1. get("cc")          → Array (current state or empty)
    //   2. array += ["-m64"]  → appends
    //   3. set("cc", array)   → writes back to CURRENT
    e.register_type_with_name::<LoadFlagsMap>("LoadFlagsMap");
    e.register_indexer_get(|_: &mut LoadFlagsMap, role: ImmutableString| -> Array {
        CURRENT.with(|c| {
            c.borrow().as_ref()
                .and_then(|d| d.load_flags.get(role.as_str()))
                .map(|v| v.iter().map(|s| Dynamic::from(s.clone())).collect())
                .unwrap_or_default()
        })
    });
    e.register_indexer_set(|_: &mut LoadFlagsMap, role: ImmutableString, val: Array| {
        let flags: Vec<String> = val.into_iter().filter_map(|v| v.try_cast()).collect();
        with_def(|d| *d.load_flags.entry(role.to_string()).or_default() = flags);
    });

    // ── arch_flags map ────────────────────────────────────────────────────────
    e.register_type_with_name::<ArchFlagsMap>("ArchFlagsMap");
    e.register_indexer_set(|_: &mut ArchFlagsMap, key: ImmutableString, val: String| {
        with_def(|d| { d.arch_flags.insert(key.to_string(), val); });
    });
    e.register_indexer_get(|_: &mut ArchFlagsMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── pch map ───────────────────────────────────────────────────────────────
    e.register_type_with_name::<PchMap>("PchMap");
    e.register_indexer_set(|_: &mut PchMap, key: ImmutableString, val: ImmutableString| {
        with_def(|d| { d.pch.insert(key.to_string(), val.to_string()); });
    });
    e.register_indexer_get(|_: &mut PchMap, key: ImmutableString| -> ImmutableString {
        CURRENT.with(|c| {
            c.borrow().as_ref()
                .and_then(|d| d.pch.get(key.as_str()).cloned())
                .unwrap_or_default()
                .into()
        })
    });

    // ── env map (read-only) ───────────────────────────────────────────────────
    e.register_type_with_name::<EnvMap>("EnvMap");
    e.register_indexer_get(|_: &mut EnvMap, key: ImmutableString| -> Dynamic {
        std::env::var(key.as_str()).ok().map(Dynamic::from).unwrap_or(Dynamic::UNIT)
    });

    // ── utilities ─────────────────────────────────────────────────────────────
    e.register_fn("find_tool", |name: String| -> Dynamic {
        find_binary(&name).map(Dynamic::from).unwrap_or(Dynamic::UNIT)
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
