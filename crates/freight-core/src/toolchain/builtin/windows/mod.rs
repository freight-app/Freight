use crate::toolchain::template::{CompilerTemplate, LinkDef, TemplateDef, EMPTY};

const MSVC_STRUCTURE: &[(&str, &str)] = &[
    ("include_dir","/I{path}"),("define","/D{name}"),("define_value","/D{name}={value}"),
    ("output_obj","/Fo{path}"),("output_bin","/Fe{path}"),("compile_only","/c"),
    ("dep_file","/showIncludes"),("dep_file_mode","stdout"),("system_lib","{name}.lib"),
];

const CPP_EXTS: &[&str] = &[".cpp",".cc",".cxx",".c++"];

pub fn msvc() -> CompilerTemplate {
    TemplateDef {
        name: "msvc", binary: "cl.exe",
        version_arg:   "",
        version_regex: r"Version (\d+\.\d+\.\d+\.\d+)",
        extensions: &[".cpp",".cc",".cxx",".c++",".c"],
        sanitizer_options: &["address"],
        supported_os: &["windows"],
        debug:    "/Zi /FS",
        lto:      "/GL",
        lto_link: "/LTCG",
        sanitize: "/fsanitize={values}",
        opt_flags: &[("0","/Od"),("1","/O1"),("2","/O2"),("3","/Ox"),("s","/O1 /Os"),("z","/O1 /Os")],
        warning_flags: &[("none","/W0"),("default","/W3"),("all","/W4"),("error","/W4 /WX")],
        standards: &[
            ("c++17","/std:c++17"),("c++20","/std:c++20"),("c++23","/std:c++latest"),
            ("c17","/std:c17"),("c11","/std:c11"),
        ],
        structure: MSVC_STRUCTURE,
        toolset: &[("cc","cl.exe"),("cxx","cl.exe"),("ld","link.exe"),("ar","lib.exe"),("strip","")],
        linking: &[
            LinkDef { lang: "c",   abi: "c",   compatible: &[],      extensions: &[".c"],   linker: "", compile_binary: Some("cl.exe") , whole_program: false },
            LinkDef { lang: "cpp", abi: "c++", compatible: &["c"],   extensions: CPP_EXTS,  linker: "", compile_binary: None , whole_program: false },
        ],
        ..EMPTY
    }.build(&[], &[])
}

pub fn clang_cl() -> CompilerTemplate {
    TemplateDef {
        name: "clang-cl", binary: "clang-cl",
        family: "llvm",
        version_regex: r"\b(\d+\.\d+\.\d+)\b",
        extensions: &[".cpp",".cc",".cxx",".c++",".c"],
        sanitizer_options: &["address","undefined"],
        supported_os: &["windows"],
        debug:    "/Zi /FS",
        lto:      "/GL",
        lto_link: "/LTCG",
        sanitize: "/fsanitize={values}",
        opt_flags: &[("0","/Od"),("1","/O1"),("2","/O2"),("3","/Ox"),("s","/O1 /Os"),("z","/O1 /Os")],
        warning_flags: &[
            ("none","/W0"),("default","/W3"),
            ("all","/W4 -Wextra"),("error","/W4 -Wextra /WX"),
        ],
        standards: &[
            ("c++17","/std:c++17"),("c++20","/std:c++20"),("c++23","/std:c++latest"),
            ("c17","/std:c17"),("c11","/std:c11"),
        ],
        structure: MSVC_STRUCTURE,
        toolset: &[("ld","lld-link"),("ar","llvm-lib")],
        linking: &[
            LinkDef { lang: "c",   abi: "c",   compatible: &[],    extensions: &[".c"],  linker: "", compile_binary: Some("clang-cl") , whole_program: false },
            LinkDef { lang: "cpp", abi: "c++", compatible: &["c"], extensions: CPP_EXTS, linker: "", compile_binary: None , whole_program: false },
        ],
        ..EMPTY
    }.build(&[], &[])
}

pub fn masm() -> CompilerTemplate {
    TemplateDef {
        name: "masm", binary: "ml64.exe",
        alias: Some("ml.exe"),
        version_arg:   "",
        version_regex: r"(\d+\.\d+\.\d+\.\d+)",
        extensions: &[".asm",".masm"],
        supported_os: &["windows"],
        requires_toolchain: &["cpp"],
        debug: "/Zi",
        opt_flags: &[("0",""),("1",""),("2",""),("3",""),("s",""),("z","")],
        warning_flags: &[("none",""),("default",""),("all",""),("error","")],
        structure: &[
            ("include_dir","/I{path}"),("define","/D{name}"),("define_value","/D{name}={value}"),
            ("output","/Fo {path}"),("compile_only","/c"),("dep_file_mode","none"),
        ],
        linking: &[LinkDef {
            lang: "asm", abi: "c", compatible: &["c","cpp"],
            extensions: &[".asm",".masm"], linker: "c++", compile_binary: None,
            whole_program:  false,
        }],
        ..EMPTY
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![msvc(), clang_cl(), masm()]
}
