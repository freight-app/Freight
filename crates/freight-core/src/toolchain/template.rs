use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::FreightError;

// ── Raw deserialization structs (map directly to TOML layout) ─────────────────

#[derive(Debug, Deserialize)]
struct RawTemplate {
    name: String,
    binary: String,
    version_arg: String,
    version_regex: String,
    extensions: RawExtensions,
    flags: RawFlags,
    #[serde(default)]
    standards: HashMap<String, String>,
    structure: RawStructure,
    modules: RawModules,
    passthrough: RawPassthrough,
    /// Per-arch (and optionally per-arch+OS) flags, e.g. NASM output format.
    /// Keys: `"x86_64"` or `"x86_64.linux"` — more specific key wins.
    #[serde(default)]
    arch_flags: HashMap<String, String>,
    #[serde(default)]
    extra: RawExtra,
    #[serde(default)]
    linking: HashMap<String, RawLinking>,
}

#[derive(Debug, Deserialize)]
struct RawExtensions {
    handles: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawFlags {
    opt: HashMap<String, String>,
    debug: String,
    warnings: HashMap<String, String>,
    lto: String,
    #[serde(default)]
    lto_link: String,
    sanitize: String,
    /// Template for per-CPU-extension flags, e.g. `"-m{name}"` → `-mavx2`.
    /// Empty string means the compiler does not support such flags.
    #[serde(default)]
    cpu_extension: String,
}

#[derive(Debug, Deserialize)]
struct RawStructure {
    include_dir: String,
    define: String,
    define_value: String,
    output: String,
    compile_only: String,
    dep_file: String,
    /// Flag template for cross-compilation target triple, e.g. `"--target={triple}"`.
    /// Empty string means this compiler does not support a runtime `--target=` flag
    /// (e.g. GCC, which cross-compiles via a dedicated toolchain binary).
    #[serde(default)]
    target: String,
    /// Flag template for the sysroot path, e.g. `"--sysroot={path}"`.
    /// Empty string means not supported.
    #[serde(default)]
    sysroot: String,
    /// Separate compile-step output flag when it differs from `output`.
    /// e.g. MSVC: `"/Fo{path}"` vs `"-o {path}"` for GCC. Empty = use `output`.
    #[serde(default)]
    output_obj: String,
    /// Separate link-step output flag when it differs from `output`.
    /// e.g. MSVC: `"/Fe{path}"` vs `"-o {path}"` for GCC. Empty = use `output`.
    #[serde(default)]
    output_bin: String,
    /// How the compiler reports included headers.
    /// `"file"` (default) = `-MMD -MF {path}`, `"stdout"` = parse compiler stdout,
    /// `"none"` = no dep tracking.
    #[serde(default)]
    dep_file_mode: String,
    /// Template for linking a system library by name, e.g. `"-l{name}"` (GCC) or
    /// `"{name}.lib"` (MSVC). Defaults to `"-l{name}"` when empty.
    #[serde(default)]
    system_lib: String,
}

#[derive(Debug, Deserialize)]
struct RawModules {
    supported: bool,
    #[serde(default)]
    enable_flag: String,
    #[serde(default)]
    compile_miu: Option<String>,
    #[serde(default)]
    precompile: Option<String>,
    #[serde(default)]
    import_module: Option<String>,
    /// Compiler flag that designates input as a C/C++ header for header-unit precompilation.
    /// e.g. `"-x c++-header"` for clang. Empty string = no header unit support.
    #[serde(default)]
    header_unit_flag: String,
}

#[derive(Debug, Deserialize)]
struct RawPassthrough {
    enabled: bool,
    prefix: String,
}

#[derive(Debug, Default, Deserialize)]
struct RawExtra {
    #[serde(default)]
    always: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawLinking {
    abi: String,
    compatible: Vec<String>,
    #[serde(default)]
    linker: String,
    #[serde(default)]
    extensions: Vec<String>,
    /// Override the template's top-level binary for compiling this language's source files.
    /// E.g. `gcc.toml` has `binary = "g++"` for linking, but C files must be compiled with `gcc`.
    #[serde(default)]
    compile_binary: Option<String>,
}


// ── Builtin template construction types ──────────────────────────────────────

/// Callback type for `compiler_option` / `language_option` handlers.
/// Arguments: (value, compiler_version, host_arch, host_os, compiler_name).
/// Returns extra flags to inject, or `Err(msg)` to abort the build.
pub(super) type OptionHandlerFn =
    fn(&str, &str, &str, &str, &str) -> Result<Vec<String>, String>;

#[derive(Clone, Debug)]
pub(super) struct OptionHandler {
    pub default_value: Option<String>,
    pub callback: OptionHandlerFn,
}

/// Static description of a compiler template.
/// Use `..EMPTY` to fill unset fields, then call `.build()`.
/// Option handlers are passed to `build()` directly so local `fn` items work.
#[derive(Clone, Copy)]
pub(super) struct TemplateDef {
    pub name:            &'static str,
    pub binary:          &'static str,
    pub family:          &'static str,
    pub version_arg:     &'static str,
    pub version_regex:   &'static str,
    pub alias:           Option<&'static str>,
    pub subcommand:      Option<&'static str>,
    pub link_subcommand: Option<&'static str>,
    pub extensions:      &'static [&'static str],
    pub opt_flags:       &'static [(&'static str, &'static str)],
    pub debug:           &'static str,
    pub warning_flags:   &'static [(&'static str, &'static str)],
    pub lto:             &'static str,
    pub lto_link:        &'static str,
    pub sanitize:        &'static str,
    pub sanitizer_options: &'static [&'static str],
    pub cpu_ext:         &'static str,
    pub stdlib_flags:    &'static [(&'static str, &'static str)],
    pub standards:       &'static [(&'static str, &'static str)],
    pub defaults:        &'static [(&'static str, &'static str)],
    pub structure:       &'static [(&'static str, &'static str)],
    pub module_style:    &'static str,
    pub module_params:   &'static [(&'static str, &'static str)],
    pub pch:             &'static [(&'static str, &'static str)],
    pub passthrough_enabled: bool,
    pub passthrough_prefix:  &'static str,
    pub always_flags:    &'static [&'static str],
    pub linking:         &'static [LinkDef],
    pub arch_flags:      &'static [(&'static str, &'static str)],
    pub toolset:         &'static [(&'static str, &'static str)],
    pub requires_toolchain: &'static [&'static str],
    pub required_tools:  &'static [&'static str],
    pub required_env:    &'static [&'static str],
    pub supported_archs: &'static [&'static str],
    pub supported_os:    &'static [&'static str],
    pub min_version:     Option<&'static str>,
    pub kind:            &'static str,
}

/// Linking parameters for one language key within a `TemplateDef`.
#[derive(Clone, Copy)]
pub(super) struct LinkDef {
    pub lang:           &'static str,
    pub abi:            &'static str,
    pub compatible:     &'static [&'static str],
    pub extensions:     &'static [&'static str],
    pub linker:         &'static str,
    pub compile_binary: Option<&'static str>,
}

/// Zero-value `TemplateDef` — spread with `..EMPTY` and override only what you need.
pub(super) const EMPTY: TemplateDef = TemplateDef {
    name: "", binary: "", family: "",
    version_arg: "--version", version_regex: "",
    alias: None, subcommand: None, link_subcommand: None,
    extensions: &[], opt_flags: &[],
    debug: "", warning_flags: &[], lto: "", lto_link: "", sanitize: "",
    sanitizer_options: &[], cpu_ext: "", stdlib_flags: &[],
    standards: &[], defaults: &[], structure: &[],
    module_style: "", module_params: &[], pch: &[],
    passthrough_enabled: false, passthrough_prefix: "",
    always_flags: &[], linking: &[], arch_flags: &[], toolset: &[],
    requires_toolchain: &[], required_tools: &[], required_env: &[],
    supported_archs: &[], supported_os: &[],
    min_version: None, kind: "",
};

impl TemplateDef {
    /// Convert to a fully-owned `CompilerTemplate`.
    /// Pass option handler slices directly so local `fn` items are accepted.
    pub fn build(
        self,
        compiler_options: &[(&str, OptionHandlerFn, Option<&str>)],
        language_options: &[(&str, OptionHandlerFn, Option<&str>)],
    ) -> CompilerTemplate {
        fn s(v: &str) -> String { v.to_string() }
        fn vs(arr: &[&str]) -> Vec<String> { arr.iter().map(|&v| v.to_string()).collect() }
        fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
            pairs.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect()
        }

        let toolset: HashMap<String, String> = map(self.toolset);
        let binary = ["ld", "cxx", "cc"]
            .iter()
            .find_map(|r| toolset.get(*r))
            .cloned()
            .unwrap_or_else(|| s(self.binary));

        let sm = map(self.structure);
        let get = |k: &str| sm.get(k).cloned().unwrap_or_default();
        let fallback   = get("output");
        let output_obj = { let o = get("output_obj"); if !o.is_empty() { o } else { fallback.clone() } };
        let output_bin = { let o = get("output_bin"); if !o.is_empty() { o } else { fallback } };
        let system_lib    = { let v = get("system_lib");    if !v.is_empty() { v } else { "-l{name}".into() } };
        let dep_file_mode = { let v = get("dep_file_mode"); if !v.is_empty() { v } else { "file".into() } };

        let structure = StructureFlags {
            include_dir:  get("include_dir"),
            define:       get("define"),
            define_value: get("define_value"),
            output:       output_obj,
            output_bin,
            compile_only: get("compile_only"),
            dep_file:     get("dep_file"),
            dep_file_mode,
            system_lib,
            target:       get("target"),
            sysroot:      get("sysroot"),
        };

        let pm = map(self.module_params);
        let gm = |k: &str| pm.get(k).cloned().unwrap_or_default();
        let modules = match self.module_style {
            "gcc"   => ModuleStyle::Gcc {
                enable_flag:      gm("enable_flag"),
                compile_miu:      gm("compile_miu"),
                import_module:    gm("import_module"),
                header_unit_flag: gm("header_unit"),
            },
            "clang" => ModuleStyle::Clang {
                precompile:       gm("precompile"),
                import_module:    gm("import_module"),
                header_unit_flag: gm("header_unit"),
            },
            _ => ModuleStyle::Unsupported,
        };

        let pm = map(self.pch);
        let gp = |k: &str| pm.get(k).cloned().unwrap_or_default();
        let pch = PchConfig {
            compile:     gp("compile"),
            use_flag:    gp("use"),
            extension:   gp("extension"),
            clangd_flag: gp("clangd_flag"),
        };

        let linking = self.linking.iter().map(|ld| {
            (s(ld.lang), LinkingInfo {
                abi:            s(ld.abi),
                compatible:     vs(ld.compatible),
                linker:         s(ld.linker),
                extensions:     vs(ld.extensions),
                compile_binary: ld.compile_binary.map(s),
            })
        }).collect();

        let compiler_option_handlers = compiler_options.iter()
            .map(|&(k, f, d)| (s(k), OptionHandler { callback: f, default_value: d.map(s) }))
            .collect();
        let language_option_handlers = language_options.iter()
            .map(|&(k, f, d)| (s(k), OptionHandler { callback: f, default_value: d.map(s) }))
            .collect();

        CompilerTemplate {
            name:               s(self.name),
            family:             s(self.family),
            alias:              self.alias.map(s),
            sanitizer_options:  vs(self.sanitizer_options),
            binary,
            version_arg:        s(self.version_arg),
            version_regex:      s(self.version_regex),
            extensions:         vs(self.extensions),
            standards:          map(self.standards),
            defaults:           map(self.defaults),
            kind:               s(self.kind),
            structure,
            modules,
            passthrough: PassthroughConfig {
                enabled: self.passthrough_enabled,
                prefix:  s(self.passthrough_prefix),
            },
            always_flags:       vs(self.always_flags),
            subcommand:         self.subcommand.map(s),
            link_subcommand:    self.link_subcommand.map(s),
            supported_archs:    vs(self.supported_archs),
            supported_os:       vs(self.supported_os),
            required_tools:     vs(self.required_tools),
            required_env:       vs(self.required_env),
            min_version:        self.min_version.map(s),
            requires_toolchain: vs(self.requires_toolchain),
            arch_flags:         map(self.arch_flags),
            toolset,
            pch,
            linking,
            compiler_option_handlers,
            language_option_handlers,
            flags_opt:          map(self.opt_flags),
            flags_debug:        s(self.debug),
            flags_warnings:     map(self.warning_flags),
            flags_lto:          s(self.lto),
            flags_lto_link:     s(self.lto_link),
            flags_sanitize:     s(self.sanitize),
            flags_cpu_extension: s(self.cpu_ext),
            flags_stdlib:       map(self.stdlib_flags),
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// ABI and linking compatibility declared by a compiler template.
///
/// The `linking` map on `CompilerTemplate` is keyed by the language key used in
/// `[language.X]` sections of `freight.toml` (e.g. `"cpp"`, `"cuda"`). Each entry
/// describes what ABI the compiler's output conforms to and which other ABIs it can
/// be linked against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkingInfo {
    /// The ABI label this compiler's output conforms to (e.g. `"c++"`, `"cuda"`).
    pub abi: String,
    /// ABI labels whose output can be linked into a binary alongside this one.
    pub compatible: Vec<String>,
    /// If non-empty, the ABI label of the compiler that must perform the final link step.
    /// For example CUDA device code sets this to `"c++"` so the host C++ compiler drives linking.
    pub linker: String,
    /// File extensions handled by this language key (e.g. `[".cpp", ".cc"]` for `"cpp"`).
    /// Used by source discovery to assign each source file to the right language.
    pub extensions: Vec<String>,
    /// Binary name to use when *compiling* (not linking) source files of this language.
    /// Overrides the template's top-level `binary` for the compile step only.
    /// E.g. gcc.toml uses `g++` as the linker binary but `gcc` to compile `.c` files.
    pub compile_binary: Option<String>,
}

/// Settings drawn from `freight.toml` (or a profile) used to produce compiler flags.
#[derive(Debug, Clone)]
pub struct BuildSettings {
    /// "0" | "1" | "2" | "3" | "s" | "z"
    pub opt_level: String,
    pub debug: bool,
    /// "none" | "default" | "all" | "error"
    pub warnings: String,
    pub lto: bool,
    pub strip: bool,
    pub sanitize: Vec<String>,
    pub standard: Option<String>,
    pub defines: Vec<String>,
    pub include_paths: Vec<PathBuf>,
    pub extra_flags: Vec<String>,
    /// Cross-compilation target triple (e.g. `"aarch64-linux-gnu"`).
    /// `None` means native/host build.
    pub target_triple: Option<String>,
    /// Sysroot for cross-compilation (`--sysroot=...`).
    pub sysroot: Option<PathBuf>,
    /// Host (or target) CPU architecture used for `[arch_flags]` lookup in templates.
    /// Defaults to `std::env::consts::ARCH`; overridden by `[target] arch` in freight.toml.
    pub arch: String,
    /// CPU extension names (e.g. `["avx2", "fma"]`) that generate `-m<name>` flags
    /// via the template's `cpu_extension` field.
    pub cpu_extensions: Vec<String>,
    /// C++ standard library: `"libc++"` | `"libstdc++"` | `"none"` | `""` (default/unset).
    pub stdlib: String,
    /// Whether freight may derive CPU tuning flags from `target_triple` + `sysroot`.
    pub auto_cpu_tuning: bool,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            opt_level: "2".into(),
            debug: false,
            warnings: "all".into(),
            lto: false,
            strip: false,
            sanitize: vec![],
            standard: None,
            defines: vec![],
            include_paths: vec![],
            extra_flags: vec![],
            target_triple: None,
            sysroot: None,
            arch: std::env::consts::ARCH.to_string(),
            cpu_extensions: vec![],
            stdlib: String::new(),
            auto_cpu_tuning: true,
        }
    }
}

/// Module compilation strategy differs between GCC and Clang.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModuleStyle {
    /// GCC: single step — produces both `.pcm` and `.o`.
    Gcc {
        enable_flag: String,
        compile_miu: String,
        import_module: String,
        /// Non-empty → header unit precompilation is supported (`-fmodule-header`).
        /// Requires GCC 12+.
        header_unit_flag: String,
    },
    /// Clang: two steps — `--precompile` then compile.
    Clang {
        precompile: String,
        import_module: String,
        /// Non-empty → header unit precompilation is supported (`-x c++-header`).
        header_unit_flag: String,
    },
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureFlags {
    pub include_dir: String,
    pub define: String,
    pub define_value: String,
    /// Compile-step output flag (set from `output_obj`, falling back to `output`).
    pub output: String,
    /// Link-step output flag (set from `output_bin`, falling back to `output`).
    pub output_bin: String,
    pub compile_only: String,
    pub dep_file: String,
    /// `"file"` (default) | `"stdout"` (MSVC /showIncludes) | `"none"`.
    pub dep_file_mode: String,
    /// System library link flag template, e.g. `"-l{name}"` (GCC) or `"{name}.lib"` (MSVC).
    pub system_lib: String,
    /// `"--target={triple}"` or empty if unsupported.
    pub target: String,
    /// `"--sysroot={path}"` or empty if unsupported.
    pub sysroot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassthroughConfig {
    pub enabled: bool,
    pub prefix: String,
}

/// Precompiled header (PCH) configuration for a compiler template.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PchConfig {
    /// Flag(s) to compile a header as a PCH, e.g. `"-x c++-header"`.
    pub compile: String,
    /// Flag template to inject the PCH into consumers.
    /// Placeholders: `{header_path}` = original header, `{pch_path}` = compiled PCH output.
    pub use_flag: String,
    /// File extension for the PCH output, e.g. `".pch"` or `".gch"`.
    pub extension: String,
    /// Flag injected into `compile_commands.json` for IDE/clangd consumers.
    /// Clangd needs the *source* header, not the opaque binary PCH.
    /// Placeholder: `{header_path}`. Defaults to `"-include {header_path}"` when empty.
    pub clangd_flag: String,
}

/// A fully-parsed compiler template loaded from a `.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilerTemplate {
    pub name: String,
    /// Compiler family label used for same-family selection when `backend = "auto"`.
    /// E.g. `"gnu"` for GCC/gfortran/gnat, `"llvm"` for Clang/flang, `"intel"` for icpx/ifx.
    /// Empty string = no family preference.
    pub family: String,
    /// Optional alias: another name this template responds to in `[compiler.<alias>]` sections.
    /// E.g. `clang++` sets `alias = "clang"` so `[compiler.clang]` applies to both.
    pub alias: Option<String>,
    /// Sanitizer names this compiler supports (e.g. `"address"`, `"undefined"`).
    /// Empty = no declaration (don't validate — assume all pass through).
    pub sanitizer_options: Vec<String>,
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    pub extensions: Vec<String>,
    pub standards: HashMap<String, String>,
    /// Fallback option values used when the manifest doesn't specify them.
    /// Keyed by option name (e.g. `"std"`, `"stdlib"`). Set in language files.
    pub defaults: HashMap<String, String>,
    /// `"debugger"` if this template describes a debugger; `""` for compiler templates.
    /// Used by `load_templates` to skip non-compiler files.
    pub kind: String,
    pub structure: StructureFlags,
    pub modules: ModuleStyle,
    pub passthrough: PassthroughConfig,
    pub always_flags: Vec<String>,
    /// Sub-command inserted as the first argument after the binary.
    /// E.g. `zig` uses `subcommand = "cc"` / `"c++"` to dispatch to its embedded compiler.
    #[serde(default)]
    pub subcommand: Option<String>,
    /// Sub-command to use during the link step instead of `subcommand`.
    /// Zig uses `"build-obj"` for compile and `"build-exe"` for link.
    #[serde(default)]
    pub link_subcommand: Option<String>,
    /// Linking metadata keyed by the language key (e.g. `"cpp"`, `"c"`, `"cuda"`).
    /// A template may handle multiple language keys (e.g. gcc handles `"cpp"` and `"c"`).
    pub linking: HashMap<String, LinkingInfo>,

    /// Host architectures on which this toolchain is available (`std::env::consts::ARCH` values).
    /// Empty = no restriction. Used by `detect_all` to skip unavailable toolchains.
    pub supported_archs: Vec<String>,
    /// Host operating systems on which this toolchain is available (`std::env::consts::OS` values).
    /// Empty = no restriction.
    pub supported_os: Vec<String>,
    /// Co-tools that must be present in PATH for this toolchain to function correctly.
    /// If any are absent, `detect_all` warns and skips the toolchain.
    pub required_tools: Vec<String>,
    /// Environment variables that must ALL be set for this toolchain to function.
    /// If any are absent, `detect_all` warns and skips the toolchain.
    pub required_env: Vec<String>,
    /// Minimum acceptable compiler version (e.g. `"12.0"`).
    /// Compared component-by-component; toolchain is skipped when detected version is older.
    pub min_version: Option<String>,
    /// Language ABI keys (e.g. `["cpp"]`) that another detected toolchain must provide.
    /// Guest compilers (nvcc, hipcc, ispc, opencl) use this to declare their host linker dep.
    pub requires_toolchain: Vec<String>,

    /// Per-arch (optionally per-arch+OS) flags. Key `"x86_64.linux"` wins over `"x86_64"`.
    pub arch_flags: HashMap<String, String>,
    /// Toolchain role → binary map (e.g. `"ar"` → `"ar"`, `"cc"` → `"gcc"`).
    pub toolset: HashMap<String, String>,
    /// Precompiled header support configuration.
    pub pch: PchConfig,

    // ── Option handler fields ──────────────────────────────────────────────────
    /// Handlers registered via `compiler_option` in the builtin template.
    #[serde(skip)]
    pub(super) compiler_option_handlers: HashMap<String, OptionHandler>,
    /// Handlers registered via `language_option` in the builtin template.
    #[serde(skip)]
    pub(super) language_option_handlers: HashMap<String, OptionHandler>,

    flags_opt: HashMap<String, String>,
    /// Flag emitted when `debug = true`. Empty = no debug info supported.
    flags_debug: String,
    flags_warnings: HashMap<String, String>,
    /// Flag emitted when `lto = true`. Empty = LTO unsupported.
    flags_lto: String,
    /// Separate LTO flag for the link step (e.g. MSVC `/LTCG` vs compile-step `/GL`).
    /// When empty, `flags_lto` is used for both compile and link.
    flags_lto_link: String,
    flags_sanitize: String,
    /// Template for CPU-extension flags, e.g. `"-m{name}"`. Empty = unsupported.
    flags_cpu_extension: String,
    /// C++ stdlib flags: key is stdlib name (e.g. `"libc++"`) → flag string.
    pub flags_stdlib: HashMap<String, String>,
}

impl CompilerTemplate {
    /// Parse a compiler template from raw TOML bytes.
    pub fn from_toml(src: &str) -> Result<Self, FreightError> {
        let raw: RawTemplate = toml_edit::de::from_str(src)
            .map_err(|e: toml_edit::de::Error| FreightError::TemplateError(e.to_string()))?;

        let modules = build_module_style(raw.modules);

        let linking = raw.linking.into_iter().map(|(key, l)| {
            (key, LinkingInfo {
                abi: l.abi,
                compatible: l.compatible,
                linker: l.linker,
                extensions: l.extensions,
                compile_binary: l.compile_binary,
            })
        }).collect();

        let output_obj = if raw.structure.output_obj.is_empty() {
            raw.structure.output.clone()
        } else {
            raw.structure.output_obj
        };
        let output_bin = if raw.structure.output_bin.is_empty() {
            raw.structure.output.clone()
        } else {
            raw.structure.output_bin
        };

        Ok(Self {
            name: raw.name,
            family: String::new(),
            alias: None,
            sanitizer_options: vec![],
            binary: raw.binary,
            version_arg: raw.version_arg,
            version_regex: raw.version_regex,
            extensions: raw.extensions.handles,
            standards: raw.standards,
            defaults: HashMap::new(),
            kind: String::new(),
            structure: StructureFlags {
                include_dir: raw.structure.include_dir,
                define: raw.structure.define,
                define_value: raw.structure.define_value,
                output: output_obj,
                output_bin,
                compile_only: raw.structure.compile_only,
                dep_file: raw.structure.dep_file,
                dep_file_mode: if raw.structure.dep_file_mode.is_empty() {
                    "file".to_string()
                } else {
                    raw.structure.dep_file_mode
                },
                system_lib: if raw.structure.system_lib.is_empty() {
                    "-l{name}".to_string()
                } else {
                    raw.structure.system_lib
                },
                target: raw.structure.target,
                sysroot: raw.structure.sysroot,
            },
            modules,
            passthrough: PassthroughConfig {
                enabled: raw.passthrough.enabled,
                prefix: raw.passthrough.prefix,
            },
            always_flags: raw.extra.always,
            subcommand: None,
            link_subcommand: None,
            supported_archs: vec![],
            supported_os: vec![],
            required_tools: vec![],
            required_env: vec![],
            min_version: None,
            requires_toolchain: vec![],
            arch_flags: raw.arch_flags,
            toolset: HashMap::new(),
            pch: PchConfig::default(),
            linking,
            compiler_option_handlers: HashMap::new(),
            language_option_handlers: HashMap::new(),
            flags_opt: raw.flags.opt,
            flags_debug: raw.flags.debug,
            flags_warnings: raw.flags.warnings,
            flags_lto: raw.flags.lto,
            flags_lto_link: raw.flags.lto_link,
            flags_sanitize: raw.flags.sanitize,
            flags_cpu_extension: raw.flags.cpu_extension,
            flags_stdlib: HashMap::new(),
        })
    }

    /// Assemble a flat list of compiler flags from abstract build settings.
    /// Pure function — no I/O, no side effects.
    pub fn assemble_flags(&self, settings: &BuildSettings) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();

        // Optimization
        if let Some(f) = self.flags_opt.get(&settings.opt_level) {
            push_flag_str(&mut flags, f);
        }

        // Debug
        if settings.debug && !self.flags_debug.is_empty() {
            push_flag_str(&mut flags, &self.flags_debug);
        }

        // Warnings
        if let Some(f) = self.flags_warnings.get(&settings.warnings) {
            push_flag_str(&mut flags, f);
        }

        // LTO
        if settings.lto && !self.flags_lto.is_empty() {
            push_flag_str(&mut flags, &self.flags_lto);
        }

        // Sanitizers
        if !settings.sanitize.is_empty() && !self.flags_sanitize.is_empty() {
            let active: Vec<&str> = if self.sanitizer_options.is_empty() {
                settings.sanitize.iter().map(|s| s.as_str()).collect()
            } else {
                let mut active = Vec::new();
                for s in &settings.sanitize {
                    if self.sanitizer_options.contains(s) {
                        active.push(s.as_str());
                    } else {
                        eprintln!(
                            "warning: sanitizer '{}' is not supported by '{}', skipping",
                            s, self.name,
                        );
                    }
                }
                active
            };
            if !active.is_empty() {
                let flag = self.flags_sanitize.replace("{values}", &active.join(","));
                push_flag_str(&mut flags, &flag);
            }
        }

        // Language standard — manifest setting, then template default, then nothing.
        let effective_std = settings.standard.as_deref()
            .or_else(|| self.defaults.get("std").map(String::as_str));
        if let Some(std) = effective_std {
            if let Some(f) = self.standards.get(std) {
                push_flag_str(&mut flags, f);
            }
        }

        // Module enable flag (GCC only)
        if let ModuleStyle::Gcc { enable_flag, .. } = &self.modules {
            push_flag_str(&mut flags, enable_flag);
        }

        // Defines
        for def in &settings.defines {
            if let Some((name, value)) = def.split_once('=') {
                let f = self.structure.define_value
                    .replace("{name}", name)
                    .replace("{value}", value);
                flags.push(f);
            } else {
                let f = self.structure.define.replace("{name}", def);
                flags.push(f);
            }
        }

        // Include paths
        for path in &settings.include_paths {
            let f = self.structure.include_dir
                .replace("{path}", &path.to_string_lossy());
            flags.push(f);
        }

        // Compiler-level extra flags (e.g. nvcc always flags)
        for f in &self.always_flags {
            flags.push(f.clone());
        }

        // User extra flags (passthrough-wrapped if needed)
        for f in &settings.extra_flags {
            if self.passthrough.enabled && !self.passthrough.prefix.is_empty() {
                flags.push(self.passthrough.prefix.clone());
            }
            flags.push(f.clone());
        }

        // Cross-compilation target triple (e.g. `--target=aarch64-linux-gnu`)
        if let Some(triple) = &settings.target_triple {
            if !self.structure.target.is_empty() {
                let f = self.structure.target.replace("{triple}", triple);
                push_flag_str(&mut flags, &f);
            }
        }

        // Sysroot (e.g. `--sysroot=/opt/sysroot`)
        if let Some(sysroot) = &settings.sysroot {
            if !self.structure.sysroot.is_empty() {
                let f = self.structure.sysroot
                    .replace("{path}", &sysroot.to_string_lossy());
                push_flag_str(&mut flags, &f);
            }
        }

        for f in self.derived_target_cpu_flags(settings, &flags) {
            push_flag_str(&mut flags, &f);
        }

        // Arch flags (e.g. NASM output format: `-f elf64`).
        // Try arch.os first, then arch alone.
        if !self.arch_flags.is_empty() {
            let os = std::env::consts::OS;
            let arch_os = format!("{}.{os}", settings.arch);
            let arch_flag = self.arch_flags.get(&arch_os)
                .or_else(|| self.arch_flags.get(&settings.arch))
                .map(|s| s.as_str())
                .unwrap_or("");
            push_flag_str(&mut flags, arch_flag);
        }

        // C++ stdlib flag — manifest setting, then template default, then nothing.
        let effective_stdlib = if !settings.stdlib.is_empty() {
            Some(settings.stdlib.as_str())
        } else {
            self.defaults.get("stdlib").map(String::as_str)
        };
        if let Some(stdlib) = effective_stdlib {
            if let Some(f) = self.flags_stdlib.get(stdlib) {
                push_flag_str(&mut flags, f);
            }
        }

        // CPU extension flags (e.g. `-mavx2`, `-mfma`).
        if !self.flags_cpu_extension.is_empty() {
            for ext in &settings.cpu_extensions {
                let f = self.flags_cpu_extension.replace("{name}", ext);
                push_flag_str(&mut flags, &f);
            }
        }

        flags
    }

    fn derived_target_cpu_flags(&self, settings: &BuildSettings, existing_flags: &[String]) -> Vec<String> {
        if !settings.auto_cpu_tuning || !self.accepts_gnu_cpu_tuning_flags() {
            return vec![];
        }
        if existing_flags.iter().chain(settings.extra_flags.iter()).any(|f| is_cpu_tuning_flag(f)) {
            return vec![];
        }
        let Some(target) = settings.target_triple.as_deref() else { return vec![] };
        let Some(sysroot) = settings.sysroot.as_ref() else { return vec![] };
        let target = target.to_ascii_lowercase();
        let sysroot = sysroot.to_string_lossy().to_ascii_lowercase();
        if self.structure.target.is_empty() && !target_matches_host(&target) {
            return vec![];
        }

        if target.starts_with("aarch64") {
            if let Some(cpu) = first_sysroot_token(&sysroot, &[
                "neoverse-v2", "neoverse-v1", "neoverse-n2", "neoverse-n1",
                "cortex-a78", "cortex-a76", "cortex-a75", "cortex-a73",
                "cortex-a72", "cortex-a57", "cortex-a55", "cortex-a53",
            ]) {
                return vec![format!("-mcpu={cpu}")];
            }
            return vec!["-march=armv8-a".to_string()];
        }

        if target.starts_with("arm") || target.starts_with("thumb") {
            let mut flags = Vec::new();
            if let Some(cpu) = first_sysroot_token(&sysroot, &[
                "cortex-m85", "cortex-m55", "cortex-m35p", "cortex-m33", "cortex-m23",
                "cortex-m7", "cortex-m4", "cortex-m3", "cortex-m0plus", "cortex-m0",
                "cortex-a9", "cortex-a8", "cortex-a7", "cortex-r5", "cortex-r4",
            ]) {
                flags.push(format!("-mcpu={cpu}"));
                if cpu.starts_with("cortex-m") {
                    flags.push("-mthumb".to_string());
                }
            }
            if sysroot.contains("eabihf") || sysroot.contains("hardfloat") || sysroot.contains("hard-float") {
                flags.push("-mfloat-abi=hard".to_string());
            } else if sysroot.contains("softfp") {
                flags.push("-mfloat-abi=softfp".to_string());
            }
            return flags;
        }

        if target.starts_with("riscv64") {
            let mut flags = Vec::new();
            let march = first_sysroot_token(&sysroot, &["rv64gcv", "rv64gc", "rv64imafdc", "rv64imac"])
                .unwrap_or("rv64gc");
            flags.push(format!("-march={march}"));
            let abi = first_sysroot_token(&sysroot, &["lp64d", "lp64f", "lp64"])
                .unwrap_or("lp64d");
            flags.push(format!("-mabi={abi}"));
            return flags;
        }

        if target.starts_with("x86_64") {
            if let Some(march) = first_sysroot_token(&sysroot, &[
                "x86-64-v4", "x86-64-v3", "x86-64-v2", "znver4", "znver3", "znver2",
                "skylake-avx512", "skylake", "haswell",
            ]) {
                return vec![format!("-march={march}")];
            }
        }

        vec![]
    }

    fn accepts_gnu_cpu_tuning_flags(&self) -> bool {
        let handles_c_or_cpp = self.linking.contains_key("c") || self.linking.contains_key("cpp");
        handles_c_or_cpp
            && (self.family == "gnu"
                || self.family == "llvm"
                || self.family == "intel"
                || matches!(self.name.as_str(), "gcc" | "g++" | "clang" | "clang++" | "icpx"))
    }

    /// Format the `-o <path>` flag pair.
    pub fn output_flag(&self, path: &std::path::Path) -> Vec<String> {
        let s = self.structure.output.replace("{path}", &path.to_string_lossy());
        s.split_whitespace().map(str::to_owned).collect()
    }

    /// The `-c` compile-only flag.
    pub fn compile_only_flag(&self) -> Vec<String> {
        push_flag_parts(&self.structure.compile_only)
    }

    /// Flags to generate a Makefile dependency file alongside compilation.
    /// Returns an empty Vec if the template does not support dep files.
    pub fn dep_file_flags(&self, path: &std::path::Path) -> Vec<String> {
        if self.structure.dep_file.is_empty() {
            return vec![];
        }
        self.structure.dep_file
            .replace("{path}", &path.to_string_lossy())
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    /// How this toolchain reports header dependencies: `"file"`, `"stdout"`, or `"none"`.
    pub fn dep_file_mode(&self) -> &str {
        &self.structure.dep_file_mode
    }

    /// Format the output flag for the **link step** (binary or shared lib).
    /// Uses `output_bin` when it differs from the compile-step output (e.g. MSVC `/Fe{path}`).
    pub fn output_bin_flag(&self, path: &std::path::Path) -> Vec<String> {
        let s = self.structure.output_bin.replace("{path}", &path.to_string_lossy());
        s.split_whitespace().map(str::to_owned).collect()
    }

    /// The archiver binary for this toolchain (`toolset["ar"]`, or `"ar"` by default).
    pub fn ar_binary(&self) -> &str {
        self.toolset.get("ar").map(|s| s.as_str()).unwrap_or("ar")
    }

    /// The strip binary for this toolchain (`toolset["strip"]`), if one is defined and non-empty.
    /// Returns `None` for toolchains that have no standalone strip tool (e.g. MSVC).
    pub fn strip_binary(&self) -> Option<&str> {
        self.toolset.get("strip").map(|s| s.as_str()).filter(|s| !s.is_empty())
    }

    /// Format a system-library link flag using this toolchain's template.
    ///
    /// GCC/Clang: `"-lssl"`, MSVC: `"ssl.lib"`.
    pub fn system_lib_flag(&self, name: &str) -> String {
        self.structure.system_lib.replace("{name}", name)
    }

    /// Run `language_option` handlers for the given freeform options map.
    /// `version` is the detected compiler version string passed to each handler.
    /// Returns all flags injected by the handlers via `add_flag()`.
    /// Run `language_option` handlers for the given freeform options map.
    /// Handlers receive a `ctx` object with `value`, `version`, `arch`, `os`, `name`.
    /// Returns injected flags, or `Err` if a handler returns a non-empty error string.
    pub fn run_language_option_handlers(
        &self,
        options: &HashMap<String, String>,
        version: &str,
        arch: &str,
        os: &str,
    ) -> Result<Vec<String>, crate::error::FreightError> {
        run_option_handlers(&self.language_option_handlers, options, version, arch, os, &self.name)
    }

    /// Run `compiler_option` handlers for the given freeform options map.
    /// Handlers receive a `ctx` object with `value`, `version`, `arch`, `os`, `name`.
    /// Returns injected flags, or `Err` if a handler returns a non-empty error string.
    pub fn run_compiler_option_handlers(
        &self,
        options: &HashMap<String, String>,
        version: &str,
        arch: &str,
        os: &str,
    ) -> Result<Vec<String>, crate::error::FreightError> {
        run_option_handlers(&self.compiler_option_handlers, options, version, arch, os, &self.name)
    }

    /// Assemble flags for the **link step**.
    ///
    /// Like `assemble_flags` but uses `lto_link` flags when present (MSVC `/LTCG` at link
    /// time vs `/GL` at compile time). Strips compile-only settings: standard, warnings,
    /// defines, include paths.
    pub fn assemble_link_flags(&self, settings: &BuildSettings) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();

        if let Some(f) = self.flags_opt.get(&settings.opt_level) {
            push_flag_str(&mut flags, f);
        }

        if settings.debug && !self.flags_debug.is_empty() {
            push_flag_str(&mut flags, &self.flags_debug);
        }

        let lto_f = if !self.flags_lto_link.is_empty() { &self.flags_lto_link } else { &self.flags_lto };
        if settings.lto && !lto_f.is_empty() {
            push_flag_str(&mut flags, lto_f);
        }

        for f in &self.always_flags {
            flags.push(f.clone());
        }

        if let Some(triple) = &settings.target_triple {
            if !self.structure.target.is_empty() {
                let f = self.structure.target.replace("{triple}", triple);
                push_flag_str(&mut flags, &f);
            }
        }

        if let Some(sysroot) = &settings.sysroot {
            if !self.structure.sysroot.is_empty() {
                let f = self.structure.sysroot
                    .replace("{path}", &sysroot.to_string_lossy());
                push_flag_str(&mut flags, &f);
            }
        }

        // stdlib and runtime flags are also needed at link time.
        if !settings.stdlib.is_empty() {
            if let Some(f) = self.flags_stdlib.get(&settings.stdlib) {
                push_flag_str(&mut flags, f);
            }
        }

        flags
    }

    /// Whether this template supports C++20 header unit precompilation.
    pub fn supports_header_units(&self) -> bool {
        match &self.modules {
            ModuleStyle::Clang { header_unit_flag, .. } => !header_unit_flag.is_empty(),
            ModuleStyle::Gcc   { header_unit_flag, .. } => !header_unit_flag.is_empty(),
            ModuleStyle::Unsupported => false,
        }
    }

    /// Build the compiler invocation for precompiling a header as a C++20 header unit.
    ///
    /// Returns `(binary, args)` or `None` when unsupported.
    /// `std_flag` is the already-resolved standard flag (e.g. `"-std=c++20"`).
    /// `include_flags` are already-formatted `-I` flags.
    ///
    /// Clang: `clang++ {std} {includes} --precompile -x c++-header {header} -o {pcm}`
    /// GCC:   `g++    {std} -fmodules-ts -fmodule-header {includes} {header} -o {pcm}`
    pub fn precompile_header_unit_cmd(
        &self,
        header_abs: &std::path::Path,
        pcm_path: &std::path::Path,
        std_flag: &str,
        include_flags: &[String],
    ) -> Option<(std::path::PathBuf, Vec<String>)> {
        let mut args: Vec<String> = Vec::new();
        match &self.modules {
            ModuleStyle::Clang { precompile, header_unit_flag, .. } => {
                if header_unit_flag.is_empty() { return None; }
                push_flag_str(&mut args, std_flag);
                args.extend_from_slice(include_flags);
                push_flag_str(&mut args, precompile);        // --precompile
                push_flag_str(&mut args, header_unit_flag);  // -x c++-header
            }
            ModuleStyle::Gcc { enable_flag, header_unit_flag, .. } => {
                if header_unit_flag.is_empty() { return None; }
                push_flag_str(&mut args, std_flag);
                push_flag_str(&mut args, enable_flag);       // -fmodules-ts
                push_flag_str(&mut args, header_unit_flag);  // -fmodule-header
                args.extend_from_slice(include_flags);
            }
            ModuleStyle::Unsupported => return None,
        }
        args.push(header_abs.to_string_lossy().into_owned());
        args.extend(self.output_flag(pcm_path));
        Some((std::path::PathBuf::from(&self.binary), args))
    }

    /// Return the `import_module` flag template for named modules (e.g. `"-fmodule-file={name}={pcm_path}"`).
    /// Returns `""` when the compiler does not support named modules.
    pub fn module_import_template(&self) -> &str {
        match &self.modules {
            ModuleStyle::Gcc   { import_module, .. } => import_module,
            ModuleStyle::Clang { import_module, .. } => import_module,
            ModuleStyle::Unsupported => "",
        }
    }

    /// Format a `-fmodule-file=` import flag for a header unit.
    ///
    /// `rel_path` is the path relative to its include directory, matching
    /// what a consumer writes in `import "rel_path";`.
    pub fn header_unit_import_flag(&self, rel_path: &str, pcm_path: &std::path::Path) -> Option<String> {
        let (import_module, supported) = match &self.modules {
            ModuleStyle::Clang { import_module, header_unit_flag, .. } => (import_module, !header_unit_flag.is_empty()),
            ModuleStyle::Gcc   { import_module, header_unit_flag, .. } => (import_module, !header_unit_flag.is_empty()),
            ModuleStyle::Unsupported => return None,
        };
        if !supported { return None; }
        Some(import_module
            .replace("{name}", rel_path)
            .replace("{pcm_path}", &pcm_path.to_string_lossy()))
    }
}

fn build_module_style(raw: RawModules) -> ModuleStyle {
    if !raw.supported {
        return ModuleStyle::Unsupported;
    }
    if let Some(precompile) = raw.precompile {
        ModuleStyle::Clang {
            precompile,
            import_module: raw.import_module.unwrap_or_default(),
            header_unit_flag: raw.header_unit_flag,
        }
    } else {
        ModuleStyle::Gcc {
            enable_flag: raw.enable_flag,
            compile_miu: raw.compile_miu.unwrap_or_default(),
            import_module: raw.import_module.unwrap_or_default(),
            header_unit_flag: raw.header_unit_flag,
        }
    }
}

fn run_option_handlers(
    handlers: &HashMap<String, OptionHandler>,
    options: &HashMap<String, String>,
    version: &str,
    arch: &str,
    os: &str,
    name: &str,
) -> Result<Vec<String>, crate::error::FreightError> {
    let mut flags = Vec::new();
    for (key, handler) in handlers {
        let value = match options.get(key) {
            Some(v) => v.as_str(),
            None => match handler.default_value.as_deref() {
                Some(d) => d,
                None => continue,
            },
        };
        let result = (handler.callback)(value, version, arch, os, name)
            .map_err(|e| crate::error::FreightError::TemplateError(
                format!("option handler '{key}' failed: {e}")
            ))?;
        flags.extend(result);
    }
    Ok(flags)
}

fn push_flag_str(out: &mut Vec<String>, s: &str) {
    for part in push_flag_parts(s) {
        out.push(part);
    }
}

fn push_flag_parts(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

fn is_cpu_tuning_flag(flag: &str) -> bool {
    matches!(
        flag.split_once('=').map(|(k, _)| k).unwrap_or(flag),
        "-march" | "-mcpu" | "-mtune" | "-mfpu" | "-mfloat-abi" | "-mabi"
    )
}

fn first_sysroot_token<'a>(sysroot: &str, tokens: &[&'a str]) -> Option<&'a str> {
    tokens.iter().copied().find(|token| sysroot.contains(token))
}

fn target_matches_host(target: &str) -> bool {
    let host = std::env::consts::ARCH;
    match host {
        "x86_64" => target.starts_with("x86_64") || target.starts_with("amd64"),
        "aarch64" => target.starts_with("aarch64") || target.starts_with("arm64"),
        "arm" => target.starts_with("arm") || target.starts_with("thumb"),
        "riscv64" => target.starts_with("riscv64"),
        other => target.starts_with(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn gcc() -> CompilerTemplate {
        TemplateDef {
            name: "g++", binary: "g++",
            family: "gnu",
            version_regex: r"(\d+\.\d+\.\d+)",
            extensions: &[".cpp",".cxx",".cc"],
            debug: "-g", lto: "-flto",
            sanitize: "-fsanitize={values}",
            sanitizer_options: &["address","undefined"],
            module_style: "gcc",
            opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os")],
            warning_flags: &[("none",""),("all","-Wall -Wextra -Wpedantic"),("error","-Werror")],
            standards: &[
                ("c++14","-std=c++14"),("c++17","-std=c++17"),
                ("c++20","-std=c++20"),("c++23","-std=c++23"),
            ],
            defaults: &[("std","c++17")],
            structure: &[
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
                ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
                ("sysroot","--sysroot={path}"),
            ],
            module_params: &[
                ("enable_flag","-fmodules-ts"),("compile_miu","-fmodule-output={pcm_path}"),
                ("import_module","-fmodule-file={name}={pcm_path}"),("header_unit","-fmodule-header"),
            ],
            toolset: &[("ar","ar"),("strip","strip")],
            linking: &[LinkDef {
                lang: "cpp", abi: "c++", compatible: &["c","fortran"],
                extensions: &[".cpp",".cxx",".cc"], linker: "", compile_binary: None,
            }],
            ..EMPTY
        }.build(&[], &[])
    }

    fn gcc_c() -> CompilerTemplate {
        TemplateDef {
            name: "gcc", binary: "gcc",
            family: "gnu",
            version_regex: r"(\d+\.\d+\.\d+)",
            extensions: &[".c"],
            debug: "-g", lto: "-flto",
            sanitize: "-fsanitize={values}",
            opt_flags: &[("0","-O0"),("2","-O2"),("3","-O3")],
            warning_flags: &[("all","-Wall -Wextra")],
            standards: &[("c11","-std=c11"),("c17","-std=c17")],
            defaults: &[("std","c11")],
            structure: &[
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
                ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
                ("sysroot","--sysroot={path}"),
            ],
            toolset: &[("ar","ar"),("strip","strip")],
            linking: &[LinkDef {
                lang: "c", abi: "c", compatible: &[],
                extensions: &[".c"], linker: "", compile_binary: Some("gcc"),
            }],
            ..EMPTY
        }.build(&[], &[])
    }

    fn clang() -> CompilerTemplate {
        TemplateDef {
            name: "clang++", binary: "clang++",
            family: "llvm",
            version_regex: r"(\d+\.\d+\.\d+)",
            extensions: &[".cpp",".cxx",".cc"],
            debug: "-g", lto: "-flto",
            sanitize: "-fsanitize={values}",
            module_style: "clang",
            opt_flags: &[("0","-O0"),("2","-O2"),("3","-O3")],
            warning_flags: &[("all","-Wall -Wextra -Wpedantic"),("error","-Werror")],
            standards: &[
                ("c++14","-std=c++14"),("c++17","-std=c++17"),
                ("c++20","-std=c++20"),("c++23","-std=c++23"),
            ],
            defaults: &[("std","c++17")],
            structure: &[
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
                ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
                ("target","--target={triple}"),("sysroot","--sysroot={path}"),
            ],
            module_params: &[
                ("precompile","--precompile"),
                ("import_module","-fmodule-file={name}={pcm_path}"),
                ("header_unit","-x c++-header"),
            ],
            toolset: &[("ar","ar"),("strip","strip")],
            linking: &[
                LinkDef { lang: "cpp", abi: "c++", compatible: &["c","fortran"], extensions: &[".cpp"], linker: "", compile_binary: None },
                LinkDef { lang: "c",   abi: "c",   compatible: &[],             extensions: &[".c"],   linker: "", compile_binary: None },
            ],
            ..EMPTY
        }.build(&[], &[])
    }

    fn nvcc() -> CompilerTemplate {
        TemplateDef {
            name: "nvcc", binary: "nvcc",
            family: "nvidia",
            version_regex: r"release (\d+\.\d+)",
            extensions: &[".cu"],
            passthrough_enabled: true,
            passthrough_prefix: "-Xcompiler",
            always_flags: &["--expt-relaxed-constexpr","--extended-lambda"],
            opt_flags: &[("0","-O0"),("2","-O2"),("3","-O3")],
            structure: &[
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
                ("output","-o {path}"),("compile_only","-c"),
            ],
            linking: &[LinkDef {
                lang: "cuda", abi: "cuda", compatible: &[],
                extensions: &[".cu"], linker: "c++", compile_binary: None,
            }],
            ..EMPTY
        }.build(&[], &[])
    }

    fn msvc() -> CompilerTemplate {
        TemplateDef {
            name: "msvc", binary: "cl.exe",
            version_arg: "",
            version_regex: r"(\d+\.\d+)",
            extensions: &[".cpp",".cxx",".c"],
            debug: "/Zi", lto: "/GL", lto_link: "/LTCG",
            opt_flags: &[("0","/Od"),("2","/O2"),("3","/O2")],
            structure: &[
                ("include_dir","/I{path}"),("define","/D{name}"),("define_value","/D{name}={value}"),
                ("output_obj","/Fo{path}"),("output_bin","/Fe{path}"),("compile_only","/c"),
                ("dep_file_mode","stdout"),("system_lib","{name}.lib"),
            ],
            toolset: &[("ar","lib.exe")],
            linking: &[LinkDef {
                lang: "cpp", abi: "c++", compatible: &["c"],
                extensions: &[".cpp"], linker: "", compile_binary: None,
            }],
            ..EMPTY
        }.build(&[], &[])
    }

    fn gfortran() -> CompilerTemplate {
        TemplateDef {
            name: "gfortran", binary: "gfortran",
            family: "gnu",
            version_regex: r"(\d+\.\d+\.\d+)",
            extensions: &[".f90",".f95",".f03",".f08",".f"],
            debug: "-g", lto: "-flto",
            opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os")],
            warning_flags: &[("none",""),("default",""),("all","-Wall"),("error","-Wall -Werror")],
            standards: &[("f95","-std=f95"),("f2003","-std=f2003"),("f2008","-std=f2008"),("f2018","-std=f2018")],
            defaults: &[("std","f2008")],
            structure: &[
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
                ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
            ],
            toolset: &[("ar","ar"),("strip","strip")],
            linking: &[LinkDef {
                lang: "fortran", abi: "fortran", compatible: &["c"],
                extensions: &[".f90",".f95",".f03",".f08",".f"], linker: "", compile_binary: None,
            }],
            ..EMPTY
        }.build(&[], &[])
    }

    fn dmd() -> CompilerTemplate {
        TemplateDef {
            name: "dmd", binary: "dmd",
            version_regex: r"v(\d+\.\d+\.\d+)",
            extensions: &[".d"],
            debug: "-g",
            opt_flags: &[("0",""),("1","-O"),("2","-O"),("3","-O -release"),("s","-O -release")],
            warning_flags: &[("none",""),("default",""),("all","-wi"),("error","-w")],
            structure: &[
                ("include_dir","-I{path}"),("define","-version={name}"),
                ("output","-of{path}"),("compile_only","-c"),("dep_file_mode","none"),
                ("system_lib","-L-l{name}"),
            ],
            toolset: &[("ar","ar")],
            linking: &[LinkDef {
                lang: "d", abi: "d", compatible: &["c"],
                extensions: &[".d"], linker: "", compile_binary: None,
            }],
            ..EMPTY
        }.build(&[], &[])
    }

    fn nasm() -> CompilerTemplate {
        TemplateDef {
            name: "nasm", binary: "nasm",
            version_regex: r"NASM version (\d+\.\d+(?:\.\d+)?)",
            extensions: &[".asm",".nasm"],
            supported_archs: &["x86","x86_64"],
            requires_toolchain: &["c"],
            debug: "-g -F dwarf",
            opt_flags: &[("0",""),("1",""),("2",""),("3",""),("s",""),("z","")],
            warning_flags: &[("none",""),("default",""),("all","-w+all"),("error","-w+all -w+error")],
            structure: &[
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
                ("output","-o {path}"),("compile_only",""),
            ],
            arch_flags: &[
                ("x86_64.linux","-f elf64"),("x86_64.macos","-f macho64"),("x86_64.windows","-f win64"),
                ("x86.linux","-f elf32"),("x86.macos","-f macho32"),("x86.windows","-f win32"),
            ],
            linking: &[LinkDef {
                lang: "asm", abi: "c", compatible: &["c","cpp"],
                extensions: &[".asm",".nasm"], linker: "", compile_binary: None,
            }],
            toolset: &[("as","nasm")],
            ..EMPTY
        }.build(&[], &[])
    }

    // ── Option handlers ───────────────────────────────────────────────────────

    #[test]
    fn option_handlers_use_registered_defaults() {
        fn mode_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
            Ok(vec![format!("--mode={v}")])
        }
        fn feature_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
            Ok(vec![format!("--feature={v}")])
        }
        fn dialect_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
            Ok(vec![format!("--dialect={v}")])
        }
        let t = TemplateDef {
            name: "toy", binary: "toycc",
            version_regex: "(.*)",
            extensions: &[".toy"],
            structure: &[
                ("output","-o {path}"),("compile_only","-c"),
                ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ],
            ..EMPTY
        }.build(
            &[
                ("mode",    mode_h    as OptionHandlerFn, Some("safe")),
                ("feature", feature_h as OptionHandlerFn, None),
            ],
            &[
                ("dialect", dialect_h as OptionHandlerFn, Some("portable")),
            ],
        );

        let empty = HashMap::new();
        assert_eq!(
            t.run_compiler_option_handlers(&empty, "1.0", "x86_64", "linux").unwrap(),
            vec!["--mode=safe".to_string()],
            "defaulted handler runs; handler without default and no value is skipped"
        );
        assert_eq!(
            t.run_language_option_handlers(&empty, "1.0", "x86_64", "linux").unwrap(),
            vec!["--dialect=portable".to_string()]
        );

        let mut overridden = HashMap::new();
        overridden.insert("mode".to_string(), "fast".to_string());
        overridden.insert("feature".to_string(), "on".to_string());
        let mut flags = t.run_compiler_option_handlers(&overridden, "1.0", "x86_64", "linux").unwrap();
        flags.sort();
        assert_eq!(flags, vec!["--feature=on".to_string(), "--mode=fast".to_string()]);
    }

    #[test]
    fn gcc_cpp_linking() {
        let t = gcc();
        let cpp = t.linking.get("cpp").expect("g++ should have linking.cpp");
        assert_eq!(cpp.abi, "c++");
        assert!(cpp.compatible.contains(&"c".to_string()));
        assert!(cpp.compatible.contains(&"fortran".to_string()));
        assert_eq!(cpp.compile_binary, None, "C++ uses the template's main binary (g++)");
    }

    #[test]
    fn gcc_c_linking() {
        let t = gcc_c();
        let c = t.linking.get("c").expect("gcc should have linking.c");
        assert_eq!(c.abi, "c");
        assert_eq!(c.compile_binary.as_deref(), Some("gcc"),
            "C files must be compiled with gcc, not g++");
    }

    #[test]
    fn nvcc_linking_requires_cpp_linker() {
        let t = nvcc();
        let cuda = t.linking.get("cuda").expect("nvcc should have linking.cuda");
        assert_eq!(cuda.abi, "cuda");
        assert_eq!(cuda.linker, "c++");
    }


    #[test]
    fn gcc_fields() {
        let t = gcc();
        assert_eq!(t.name, "g++");
        assert_eq!(t.binary, "g++");
        assert!(t.extensions.contains(&".cpp".to_string()));
        assert!(t.standards.contains_key("c++20"));

        let tc = gcc_c();
        assert_eq!(tc.name, "gcc");
        assert!(tc.extensions.contains(&".c".to_string()));
        assert!(tc.standards.contains_key("c17"));
    }

    #[test]
    fn gcc_module_style_is_gcc_variant() {
        assert!(matches!(gcc().modules, ModuleStyle::Gcc { .. }));
        if let ModuleStyle::Gcc { enable_flag, compile_miu, import_module, .. } = gcc().modules {
            assert_eq!(enable_flag, "-fmodules-ts");
            assert!(compile_miu.contains("{pcm_path}"));
            assert!(import_module.contains("{pcm_path}"));
        }
    }

    #[test]
    fn clang_module_style_is_clang_variant() {
        assert!(matches!(clang().modules, ModuleStyle::Clang { .. }));
        if let ModuleStyle::Clang { precompile, import_module, .. } = clang().modules {
            assert_eq!(precompile, "--precompile");
            assert!(import_module.contains("{pcm_path}"));
        }
    }

    #[test]
    fn gcc_supports_header_units() {
        let t = gcc();
        assert!(t.supports_header_units(), "gcc template should support header units");
        let (bin, args) = t.precompile_header_unit_cmd(
            std::path::Path::new("/inc/foo.h"),
            std::path::Path::new("/build/foo.h.pcm"),
            "-std=c++20",
            &["-I/inc".to_string()],
        ).expect("should produce a command");
        assert!(bin.to_string_lossy().contains("g++"));
        assert!(args.contains(&"-fmodules-ts".to_string()));
        assert!(args.contains(&"-fmodule-header".to_string()));
        assert!(args.contains(&"-std=c++20".to_string()));
        assert!(args.contains(&"-I/inc".to_string()));
    }

    #[test]
    fn clang_supports_header_units() {
        let t = clang();
        assert!(t.supports_header_units(), "clang template should support header units");
        let (bin, args) = t.precompile_header_unit_cmd(
            std::path::Path::new("/inc/foo.h"),
            std::path::Path::new("/build/foo.h.pcm"),
            "-std=c++20",
            &["-I/inc".to_string()],
        ).expect("should produce a command");
        assert!(bin.to_string_lossy().contains("clang"));
        assert!(args.contains(&"--precompile".to_string()));
        assert!(args.contains(&"-x".to_string()));
        assert!(args.contains(&"c++-header".to_string()));
        assert!(!args.contains(&"-fmodules-ts".to_string()), "clang doesn't need -fmodules-ts");
    }

    #[test]
    fn gcc_and_clang_header_unit_import_flags_match_format() {
        let header = std::path::Path::new("/build/foo.h.pcm");
        let gcc_flag = gcc().header_unit_import_flag("mylib/foo.h", header).unwrap();
        let clang_flag = clang().header_unit_import_flag("mylib/foo.h", header).unwrap();
        assert!(gcc_flag.contains("mylib/foo.h"));
        assert!(gcc_flag.contains("/build/foo.h.pcm"));
        assert_eq!(gcc_flag, clang_flag, "import flag format must match between gcc and clang");
    }

    #[test]
    fn nvcc_module_style_is_unsupported() {
        assert_eq!(nvcc().modules, ModuleStyle::Unsupported);
    }

    #[test]
    fn nvcc_passthrough_and_always_flags() {
        let t = nvcc();
        assert!(t.passthrough.enabled);
        assert_eq!(t.passthrough.prefix, "-Xcompiler");
        assert!(t.always_flags.contains(&"--expt-relaxed-constexpr".to_string()));
        assert!(t.always_flags.contains(&"--extended-lambda".to_string()));
    }

    #[test]
    fn gfortran_has_no_modules() {
        let t = gfortran();
        assert_eq!(t.modules, ModuleStyle::Unsupported);
        assert!(t.extensions.contains(&".f90".to_string()));
    }

    // ── assemble_flags — core profiles ───────────────────────────────────────

    #[test]
    fn dev_profile_flags() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "0".into(),
            debug: true,
            warnings: "all".into(),
            lto: false,
            strip: false,
            sanitize: vec![],
            standard: Some("c++20".into()),
            ..Default::default()
        });
        assert!(flags.contains(&"-O0".to_string()), "opt-level 0");
        assert!(flags.contains(&"-g".to_string()), "debug");
        assert!(flags.contains(&"-Wall".to_string()), "Wall");
        assert!(flags.contains(&"-Wextra".to_string()), "Wextra");
        assert!(flags.contains(&"-Wpedantic".to_string()), "Wpedantic");
        assert!(flags.contains(&"-std=c++20".to_string()), "standard");
        assert!(flags.contains(&"-fmodules-ts".to_string()), "module enable");
        assert!(!flags.contains(&"-flto".to_string()), "no lto");
        assert!(!flags.contains(&"-s".to_string()), "no strip");
    }

    #[test]
    fn release_profile_flags() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "3".into(),
            debug: false,
            warnings: "all".into(),
            lto: true,
            strip: true,
            sanitize: vec![],
            standard: None,
            ..Default::default()
        });
        assert!(flags.contains(&"-O3".to_string()), "opt-level 3");
        assert!(flags.contains(&"-flto".to_string()), "lto");
        // Strip is a post-link step, not a compiler flag — -s must not appear here.
        assert!(!flags.contains(&"-s".to_string()), "strip is post-link, not a compile flag");
        assert!(!flags.contains(&"-g".to_string()), "no debug");
        // Default standard from _cpp.rhai must be applied even when manifest omits it.
        assert!(flags.contains(&"-std=c++17".to_string()), "default c++17 standard");
    }

    #[test]
    fn default_standard_applied_when_not_set() {
        // C++ template defaults to c++17.
        let cpp_flags = gcc().assemble_flags(&BuildSettings { standard: None, ..Default::default() });
        assert!(cpp_flags.contains(&"-std=c++17".to_string()), "g++ default std is c++17");

        // C template defaults to c11.
        let c_flags = gcc_c().assemble_flags(&BuildSettings { standard: None, ..Default::default() });
        assert!(c_flags.contains(&"-std=c11".to_string()), "gcc default std is c11");
    }

    #[test]
    fn manifest_standard_overrides_default() {
        let flags = gcc().assemble_flags(&BuildSettings {
            standard: Some("c++23".into()),
            ..Default::default()
        });
        assert!(flags.contains(&"-std=c++23".to_string()), "explicit standard used");
        assert!(!flags.contains(&"-std=c++17".to_string()), "default not emitted when overridden");
    }

    #[test]
    fn strip_binary_returns_strip_tool() {
        assert_eq!(gcc().strip_binary(), Some("strip"), "gcc toolset should declare strip binary");
        assert_eq!(clang().strip_binary(), Some("strip"), "clang toolset should declare strip binary");
    }

    #[test]
    fn strip_not_in_link_flags() {
        let flags = gcc().assemble_link_flags(&BuildSettings {
            opt_level: "3".into(),
            lto: true,
            strip: true,
            ..Default::default()
        });
        assert!(!flags.contains(&"-s".to_string()), "strip flag must not appear in link flags");
    }

    #[test]
    fn warnings_error_adds_werror() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            warnings: "error".into(),
            ..Default::default()
        });
        assert!(flags.contains(&"-Werror".to_string()));
    }

    #[test]
    fn warnings_none_adds_nothing() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            warnings: "none".into(),
            ..Default::default()
        });
        assert!(!flags.iter().any(|f| f.starts_with("-W")));
    }

    // ── assemble_flags — sanitizers ───────────────────────────────────────────

    #[test]
    fn single_sanitizer() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "0".into(),
            sanitize: vec!["address".into()],
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f == "-fsanitize=address"), "single sanitizer");
    }

    #[test]
    fn multiple_sanitizers_joined() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "0".into(),
            sanitize: vec!["address".into(), "undefined".into()],
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f == "-fsanitize=address,undefined"));
    }

    #[test]
    fn nvcc_sanitize_ignored() {
        // nvcc sanitize template is empty — no flag should appear
        let flags = nvcc().assemble_flags(&BuildSettings {
            opt_level: "0".into(),
            sanitize: vec!["address".into()],
            ..Default::default()
        });
        assert!(!flags.iter().any(|f| f.starts_with("-fsanitize")));
    }

    // ── assemble_flags — defines and includes ────────────────────────────────

    #[test]
    fn plain_define() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            defines: vec!["USE_BLAS".into()],
            ..Default::default()
        });
        assert!(flags.contains(&"-DUSE_BLAS".to_string()));
    }

    #[test]
    fn value_define() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            defines: vec!["VERSION=3".into()],
            ..Default::default()
        });
        assert!(flags.contains(&"-DVERSION=3".to_string()));
    }

    #[test]
    fn include_paths() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            include_paths: vec![PathBuf::from("include/"), PathBuf::from("third_party/include/")],
            ..Default::default()
        });
        assert!(flags.contains(&"-Iinclude/".to_string()));
        assert!(flags.contains(&"-Ithird_party/include/".to_string()));
    }

    // ── assemble_flags — passthrough (nvcc) ───────────────────────────────────

    #[test]
    fn nvcc_passthrough_wraps_extra_flags() {
        let flags = nvcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            extra_flags: vec!["-march=native".into()],
            ..Default::default()
        });
        let idx = flags.iter().position(|f| f == "-Xcompiler")
            .expect("-Xcompiler prefix should be present");
        assert_eq!(flags[idx + 1], "-march=native");
    }

    #[test]
    fn nvcc_always_flags_present_without_extras() {
        let flags = nvcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            ..Default::default()
        });
        assert!(flags.contains(&"--expt-relaxed-constexpr".to_string()));
        assert!(flags.contains(&"--extended-lambda".to_string()));
    }

    #[test]
    fn gcc_extra_flags_not_wrapped() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            extra_flags: vec!["-march=native".into()],
            ..Default::default()
        });
        assert!(flags.contains(&"-march=native".to_string()));
        assert!(!flags.iter().any(|f| f.starts_with("-Xcompiler")));
    }

    // ── assemble_flags — edge cases ───────────────────────────────────────────

    #[test]
    fn unknown_opt_level_omitted() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "99".into(),
            ..Default::default()
        });
        assert!(!flags.iter().any(|f| f.starts_with("-O")));
    }

    #[test]
    fn unknown_standard_omitted() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            standard: Some("c++99".into()),
            ..Default::default()
        });
        assert!(!flags.iter().any(|f| f.starts_with("-std=")));
    }

    #[test]
    fn debug_flag_appears_exactly_once() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "0".into(),
            debug: true,
            ..Default::default()
        });
        assert_eq!(flags.iter().filter(|f| *f == "-g").count(), 1);
    }

    // ── output_flag / compile_only_flag ──────────────────────────────────────

    #[test]
    fn output_flag_splits_correctly() {
        let flags = gcc().output_flag(std::path::Path::new("target/debug/objs/main.o"));
        assert_eq!(flags, vec!["-o", "target/debug/objs/main.o"]);
    }

    #[test]
    fn compile_only_flag_is_dash_c() {
        assert_eq!(gcc().compile_only_flag(), vec!["-c"]);
    }

    // ── cross-compilation — target + sysroot ─────────────────────────────────

    #[test]
    fn clang_emits_target_triple_flag() {
        let t = clang();
        let flags = t.assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            target_triple: Some("aarch64-linux-gnu".into()),
            ..Default::default()
        });
        assert!(
            flags.iter().any(|f| f == "--target=aarch64-linux-gnu"),
            "clang should emit --target= for cross builds, got: {flags:?}"
        );
    }

    #[test]
    fn gcc_does_not_emit_target_flag() {
        // GCC templates do not emit --target=; target-aware compilers like Clang do.
        let t = gcc();
        let flags = t.assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            target_triple: Some("aarch64-linux-gnu".into()),
            ..Default::default()
        });
        assert!(
            !flags.iter().any(|f| f.starts_with("--target")),
            "gcc should NOT emit --target=, got: {flags:?}"
        );
    }

    #[test]
    fn gcc_emits_sysroot_flag() {
        let t = gcc();
        let flags = t.assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            sysroot: Some(PathBuf::from("/opt/sysroot")),
            ..Default::default()
        });
        assert!(
            flags.iter().any(|f| f == "--sysroot=/opt/sysroot"),
            "gcc should emit --sysroot= when sysroot is set, got: {flags:?}"
        );
    }

    #[test]
    fn clang_emits_both_target_and_sysroot() {
        let t = clang();
        let flags = t.assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            target_triple: Some("aarch64-linux-gnu".into()),
            sysroot: Some(PathBuf::from("/opt/arm-sysroot")),
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f == "--target=aarch64-linux-gnu"));
        assert!(flags.iter().any(|f| f == "--sysroot=/opt/arm-sysroot"));
    }

    #[test]
    fn derives_aarch64_cpu_from_target_and_sysroot() {
        let t = clang();
        let flags = t.assemble_flags(&BuildSettings {
            target_triple: Some("aarch64-linux-gnu".into()),
            sysroot: Some(PathBuf::from("/opt/sysroots/cortex-a72-linux-gnu")),
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f == "-mcpu=cortex-a72"), "got: {flags:?}");
    }

    #[test]
    fn derives_riscv_arch_and_abi_from_target_and_sysroot() {
        let t = clang();
        let flags = t.assemble_flags(&BuildSettings {
            target_triple: Some("riscv64-linux-gnu".into()),
            sysroot: Some(PathBuf::from("/opt/sysroots/rv64gcv-lp64d")),
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f == "-march=rv64gcv"), "got: {flags:?}");
        assert!(flags.iter().any(|f| f == "-mabi=lp64d"), "got: {flags:?}");
    }

    #[test]
    fn auto_cpu_tuning_can_be_disabled() {
        let t = clang();
        let flags = t.assemble_flags(&BuildSettings {
            target_triple: Some("aarch64-linux-gnu".into()),
            sysroot: Some(PathBuf::from("/opt/sysroots/cortex-a72-linux-gnu")),
            auto_cpu_tuning: false,
            ..Default::default()
        });
        assert!(!flags.iter().any(|f| f.starts_with("-mcpu") || f.starts_with("-march")), "got: {flags:?}");
    }

    #[test]
    fn manual_cpu_flag_suppresses_auto_cpu_tuning() {
        let t = clang();
        let flags = t.assemble_flags(&BuildSettings {
            target_triple: Some("aarch64-linux-gnu".into()),
            sysroot: Some(PathBuf::from("/opt/sysroots/cortex-a72-linux-gnu")),
            extra_flags: vec!["-march=armv8.2-a".into()],
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f == "-march=armv8.2-a"), "got: {flags:?}");
        assert!(!flags.iter().any(|f| f == "-mcpu=cortex-a72"), "got: {flags:?}");
    }

    #[test]
    fn native_build_emits_no_target_or_sysroot() {
        let flags = gcc().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            target_triple: None,
            sysroot: None,
            ..Default::default()
        });
        assert!(!flags.iter().any(|f| f.starts_with("--target")));
        assert!(!flags.iter().any(|f| f.starts_with("--sysroot")));
    }

    // ── MSVC toolchain ────────────────────────────────────────────────────────

    #[test]
    fn msvc_ar_is_lib_exe() {
        assert_eq!(msvc().ar_binary(), "lib.exe");
    }

    #[test]
    fn msvc_system_lib_flag_uses_dot_lib() {
        assert_eq!(msvc().system_lib_flag("ssl"), "ssl.lib");
        assert_ne!(msvc().system_lib_flag("ssl"), "-lssl");
    }

    #[test]
    fn msvc_dep_file_mode_is_stdout() {
        assert_eq!(msvc().dep_file_mode(), "stdout");
    }

    #[test]
    fn msvc_output_bin_flag_uses_fe() {
        let flags = msvc().output_bin_flag(std::path::Path::new("target/debug/app.exe"));
        assert_eq!(flags, vec!["/Fetarget/debug/app.exe"]);
    }

    #[test]
    fn msvc_output_obj_flag_uses_fo() {
        let flags = msvc().output_flag(std::path::Path::new("target/debug/objs/main.o"));
        assert_eq!(flags, vec!["/Fotarget/debug/objs/main.o"]);
    }

    #[test]
    fn msvc_lto_link_uses_ltcg_not_gl() {
        let settings = BuildSettings { lto: true, ..Default::default() };
        let compile_flags = msvc().assemble_flags(&settings);
        let link_flags   = msvc().assemble_link_flags(&settings);
        // Compile step gets /GL, link step gets /LTCG
        assert!(compile_flags.contains(&"/GL".to_string()), "/GL should be in compile flags");
        assert!(!compile_flags.contains(&"/LTCG".to_string()), "/LTCG must not appear at compile time");
        assert!(link_flags.contains(&"/LTCG".to_string()), "/LTCG should be in link flags");
        assert!(!link_flags.contains(&"/GL".to_string()), "/GL must not appear at link time");
    }

    #[test]
    fn msvc_gcc_default_system_lib_is_dash_l() {
        assert_eq!(gcc().system_lib_flag("pthread"), "-lpthread");
    }

    // ── Fortran ───────────────────────────────────────────────────────────────

    #[test]
    fn gfortran_extensions_and_linking() {
        let t = gfortran();
        assert!(t.extensions.contains(&".f90".to_string()));
        assert!(t.extensions.contains(&".f08".to_string()));
        let link = t.linking.get("fortran").expect("gfortran must have linking.fortran");
        assert_eq!(link.abi, "fortran");
        assert!(link.compatible.contains(&"c".to_string()),
            "Fortran can link against C libraries");
    }

    #[test]
    fn gfortran_default_standard_is_f2008() {
        let flags = gfortran().assemble_flags(&BuildSettings { standard: None, ..Default::default() });
        assert!(flags.contains(&"-std=f2008".to_string()), "gfortran default std is f2008");
    }

    #[test]
    fn gfortran_opt_and_debug_flags() {
        let flags = gfortran().assemble_flags(&BuildSettings {
            opt_level: "2".into(),
            debug: true,
            ..Default::default()
        });
        assert!(flags.contains(&"-O2".to_string()));
        assert!(flags.contains(&"-g".to_string()));
    }

    #[test]
    fn gfortran_no_module_support() {
        assert_eq!(gfortran().modules, ModuleStyle::Unsupported,
            "Fortran uses its own module system, not the C++ module style");
    }

    // ── D language (DMD) ──────────────────────────────────────────────────────

    #[test]
    fn dmd_extensions_and_linking() {
        let t = dmd();
        assert!(t.extensions.contains(&".d".to_string()));
        let link = t.linking.get("d").expect("dmd must have linking.d");
        assert_eq!(link.abi, "d");
        assert!(link.compatible.contains(&"c".to_string()),
            "D can link against C libraries");
    }

    #[test]
    fn dmd_define_uses_version_flag() {
        let flags = dmd().assemble_flags(&BuildSettings {
            defines: vec!["UseSomeFeature".into()],
            ..Default::default()
        });
        assert!(flags.contains(&"-version=UseSomeFeature".to_string()),
            "DMD uses -version= not -D for defines");
    }

    #[test]
    fn dmd_system_lib_uses_linker_passthrough() {
        assert_eq!(dmd().system_lib_flag("curl"), "-L-lcurl",
            "DMD passes linker flags via -L prefix");
    }

    #[test]
    fn dmd_dep_file_mode_is_none() {
        assert_eq!(dmd().dep_file_mode(), "none",
            "DMD does not produce .d dep files (different meaning in D)");
    }

    // ── Assembly (NASM) ───────────────────────────────────────────────────────

    #[test]
    fn nasm_extensions_and_abi() {
        let t = nasm();
        assert!(t.extensions.contains(&".asm".to_string()));
        assert!(t.extensions.contains(&".nasm".to_string()));
        let link = t.linking.get("asm").expect("nasm must have linking.asm");
        assert_eq!(link.abi, "c", "ASM uses C ABI for linking");
        assert!(link.compatible.contains(&"c".to_string()));
        assert!(link.compatible.contains(&"cpp".to_string()));
    }

    #[test]
    fn nasm_arch_flags_for_elf64() {
        let t = nasm();
        // arch_flags are on the template, keyed by "arch.os"
        assert!(t.arch_flags.contains_key("x86_64.linux"),
            "nasm must have an arch flag for x86_64 linux");
        assert_eq!(t.arch_flags["x86_64.linux"], "-f elf64");
        assert_eq!(t.arch_flags["x86_64.macos"], "-f macho64");
        assert_eq!(t.arch_flags["x86_64.windows"], "-f win64");
    }

    #[test]
    fn nasm_debug_uses_dwarf() {
        let flags = nasm().assemble_flags(&BuildSettings {
            debug: true,
            ..Default::default()
        });
        assert!(flags.iter().any(|f| f.contains("dwarf")),
            "NASM debug should include dwarf format, got: {flags:?}");
    }

    // ── Cross-language linking ─────────────────────────────────────────────────

    #[test]
    fn cpp_template_compatible_with_c_and_fortran() {
        let t = gcc();
        let link = t.linking.get("cpp").unwrap();
        assert!(link.compatible.contains(&"c".to_string()),
            "C++ can link C objects");
        assert!(link.compatible.contains(&"fortran".to_string()),
            "C++ can link Fortran objects");
    }

    #[test]
    fn c_template_not_compatible_with_cpp() {
        // C linker cannot link C++ — C++ requires the C++ runtime
        let t = gcc_c();
        let link = t.linking.get("c").unwrap();
        assert!(!link.compatible.contains(&"cpp".to_string()),
            "C linker must not claim C++ compatibility");
        assert!(!link.compatible.contains(&"fortran".to_string()),
            "C linker must not claim Fortran compatibility");
    }

    #[test]
    fn d_compatible_with_c_not_cpp() {
        let t = dmd();
        let link = t.linking.get("d").unwrap();
        assert!(link.compatible.contains(&"c".to_string()),
            "D can link C objects");
        assert!(!link.compatible.contains(&"cpp".to_string()),
            "D does not claim C++ ABI compatibility");
    }

    #[test]
    fn fortran_compatible_with_c_not_cpp() {
        let t = gfortran();
        let link = t.linking.get("fortran").unwrap();
        assert!(link.compatible.contains(&"c".to_string()),
            "Fortran can link C objects");
        assert!(!link.compatible.contains(&"cpp".to_string()),
            "Fortran does not claim C++ ABI compatibility");
    }

    #[test]
    fn asm_compatible_with_both_c_and_cpp() {
        let t = nasm();
        let link = t.linking.get("asm").unwrap();
        assert!(link.compatible.contains(&"c".to_string()),
            "Assembly can be linked by a C linker");
        assert!(link.compatible.contains(&"cpp".to_string()),
            "Assembly can be linked by a C++ linker");
    }

    #[test]
    fn nvcc_cuda_linker_is_cpp() {
        let t = nvcc();
        let link = t.linking.get("cuda").unwrap();
        assert_eq!(link.linker, "c++",
            "CUDA requires a C++ linker for host code");
    }


}
