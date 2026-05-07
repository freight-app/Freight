use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rhai::{Array, Dynamic, Engine, Expression, FnPtr, ImmutableString, Map, Scope, AST};

use crate::error::FreightError;

// ── Handler result type ───────────────────────────────────────────────────────

/// Everything produced by evaluating a toolchain Rhai script.
pub(super) struct EvalResult {
    pub def: ToolchainDef,
    pub engine: Engine,
    pub ast: AST,
    pub compiler_option_handlers: HashMap<String, FnPtr>,
    pub language_option_handlers: HashMap<String, FnPtr>,
}

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
    /// Default option values used when the manifest doesn't specify them.
    /// Keys match manifest language/compiler option names (e.g. `"std"`, `"stdlib"`).
    pub defaults: HashMap<String, String>,
    /// `"debugger"` marks the file as a debugger template; `""` = compiler template.
    pub kind: String,
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
    /// Collected `compiler_option(name, fn)` registrations from the current script eval.
    static COLLECTED_COMP_OPTS: RefCell<HashMap<String, FnPtr>> = RefCell::new(HashMap::new());
    /// Collected `language_option(name, fn)` registrations from the current script eval.
    static COLLECTED_LANG_OPTS: RefCell<HashMap<String, FnPtr>> = RefCell::new(HashMap::new());
    /// Flags accumulated by `add_flag(s)` calls inside option handlers at build time.
    static PENDING_FLAGS: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

fn with_def<F: FnOnce(&mut ToolchainDef)>(f: F) {
    CURRENT.with(|cell| {
        if let Some(d) = cell.borrow_mut().as_mut() { f(d); }
    });
}

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        CURRENT.with(|c| *c.borrow_mut() = None);
        COLLECTED_COMP_OPTS.with(|c| c.borrow_mut().clear());
        COLLECTED_LANG_OPTS.with(|c| c.borrow_mut().clear());
    }
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

// ── Script evaluation ─────────────────────────────────────────────────────────

/// Fast text scan that returns the value of the top-level `kind = "..."` assignment
/// without a full Rhai eval. Returns `"compiler"` when no explicit kind is set.
pub(super) fn quick_kind(src: &str) -> String {
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("//") { continue; }
        let Some(rest) = t.strip_prefix("kind") else { continue };
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let v = rest.trim().trim_end_matches(';').trim().trim_matches('"');
        if !v.is_empty() { return v.to_string(); }
    }
    "compiler".to_string()
}

/// Evaluate a Rhai toolchain script. When `dir` is provided, `include()` directives
/// inside the script resolve relative to that directory.
pub(super) fn eval_script(src: &str, dir: Option<&Path>) -> Result<EvalResult, FreightError> {
    let engine = make_engine(dir.map(|d| d.to_path_buf()));

    let ast = engine
        .compile(src)
        .map_err(|e| FreightError::TemplateError(format!("script compile error: {e}")))?;

    CURRENT.with(|c| *c.borrow_mut() = Some(ToolchainDef::default()));
    let _guard = Guard;

    let mut scope = Scope::new();

    // ── Plain variables — identity & constraints ───────────────────────────
    for key in &[
        "name", "homepage", "binary", "version_arg", "version_regex",
        "passthrough_prefix", "min_version", "family", "kind",
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
    for cat in &["opt", "dbg", "warnings", "lto", "lto_link", "stdlib", "runtime", "defaults"] {
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
    def.defaults       = flag_map!("defaults");
    def.kind           = str!("kind");

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

    let compiler_option_handlers = COLLECTED_COMP_OPTS.with(|c| c.borrow_mut().drain().collect());
    let language_option_handlers = COLLECTED_LANG_OPTS.with(|c| c.borrow_mut().drain().collect());

    Ok(EvalResult { def, engine, ast, compiler_option_handlers, language_option_handlers })
}

// ── Handler invocation ────────────────────────────────────────────────────────

/// Call each handler whose name matches a key in `options`.
///
/// Handlers receive a single `ctx` map object with fields:
/// - `ctx.value`   — the value from the manifest
/// - `ctx.version` — detected compiler version string
/// - `ctx.arch`    — effective target architecture
/// - `ctx.os`      — effective target OS
/// - `ctx.name`    — template name (e.g. `"clang++"`)
///
/// A handler returns `""` on success or a non-empty string as an error message.
/// Flags injected via the global `add_flag()` function are collected and returned.
///
/// Returns `Err(FreightError::OptionError)` if any handler returns a non-empty string.
pub(super) fn run_handlers(
    engine: &Engine,
    ast: &AST,
    handlers: &HashMap<String, FnPtr>,
    options: &HashMap<String, String>,
    version: &str,
    arch: &str,
    os: &str,
    name: &str,
) -> Result<Vec<String>, crate::error::FreightError> {
    let mut all_flags = Vec::new();
    for (opt_name, value) in options {
        let Some(handler) = handlers.get(opt_name) else { continue };

        let mut ctx = rhai::Map::new();
        ctx.insert("value".into(),   rhai::Dynamic::from(value.clone()));
        ctx.insert("version".into(), rhai::Dynamic::from(version.to_string()));
        ctx.insert("arch".into(),    rhai::Dynamic::from(arch.to_string()));
        ctx.insert("os".into(),      rhai::Dynamic::from(os.to_string()));
        ctx.insert("name".into(),    rhai::Dynamic::from(name.to_string()));

        PENDING_FLAGS.with(|f| f.borrow_mut().clear());
        let result = handler.call::<rhai::Dynamic>(engine, ast, (rhai::Dynamic::from(ctx),));
        let collected: Vec<String> = PENDING_FLAGS.with(|f| f.borrow().clone());
        all_flags.extend(collected);

        match result {
            Ok(v) => {
                if let Some(msg) = v.try_cast::<String>() {
                    if !msg.is_empty() {
                        return Err(crate::error::FreightError::OptionError(msg));
                    }
                }
            }
            Err(e) => {
                return Err(crate::error::FreightError::TemplateError(
                    format!("option handler '{opt_name}': {e}")
                ));
            }
        }
    }
    Ok(all_flags)
}

// ── Engine factory ────────────────────────────────────────────────────────────

fn make_engine(base_dir: Option<PathBuf>) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(100_000);

    // ── include("path") — engine-native include custom syntax ────────────────
    // Runs the referenced file in the current scope so variable assignments and
    // `let` declarations from the included file are visible to the caller.
    // Paths without an extension get ".rhai" appended automatically.
    if let Some(dir) = base_dir {
        e.register_custom_syntax(
            &["include", "$string$"],
            true,
            move |context: &mut rhai::EvalContext, inputs: &[Expression]| {
                let path_str = inputs[0]
                    .get_string_value()
                    .ok_or_else(|| -> Box<rhai::EvalAltResult> {
                        "include: expected a string literal".into()
                    })?;

                let p = dir.join(path_str);
                let p = if p.extension().is_some() { p } else { p.with_extension("rhai") };

                let src = std::fs::read_to_string(&p)
                    .map_err(|err| -> Box<rhai::EvalAltResult> {
                        format!("include \"{path_str}\": {err}").into()
                    })?;

                // engine() returns &'a Engine — borrow on context ends immediately.
                // scope_mut() can then be called without a conflict.
                let ast = context.engine().compile(&src)
                    .map_err(|err| -> Box<rhai::EvalAltResult> {
                        format!("include \"{path_str}\" compile error: {err}").into()
                    })?;

                let engine = context.engine();
                let scope  = context.scope_mut();
                engine.run_ast_with_scope(scope, &ast)?;

                Ok(Dynamic::UNIT)
            },
        )
        .expect("failed to register include syntax");
    }

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

    // ── compiler_option / language_option ─────────────────────────────────────
    // Called in template scripts to declare per-option callbacks. Handlers are
    // stored in thread-locals during eval, then moved into CompilerTemplate.
    e.register_fn("compiler_option", |name: String, handler: FnPtr| {
        COLLECTED_COMP_OPTS.with(|c| { c.borrow_mut().insert(name, handler); });
    });
    e.register_fn("language_option", |name: String, handler: FnPtr| {
        COLLECTED_LANG_OPTS.with(|c| { c.borrow_mut().insert(name, handler); });
    });

    // ── add_flag ──────────────────────────────────────────────────────────────
    // Called inside option handlers to inject a compiler flag. Accumulates into
    // PENDING_FLAGS which is drained by run_handlers() after each call.
    e.register_fn("add_flag", |flag: String| {
        PENDING_FLAGS.with(|f| f.borrow_mut().push(flag));
    });

    // ── version comparison helpers ────────────────────────────────────────────
    // Compare version strings component-by-component (e.g. "14.1.0" vs "14.0").
    // Suffixes after '-' (e.g. "17.0.6-r1") are ignored.
    e.register_fn("version_gte", |a: String, b: String| version_cmp(&a, &b) != std::cmp::Ordering::Less);
    e.register_fn("version_lte", |a: String, b: String| version_cmp(&a, &b) != std::cmp::Ordering::Greater);
    e.register_fn("version_gt",  |a: String, b: String| version_cmp(&a, &b) == std::cmp::Ordering::Greater);
    e.register_fn("version_lt",  |a: String, b: String| version_cmp(&a, &b) == std::cmp::Ordering::Less);

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

/// Compare two version strings component-by-component.
/// Ignores any suffix after the first `-` (e.g. "17.0.6-r1" → [17, 0, 6]).
/// Missing components are treated as 0 ("14.1" vs "14.1.0" are equal).
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.split('-').next().unwrap_or(s)
         .split('.')
         .filter_map(|c| c.parse().ok())
         .collect()
    };
    let av = parse(a);
    let bv = parse(b);
    let len = av.len().max(bv.len());
    for i in 0..len {
        let ai = av.get(i).copied().unwrap_or(0);
        let bi = bv.get(i).copied().unwrap_or(0);
        match ai.cmp(&bi) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
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
