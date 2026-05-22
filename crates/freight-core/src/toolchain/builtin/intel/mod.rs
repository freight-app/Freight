use crate::toolchain::template::{CompilerTemplate, LinkDef, TemplateDef, EMPTY};

const BASE_INTEL: TemplateDef = TemplateDef {
    family:        "intel",
    debug:  "-g",
    supported_archs: &["x86","x86_64"],
    supported_os:    &["linux","windows"],
    required_env:    &["ONEAPI_ROOT"],
    opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3")],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only","-c"),
    ],
    ..EMPTY
};

const F_EXTS: &[&str] = &[".f90",".f95",".f03",".f08",".f",".F90"];

pub fn icpx() -> CompilerTemplate {
    TemplateDef {
        name: "icpx", binary: "icpx",
        version_regex: r"\b(\d+\.\d+\.\d+)\b",
        extensions: &[".cpp",".cc",".cxx",".c++",".sycl"],
        sanitizer_options: &["address","undefined","thread","leak"],
        always_flags: &["-fsycl"],
        lto: "-flto",
        sanitize: "-fsanitize={values}",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
        warning_flags: &[
            ("none",""),("default","-Wall"),
            ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
        ],
        standards: &[("c++17","-std=c++17"),("c++20","-std=c++20"),("c++23","-std=c++23")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
            ("target","--target={triple}"),("sysroot","--sysroot={path}"),
        ],
        toolset: &[("ld","icpx")],
        linking: &[LinkDef {
            lang: "sycl", abi: "sycl", compatible: &["c++","c","fortran"],
            extensions: &[".sycl",".cpp",".cc",".cxx"], linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_INTEL
    }.build(&[], &[])
}

pub fn ifx() -> CompilerTemplate {
    TemplateDef {
        name: "ifx", binary: "ifx",
        version_regex: r"(\d+\.\d+\.\d+)",
        extensions: F_EXTS,
        sanitizer_options: &["address","undefined"],
        lto: "-ipo",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-O2"),("z","-O2")],
        warning_flags: &[
            ("none","-warn none"),("default",""),
            ("all","-warn all"),("error","-warn all -warn errors"),
        ],
        standards: &[("f95","-std=f95"),("f2003","-std=f2003"),("f2008","-std=f2008"),("f2018","-std=f2018")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-cpp -MMD -MF {path}"),
        ],
        toolset: &[("ar","ar"),("strip","strip"),("ld","ifx")],
        linking: &[LinkDef {
            lang: "fortran", abi: "fortran", compatible: &["c"],
            extensions: F_EXTS, linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_INTEL
    }.build(&[], &[])
}

pub fn ispc() -> CompilerTemplate {
    TemplateDef {
        name: "ispc", binary: "ispc",
        version_regex: r"(\d+\.\d+\.\d+)",
        extensions: &[".ispc"],
        supported_archs: &["x86_64","aarch64"],
        requires_toolchain: &["cpp"],
        debug: "-g",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-O2"),("z","-O2")],
        warning_flags: &[("none","--woff"),("default",""),("all",""),("error","--werror")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only",""),("dep_file","-MMM {path}"),
        ],
        toolset: &[("ld","ispc")],
        linking: &[LinkDef {
            lang: "ispc", abi: "ispc", compatible: &["c++","c"],
            extensions: &[".ispc"], linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..EMPTY
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![icpx(), ifx(), ispc()]
}
