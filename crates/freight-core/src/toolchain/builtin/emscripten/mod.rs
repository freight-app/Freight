use crate::toolchain::template::{CompilerTemplate, LinkDef, TemplateDef, EMPTY};

const BASE_EMCC: TemplateDef = TemplateDef {
    version_regex: r"emcc.*?(\d+\.\d+\.\d+)",
    debug: "-g",
    lto:   "-flto",
    supported_archs: &["x86_64","aarch64"],
    opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
    warning_flags: &[
        ("none",""),("default","-Wall"),
        ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
    ],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
    ],
    toolset: &[("ar","emar")],
    ..EMPTY
};

pub fn emcc() -> CompilerTemplate {
    TemplateDef {
        name: "emcc", binary: "emcc",
        extensions: &[".c",".s"],
        standards: &[("c99","-std=c99"),("c11","-std=c11"),("c17","-std=c17")],
        defaults: &[("std","c11")],
        toolset: &[("ar","emar"),("ld","emcc")],
        linking: &[LinkDef {
            lang: "c", abi: "c", compatible: &[],
            extensions: &[".c"], linker: "", compile_binary: Some("emcc"),
        }],
        ..BASE_EMCC
    }.build(&[], &[])
}

pub fn empp() -> CompilerTemplate {
    TemplateDef {
        name: "em++", binary: "em++",
        version_regex: r"em\+\+.*?(\d+\.\d+\.\d+)",
        extensions: &[".cpp",".cc",".cxx",".c++"],
        standards: &[("c++17","-std=c++17"),("c++20","-std=c++20"),("c++23","-std=c++23")],
        defaults: &[("std","c++17")],
        toolset: &[("ar","emar"),("ld","em++")],
        linking: &[LinkDef {
            lang: "cpp", abi: "c++", compatible: &["c"],
            extensions: &[".cpp",".cc",".cxx",".c++"], linker: "", compile_binary: None,
        }],
        ..BASE_EMCC
    }.build(&[], &[])
}

pub fn wasi_clang() -> CompilerTemplate {
    TemplateDef {
        name: "wasi-clang", binary: "wasi-clang",
        alias: Some("wasi-clang++"),
        family: "llvm",
        version_regex: r"\b(\d+\.\d+\.\d+)\b",
        extensions: &[".c",".cpp",".cc",".cxx"],
        always_flags: &["--target=wasm32-wasi"],
        debug: "-g",
        lto:   "-flto",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
        warning_flags: &[
            ("none",""),("default","-Wall"),
            ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
        ],
        standards: &[
            ("c11","-std=c11"),("c17","-std=c17"),
            ("c++17","-std=c++17"),("c++20","-std=c++20"),
        ],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
        ],
        toolset: &[("ar","wasi-ar")],
        linking: &[
            LinkDef { lang: "c",   abi: "c",   compatible: &[],      extensions: &[".c"],                          linker: "", compile_binary: Some("wasi-clang") },
            LinkDef { lang: "cpp", abi: "c++", compatible: &["c"],   extensions: &[".cpp",".cc",".cxx"],           linker: "", compile_binary: None },
        ],
        ..EMPTY
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![emcc(), empp(), wasi_clang()]
}
