use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::CraneError;

// ── Raw deserialization structs (map directly to TOML layout) ─────────────────

#[derive(Debug, Deserialize)]
struct RawTemplate {
    name: String,
    binary: String,
    version_arg: String,
    version_regex: String,
    extensions: RawExtensions,
    flags: RawFlags,
    standards: HashMap<String, String>,
    structure: RawStructure,
    modules: RawModules,
    passthrough: RawPassthrough,
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
    debug: HashMap<String, String>,
    warnings: HashMap<String, String>,
    lto: HashMap<String, String>,
    #[serde(default)]
    strip: HashMap<String, String>,
    sanitize: String,
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

// ── Public API ────────────────────────────────────────────────────────────────

/// ABI and linking compatibility declared by a compiler template.
///
/// The `linking` map on `CompilerTemplate` is keyed by the language key used in
/// `[language.X]` sections of `crane.toml` (e.g. `"cpp"`, `"cuda"`). Each entry
/// describes what ABI the compiler's output conforms to and which other ABIs it can
/// be linked against.
#[derive(Debug, Clone)]
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

/// Settings drawn from `crane.toml` (or a profile) used to produce compiler flags.
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
    /// `None` means native/host build. Reserved for the cross-compilation phase.
    pub target_triple: Option<String>,
    /// Sysroot for cross-compilation (`--sysroot=...`).
    /// `None` means use the default sysroot. Reserved for the cross-compilation phase.
    pub sysroot: Option<PathBuf>,
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
        }
    }
}

/// Module compilation strategy differs between GCC and Clang.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleStyle {
    /// GCC: single step — produces both `.pcm` and `.o`.
    Gcc {
        enable_flag: String,
        compile_miu: String,
        import_module: String,
    },
    /// Clang: two steps — `--precompile` then compile.
    Clang {
        precompile: String,
        import_module: String,
    },
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct StructureFlags {
    pub include_dir: String,
    pub define: String,
    pub define_value: String,
    pub output: String,
    pub compile_only: String,
    pub dep_file: String,
    /// `"--target={triple}"` or empty if unsupported.
    pub target: String,
    /// `"--sysroot={path}"` or empty if unsupported.
    pub sysroot: String,
}

#[derive(Debug, Clone)]
pub struct PassthroughConfig {
    pub enabled: bool,
    pub prefix: String,
}

/// A fully-parsed compiler template loaded from a `.toml` file.
#[derive(Debug, Clone)]
pub struct CompilerTemplate {
    pub name: String,
    pub binary: String,
    pub version_arg: String,
    pub version_regex: String,
    pub extensions: Vec<String>,
    pub standards: HashMap<String, String>,
    pub structure: StructureFlags,
    pub modules: ModuleStyle,
    pub passthrough: PassthroughConfig,
    pub always_flags: Vec<String>,
    /// Linking metadata keyed by the language key (e.g. `"cpp"`, `"c"`, `"cuda"`).
    /// A template may handle multiple language keys (e.g. gcc handles `"cpp"` and `"c"`).
    pub linking: HashMap<String, LinkingInfo>,

    flags_opt: HashMap<String, String>,
    flags_debug: HashMap<String, String>,
    flags_warnings: HashMap<String, String>,
    flags_lto: HashMap<String, String>,
    flags_strip: HashMap<String, String>,
    flags_sanitize: String,
}

impl CompilerTemplate {
    /// Parse a compiler template from raw TOML bytes.
    pub fn from_toml(src: &str) -> Result<Self, CraneError> {
        let raw: RawTemplate = toml_edit::de::from_str(src)
            .map_err(|e: toml_edit::de::Error| CraneError::TemplateError(e.to_string()))?;

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

        Ok(Self {
            name: raw.name,
            binary: raw.binary,
            version_arg: raw.version_arg,
            version_regex: raw.version_regex,
            extensions: raw.extensions.handles,
            standards: raw.standards,
            structure: StructureFlags {
                include_dir: raw.structure.include_dir,
                define: raw.structure.define,
                define_value: raw.structure.define_value,
                output: raw.structure.output,
                compile_only: raw.structure.compile_only,
                dep_file: raw.structure.dep_file,
                target: raw.structure.target,
                sysroot: raw.structure.sysroot,
            },
            modules,
            passthrough: PassthroughConfig {
                enabled: raw.passthrough.enabled,
                prefix: raw.passthrough.prefix,
            },
            always_flags: raw.extra.always,
            linking,
            flags_opt: raw.flags.opt,
            flags_debug: raw.flags.debug,
            flags_warnings: raw.flags.warnings,
            flags_lto: raw.flags.lto,
            flags_strip: raw.flags.strip,
            flags_sanitize: raw.flags.sanitize,
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
        let debug_key = if settings.debug { "true" } else { "false" };
        if let Some(f) = self.flags_debug.get(debug_key) {
            push_flag_str(&mut flags, f);
        }

        // Warnings
        if let Some(f) = self.flags_warnings.get(&settings.warnings) {
            push_flag_str(&mut flags, f);
        }

        // LTO
        let lto_key = if settings.lto { "true" } else { "false" };
        if let Some(f) = self.flags_lto.get(lto_key) {
            push_flag_str(&mut flags, f);
        }

        // Strip
        if !self.flags_strip.is_empty() {
            let strip_key = if settings.strip { "true" } else { "false" };
            if let Some(f) = self.flags_strip.get(strip_key) {
                push_flag_str(&mut flags, f);
            }
        }

        // Sanitizers
        if !settings.sanitize.is_empty() && !self.flags_sanitize.is_empty() {
            let values = settings.sanitize.join(",");
            let flag = self.flags_sanitize.replace("{values}", &values);
            push_flag_str(&mut flags, &flag);
        }

        // Language standard
        if let Some(std) = &settings.standard {
            if let Some(f) = self.standards.get(std.as_str()) {
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

        flags
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
}

fn build_module_style(raw: RawModules) -> ModuleStyle {
    if !raw.supported {
        return ModuleStyle::Unsupported;
    }
    if let Some(precompile) = raw.precompile {
        ModuleStyle::Clang {
            precompile,
            import_module: raw.import_module.unwrap_or_default(),
        }
    } else {
        ModuleStyle::Gcc {
            enable_flag: raw.enable_flag,
            compile_miu: raw.compile_miu.unwrap_or_default(),
            import_module: raw.import_module.unwrap_or_default(),
        }
    }
}

fn push_flag_str(out: &mut Vec<String>, s: &str) {
    for part in push_flag_parts(s) {
        out.push(part);
    }
}

fn push_flag_parts(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const GCC_TOML: &str = include_str!("../../../../toolchains/gcc.toml");
    const CLANG_TOML: &str = include_str!("../../../../toolchains/clang.toml");
    const GFORTRAN_TOML: &str = include_str!("../../../../toolchains/gfortran.toml");
    const GNAT_TOML: &str = include_str!("../../../../toolchains/gnat.toml");
    const NVCC_TOML: &str = include_str!("../../../../toolchains/nvcc.toml");
    const DMD_TOML: &str = include_str!("../../../../toolchains/dmd.toml");
    const OPENCL_TOML: &str = include_str!("../../../../toolchains/opencl.toml");
    const HIPCC_TOML: &str = include_str!("../../../../toolchains/hipcc.toml");
    const ICPX_TOML: &str = include_str!("../../../../toolchains/icpx.toml");
    const ISPC_TOML: &str = include_str!("../../../../toolchains/ispc.toml");

    fn gcc() -> CompilerTemplate { CompilerTemplate::from_toml(GCC_TOML).unwrap() }
    fn clang() -> CompilerTemplate { CompilerTemplate::from_toml(CLANG_TOML).unwrap() }
    fn nvcc() -> CompilerTemplate { CompilerTemplate::from_toml(NVCC_TOML).unwrap() }

    // ── Parsing ───────────────────────────────────────────────────────────────

    #[test]
    fn all_templates_parse() {
        CompilerTemplate::from_toml(GCC_TOML).unwrap();
        CompilerTemplate::from_toml(CLANG_TOML).unwrap();
        CompilerTemplate::from_toml(GFORTRAN_TOML).unwrap();
        CompilerTemplate::from_toml(GNAT_TOML).unwrap();
        CompilerTemplate::from_toml(NVCC_TOML).unwrap();
        CompilerTemplate::from_toml(DMD_TOML).unwrap();
        CompilerTemplate::from_toml(OPENCL_TOML).unwrap();
        CompilerTemplate::from_toml(HIPCC_TOML).unwrap();
        CompilerTemplate::from_toml(ICPX_TOML).unwrap();
        CompilerTemplate::from_toml(ISPC_TOML).unwrap();
    }

    #[test]
    fn gcc_linking_declares_cpp_and_c() {
        let t = gcc();
        let cpp = t.linking.get("cpp").expect("gcc should have linking.cpp");
        assert_eq!(cpp.abi, "c++");
        assert!(cpp.compatible.contains(&"c".to_string()));
        assert!(cpp.compatible.contains(&"fortran".to_string()));
        assert_eq!(cpp.compile_binary, None, "C++ uses the template's main binary (g++)");

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
    fn dmd_linking_d_compatible_with_c_and_fortran() {
        let t = CompilerTemplate::from_toml(DMD_TOML).unwrap();
        let d = t.linking.get("d").expect("dmd should have linking.d");
        assert_eq!(d.abi, "d");
        assert!(d.compatible.contains(&"c".to_string()));
        assert!(d.compatible.contains(&"fortran".to_string()));
    }

    #[test]
    fn gcc_fields() {
        let t = gcc();
        assert_eq!(t.name, "gcc");
        assert_eq!(t.binary, "g++");
        assert!(t.extensions.contains(&".cpp".to_string()));
        assert!(t.extensions.contains(&".c".to_string()));
        assert!(t.standards.contains_key("c++20"));
        assert!(t.standards.contains_key("c17"), "gcc handles C standards too");
    }

    #[test]
    fn gcc_module_style_is_gcc_variant() {
        assert!(matches!(gcc().modules, ModuleStyle::Gcc { .. }));
        if let ModuleStyle::Gcc { enable_flag, compile_miu, import_module } = gcc().modules {
            assert_eq!(enable_flag, "-fmodules-ts");
            assert!(compile_miu.contains("{pcm_path}"));
            assert!(import_module.contains("{pcm_path}"));
        }
    }

    #[test]
    fn clang_module_style_is_clang_variant() {
        assert!(matches!(clang().modules, ModuleStyle::Clang { .. }));
        if let ModuleStyle::Clang { precompile, import_module } = clang().modules {
            assert_eq!(precompile, "--precompile");
            assert!(import_module.contains("{pcm_path}"));
        }
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
        let t = CompilerTemplate::from_toml(GFORTRAN_TOML).unwrap();
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
        assert!(flags.contains(&"-s".to_string()), "strip");
        assert!(!flags.contains(&"-g".to_string()), "no debug");
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
        // GCC cross-compiles via dedicated toolchain binary, not --target=.
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
}
