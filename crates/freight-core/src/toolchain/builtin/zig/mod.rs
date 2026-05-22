use crate::toolchain::template::{CompilerTemplate, LinkDef, TemplateDef, EMPTY};

const BASE_ZIG: TemplateDef = TemplateDef {
    binary:        "zig",
    version_arg:   "version",
    version_regex: r"(\d+\.\d+\.\d+)",
    debug:    "-g",
    lto:      "-flto",
    sanitize: "-fsanitize={values}",
    sanitizer_options: &["address","undefined","thread"],
    opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
    warning_flags: &[
        ("none",""),("default","-Wall"),
        ("all","-Wall -Wextra -Wpedantic"),("error","-Wall -Wextra -Wpedantic -Werror"),
    ],
    stdlib_flags: &[("libc++","-stdlib=libc++"),("libstdc++","-stdlib=libstdc++"),("none","-nostdlib")],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
        ("target","--target={triple}"),
    ],
    toolset: &[("ar","zig ar"),("strip","strip")],
    ..EMPTY
};

pub fn zig_c() -> CompilerTemplate {
    TemplateDef {
        name: "zig-c",
        family: "zig",
        subcommand: Some("cc"),
        extensions: &[".c",".s",".S"],
        standards: &[("c11","-std=c11"),("c17","-std=c17"),("c23","-std=c23")],
        defaults: &[("std","c11")],
        toolset: &[("ar","zig ar"),("strip","strip")],
        linking: &[LinkDef {
            lang: "c", abi: "c", compatible: &["asm"],
            extensions: &[".c",".s",".S"], linker: "", compile_binary: None,
        }],
        ..BASE_ZIG
    }.build(&[], &[])
}

pub fn zig_cxx() -> CompilerTemplate {
    TemplateDef {
        name: "zig-c++",
        family: "zig",
        subcommand: Some("c++"),
        extensions: &[".cpp",".cc",".cxx",".c++"],
        standards: &[("c++17","-std=c++17"),("c++20","-std=c++20"),("c++23","-std=c++23")],
        defaults: &[("std","c++17")],
        toolset: &[("ar","zig ar"),("strip","strip")],
        linking: &[LinkDef {
            lang: "cpp", abi: "c++", compatible: &["c"],
            extensions: &[".cpp",".cc",".cxx",".c++"], linker: "", compile_binary: None,
        }],
        ..BASE_ZIG
    }.build(&[], &[])
}

pub fn zig_native() -> CompilerTemplate {
    TemplateDef {
        name: "zig",
        family: "zig",
        // compile: zig build-obj; link: zig build-exe
        subcommand:      Some("build-obj"),
        link_subcommand: Some("build-exe"),
        extensions: &[".zig"],
        // Zig doesn't use -g; debug info is included in Debug mode by default.
        // LTO is implicit in ReleaseFast/ReleaseSmall.
        debug: "",
        lto:   "",
        // Zig's optimization levels are named modes, not numeric.
        opt_flags: &[
            ("0","-O Debug"),("1","-O ReleaseSafe"),("2","-O ReleaseSafe"),
            ("3","-O ReleaseFast"),("s","-O ReleaseSmall"),("z","-O ReleaseSmall"),
        ],
        // No C-style warning flags in Zig; warnings are compiler-controlled.
        warning_flags: &[("none",""),("default",""),("all",""),("error","")],
        structure: &[
            ("output","-femit-bin={path}"),
            // compile_only is empty because the subcommand (build-obj) IS the compile-only mode.
            ("compile_only",""),
            ("dep_file_mode","none"),
        ],
        linking: &[LinkDef {
            // zig build-exe can link its own object files; no separate runtime needed.
            lang: "zig", abi: "zig", compatible: &["c"],
            extensions: &[".zig"], linker: "", compile_binary: None,
        }],
        ..BASE_ZIG
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![zig_c(), zig_cxx(), zig_native()]
}
