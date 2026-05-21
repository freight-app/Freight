use crate::toolchain::template::{CompilerTemplate, LinkDef, OptionHandlerFn, TemplateDef, EMPTY};

const BASE_NVHPC: TemplateDef = TemplateDef {
    family:        "nvidia",
    version_regex: r"(\d+\.\d+)",
    debug:  "-g",
    cpu_ext: "-m{name}",
    supported_archs: &["x86_64","aarch64"],
    supported_os:    &["linux"],
    opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-O2"),("z","-O2")],
    warning_flags: &[
        ("none",""),("default",""),
        ("all","-Minform=warn"),("error","-Minform=warn -Werror"),
    ],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
    ],
    toolset: &[("ar","ar")],
    ..EMPTY
};

const CPP_EXTS: &[&str] = &[".cpp",".cc",".cxx",".c++"];
const F_EXTS:   &[&str] = &[".f90",".f95",".f",".F90"];

fn sm_arch_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
    if !v.is_empty() { Ok(vec![format!("--gpu-architecture={v}")]) } else { Ok(vec![]) }
}

fn lang_arch_h(v: &str, _: &str, arch: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
    if !v.is_empty() && arch != v {
        Err(format!("nvcc requires arch '{v}' but the effective target is '{arch}'"))
    } else {
        Ok(vec![])
    }
}

pub fn nvcc() -> CompilerTemplate {
    TemplateDef {
        name: "nvcc", binary: "nvcc",
        version_regex: r"release (\d+\.\d+)",
        extensions: &[".cu",".cuh"],
        passthrough_enabled: true,
        passthrough_prefix:  "-Xcompiler",
        always_flags: &["--expt-relaxed-constexpr","--extended-lambda"],
        supported_archs: &["x86_64","aarch64"],
        supported_os:    &["linux","windows"],
        required_tools:  &["ptxas","fatbinary"],
        requires_toolchain: &["cpp"],
        debug: "-g -G",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-O2"),("z","-O2")],
        warning_flags: &[
            ("none","--diag-suppress all"),
            ("default","-Xcompiler -Wall"),
            ("all","-Xcompiler -Wall,-Wextra --Werror cross-execution-space-call,reorder"),
            ("error","-Xcompiler -Wall,-Wextra,-Werror --Werror all-warnings"),
        ],
        standards: &[("c++17","-std=c++17"),("c++20","-std=c++20")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-MD -MF {path}"),
        ],
        toolset: &[("ld","nvcc")],
        linking: &[LinkDef {
            lang: "cuda", abi: "cuda", compatible: &["c++","c","fortran"],
            extensions: &[".cu",".cuh"], linker: "cuda", compile_binary: None,
        }],
        ..EMPTY
    }.build(
        &[("sm_arch", sm_arch_h as OptionHandlerFn, None)],
        &[("arch",    lang_arch_h as OptionHandlerFn, None)],
    )
}

pub fn nvcpp() -> CompilerTemplate {
    TemplateDef {
        name: "nvc++", binary: "nvc++",
        extensions: CPP_EXTS,
        sanitizer_options: &["address"],
        standards: &[("c++17","-std=c++17"),("c++20","-std=c++20")],
        toolset: &[("ar","ar"),("cc","nvc"),("cxx","nvc++"),("ld","nvc++")],
        linking: &[LinkDef {
            lang: "cpp", abi: "c++", compatible: &["c","fortran"],
            extensions: CPP_EXTS, linker: "", compile_binary: None,
        }],
        ..BASE_NVHPC
    }.build(&[], &[])
}

pub fn nvc() -> CompilerTemplate {
    TemplateDef {
        name: "nvc", binary: "nvc",
        extensions: &[".c"],
        sanitizer_options: &["address"],
        standards: &[("c11","-std=c11"),("c17","-std=c17")],
        toolset: &[("ar","ar"),("cc","nvc"),("ld","nvc")],
        linking: &[LinkDef {
            lang: "c", abi: "c", compatible: &["fortran"],
            extensions: &[".c"], linker: "", compile_binary: Some("nvc"),
        }],
        ..BASE_NVHPC
    }.build(&[], &[])
}

pub fn nvfortran() -> CompilerTemplate {
    TemplateDef {
        name: "nvfortran", binary: "nvfortran",
        extensions: F_EXTS,
        sanitizer_options: &["address"],
        stdlib_flags: &[],
        standards: &[("f2003","-Mstandard"),("f2008","-Mstandard"),("f2018","-Mstandard")],
        toolset: &[("ar","ar"),("ld","nvfortran")],
        linking: &[LinkDef {
            lang: "fortran", abi: "fortran", compatible: &["c"],
            extensions: F_EXTS, linker: "", compile_binary: Some("nvfortran"),
        }],
        ..BASE_NVHPC
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![nvcc(), nvcpp(), nvc(), nvfortran()]
}
