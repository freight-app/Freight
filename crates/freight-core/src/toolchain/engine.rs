use std::cell::RefCell;
use std::collections::HashMap;

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
    /// category → { key → flag }
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

/// `compiler` — top-level identity, constraints, and metadata.
///
/// ```rhai
/// compiler["name"]               = "gcc";
/// compiler["binary"]             = "g++";
/// compiler["version_arg"]        = "--version";
/// compiler["version_regex"]      = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
/// compiler["extensions"]         = [".cpp", ".cc", ".c"];
/// compiler["always_flags"]       = ["--expt-relaxed-constexpr"];
/// compiler["passthrough"]        = false;
/// compiler["passthrough_prefix"] = "";
/// compiler["min_version"]        = "12.0";
/// compiler["supported_archs"]    = ["x86_64", "aarch64"];
/// compiler["supported_os"]       = ["linux", "windows"];
/// compiler["required_tools"]     = ["ptxas", "fatbinary"];
/// compiler["required_env"]       = ["ONEAPI_ROOT"];
/// compiler["requires_toolchain"] = ["cpp"];
/// compiler["homepage"]           = "https://...";  // informational
/// ```
#[derive(Clone)] struct CompilerMap;

/// `flags` — two-level flag map.
///
/// Assign a whole category at once or write individual entries; both forms work:
/// ```rhai
/// flags["opt"] = #{"0": "-O0", "1": "-O1", "2": "-O2", "3": "-O3"};
/// flags["debug"]["true"]  = "-g";   // chained write-back also works
/// flags["debug"]["false"] = "";
/// flags["sanitize"] = #{"template": "-fsanitize={values}"};
/// ```
#[derive(Clone)] struct FlagsMap;

/// `standards` — language standard → compiler flag.
/// ```rhai
/// standards["c++20"] = "-std=c++20";
/// ```
#[derive(Clone)] struct StandardsMap;

/// `structure` — template patterns for flag assembly.
/// ```rhai
/// structure["include_dir"]  = "-I{path}";
/// structure["define"]       = "-D{name}";
/// structure["define_value"] = "-D{name}={value}";
/// structure["output"]       = "-o {path}";
/// structure["compile_only"] = "-c";
/// structure["dep_file"]     = "-MMD -MF {path}";
/// structure["target"]       = "--target={triple}";  // "" = unsupported
/// structure["sysroot"]      = "--sysroot={path}";
/// ```
#[derive(Clone)] struct StructureMap;

/// `modules` — C++ module configuration.
/// ```rhai
/// modules["style"]         = "gcc";          // "gcc" | "clang" | "none"
/// modules["enable_flag"]   = "-fmodules-ts";
/// modules["compile_miu"]   = "-fmodule-output={pcm_path}";
/// modules["import_module"] = "-fmodule-file={name}={pcm_path}";
/// modules["header_unit"]   = "-fmodule-header";
/// modules["precompile"]    = "--precompile"; // clang two-step
/// ```
#[derive(Clone)] struct ModulesMap;

/// `linking` — language ABI declarations.
/// ```rhai
/// linking["cpp"] = #{
///     abi:            "c++",
///     compatible:     ["c", "fortran"],
///     compile_binary: "g++",  // optional override
///     linker:         "",
///     extensions:     [".cpp", ".cc", ".cxx"],
/// };
/// ```
#[derive(Clone)] struct LinkingMap;

/// `toolset` — role → binary mapping.
/// ```rhai
/// toolset["cc"]    = "gcc";
/// toolset["cxx"]   = "g++";
/// toolset["ld"]    = "g++";
/// toolset["ar"]    = "ar";
/// toolset["strip"] = "strip";
/// ```
#[derive(Clone)] struct ToolsetMap;

/// `load_flags` — role-specific flags added at load time (inside `fn load()`).
/// ```rhai
/// fn load() {
///     if arch == "x86_64" {
///         load_flags["cc"]  += ["-m64"];
///         load_flags["cxx"] += ["-m64"];
///         load_flags["ld"]  += ["-m64"];
///     }
/// }
/// ```
#[derive(Clone)] struct LoadFlagsMap;

/// `arch_flags` — arch+OS key → output-format flag (primarily for assemblers).
/// ```rhai
/// arch_flags["x86_64.linux"]   = "-f elf64";
/// arch_flags["x86_64.windows"] = "-f win64";
/// ```
#[derive(Clone)] struct ArchFlagsMap;

/// `env` — read-only access to host environment variables.
/// ```rhai
/// let root = env["ONEAPI_ROOT"];  // () when unset
/// ```
#[derive(Clone)] struct EnvMap;

// ── Script evaluation ─────────────────────────────────────────────────────────

pub(super) fn eval_script(src: &str) -> Result<ToolchainDef, FreightError> {
    let engine = make_engine();

    let ast = engine
        .compile(src)
        .map_err(|e| FreightError::TemplateError(format!("script compile error: {e}")))?;

    CURRENT.with(|c| *c.borrow_mut() = Some(ToolchainDef::default()));
    let _guard = Guard;

    let mut scope = Scope::new();
    // Map objects — all writes go directly to thread-local CURRENT state.
    scope.push("compiler",   CompilerMap);
    scope.push("flags",      FlagsMap);
    scope.push("standards",  StandardsMap);
    scope.push("structure",  StructureMap);
    scope.push("modules",    ModulesMap);
    scope.push("linking",    LinkingMap);
    scope.push("toolset",    ToolsetMap);
    scope.push("load_flags", LoadFlagsMap);
    scope.push("arch_flags", ArchFlagsMap);
    scope.push("env",        EnvMap);
    // Read-only host info — available as plain variables everywhere.
    scope.push("arch", std::env::consts::ARCH.to_string());
    scope.push("os",   std::env::consts::OS.to_string());

    engine
        .run_ast_with_scope(&mut scope, &ast)
        .map_err(|e| FreightError::TemplateError(format!("script error: {e}")))?;

    // fn check() and fn load() see the same scope (and same thread-local state).
    let _ = engine.call_fn::<()>(&mut scope, &ast, "load", ());

    let def = CURRENT
        .with(|c| c.borrow_mut().take())
        .ok_or_else(|| FreightError::TemplateError("builder lost after eval".into()))?;

    if def.name.is_empty() {
        return Err(FreightError::TemplateError(
            "toolchain script must set compiler[\"name\"]".into(),
        ));
    }

    Ok(def)
}

// ── Engine factory ────────────────────────────────────────────────────────────

fn make_engine() -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(100_000);

    // ── compiler map ─────────────────────────────────────────────────────────
    e.register_type_with_name::<CompilerMap>("Compiler");
    e.register_indexer_set(|_: &mut CompilerMap, key: ImmutableString, val: Dynamic| {
        with_def(|d| match key.as_str() {
            "name"               => { if let Some(s) = val.try_cast::<String>() { d.name = s; } }
            "binary"             => { if let Some(s) = val.try_cast::<String>() { d.binary = s; } }
            "version_arg"        => { if let Some(s) = val.try_cast::<String>() { d.version_arg = s; } }
            "version_regex"      => { if let Some(s) = val.try_cast::<String>() { d.version_regex = s; } }
            "extensions"         => { if let Some(a) = val.try_cast::<Array>() { d.extensions = strings_from(a); } }
            "always_flags"       => { if let Some(a) = val.try_cast::<Array>() { d.always_flags = strings_from(a); } }
            "passthrough"        => { if let Some(b) = val.try_cast::<bool>() { d.passthrough_enabled = b; } }
            "passthrough_prefix" => { if let Some(s) = val.try_cast::<String>() { d.passthrough_prefix = s; } }
            "min_version"        => { if let Some(s) = val.try_cast::<String>() { d.min_version = Some(s); } }
            "supported_archs"    => { if let Some(a) = val.try_cast::<Array>() { d.supported_archs = strings_from(a); } }
            "supported_os"       => { if let Some(a) = val.try_cast::<Array>() { d.supported_os = strings_from(a); } }
            "required_tools"     => { if let Some(a) = val.try_cast::<Array>() { d.required_tools = strings_from(a); } }
            "required_env"       => { if let Some(a) = val.try_cast::<Array>() { d.required_env = strings_from(a); } }
            "requires_toolchain" => { if let Some(a) = val.try_cast::<Array>() { d.requires_toolchain = strings_from(a); } }
            "homepage"           => {} // informational only
            _ => {}
        });
    });
    e.register_indexer_get(|_: &mut CompilerMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── flags map ─────────────────────────────────────────────────────────────
    // get returns a copy of the inner map so chained assignment write-back works:
    //   flags["debug"]["true"] = "-g"
    //   → get("debug") → Map  →  map["true"] = "-g"  →  set("debug", map)
    e.register_type_with_name::<FlagsMap>("FlagsMap");
    e.register_indexer_get(|_: &mut FlagsMap, cat: ImmutableString| -> Dynamic {
        CURRENT.with(|c| {
            c.borrow().as_ref()
                .and_then(|d| d.flags.get(cat.as_str()))
                .map(|inner| {
                    let m: Map = inner.iter()
                        .map(|(k, v)| (k.as_str().into(), Dynamic::from(v.clone())))
                        .collect();
                    Dynamic::from(m)
                })
                .unwrap_or_else(|| Dynamic::from(Map::new()))
        })
    });
    e.register_indexer_set(|_: &mut FlagsMap, cat: ImmutableString, val: Dynamic| {
        if let Some(inner) = val.try_cast::<Map>() {
            let entries: HashMap<String, String> = inner.into_iter()
                .filter_map(|(k, v)| v.try_cast::<String>().map(|s| (k.to_string(), s)))
                .collect();
            with_def(|d| *d.flags.entry(cat.to_string()).or_default() = entries);
        }
    });

    // ── standards map ─────────────────────────────────────────────────────────
    e.register_type_with_name::<StandardsMap>("StandardsMap");
    e.register_indexer_set(|_: &mut StandardsMap, key: ImmutableString, val: String| {
        with_def(|d| { d.standards.insert(key.to_string(), val); });
    });
    e.register_indexer_get(|_: &mut StandardsMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── structure map ─────────────────────────────────────────────────────────
    e.register_type_with_name::<StructureMap>("StructureMap");
    e.register_indexer_set(|_: &mut StructureMap, key: ImmutableString, val: String| {
        with_def(|d| { d.structure.insert(key.to_string(), val); });
    });
    e.register_indexer_get(|_: &mut StructureMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── modules map ───────────────────────────────────────────────────────────
    e.register_type_with_name::<ModulesMap>("ModulesMap");
    e.register_indexer_set(|_: &mut ModulesMap, key: ImmutableString, val: String| {
        with_def(|d| {
            if key.as_str() == "style" {
                d.module_style = val;
            } else {
                d.module_params.insert(key.to_string(), val);
            }
        });
    });
    e.register_indexer_get(|_: &mut ModulesMap, _: ImmutableString| -> Dynamic {
        Dynamic::UNIT
    });

    // ── linking map ───────────────────────────────────────────────────────────
    e.register_type_with_name::<LinkingMap>("LinkingMap");
    e.register_indexer_set(|_: &mut LinkingMap, lang: ImmutableString, val: Dynamic| {
        if let Some(m) = val.try_cast::<Map>() {
            let lp = extract_linking(m);
            with_def(|d| d.linking.push((lang.to_string(), lp)));
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
    // get returns a copy of the current Array so += write-back works:
    //   load_flags["cc"] += ["-m64"]
    //   → get("cc") → Array  →  array += ["-m64"]  →  set("cc", array)
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

fn strings_from(arr: Array) -> Vec<String> {
    arr.into_iter().filter_map(|v| v.try_cast::<String>()).collect()
}

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
