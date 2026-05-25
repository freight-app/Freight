use crate::toolchain::template::{CompilerTemplate, LinkDef, TemplateDef, EMPTY};

const BASE_GNU: TemplateDef = TemplateDef {
    family:        "gnu",
    version_regex: r"\b(\d+\.\d+\.\d+)\b",
    debug:  "-g",
    lto:    "-flto",
    sanitize: "-fsanitize={values}",
    sanitizer_options: &["address", "undefined", "thread", "leak"],
    cpu_ext: "-m{name}",
    opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
    warning_flags: &[
        ("none",""),("default","-Wall"),
        ("all","-Wall -Wextra -Wpedantic"),("error","-Wall -Wextra -Wpedantic -Werror"),
    ],
    stdlib_flags: &[("libstdc++",""),("none","-nostdlib")],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
        ("sysroot","--sysroot={path}"),
    ],
    toolset: &[("ar","ar"),("strip","strip")],
    ..EMPTY
};

const CPP_EXTS: &[&str] = &[".cpp",".cppm",".ixx",".mpp",".cc",".cxx",".c++"];
const C_EXTS:   &[&str] = &[".c",".s",".S"];
const F_EXTS:   &[&str] = &[".f90",".f95",".f03",".f08",".f",".F90"];

pub fn gpp() -> CompilerTemplate {
    TemplateDef {
        name: "g++", binary: "g++",
        alias: Some("gcc"),
        extensions: CPP_EXTS,
        standards: &[
            ("c++11","-std=c++11"),("c++14","-std=c++14"),
            ("c++17","-std=c++17"),("c++20","-std=c++20"),
            ("c++23","-std=c++23"),("c++26","-std=c++26"),
        ],
        standard_min_versions: &[
            ("c++11","4.8"),("c++14","5.0"),("c++17","7.0"),
            ("c++20","10.0"),("c++23","12.0"),("c++26","14.0"),
        ],
        defaults: &[("std","c++17")],
        toolset: &[("ar","ar"),("strip","strip"),("cc","gcc"),("cxx","g++"),("ld","g++")],
        module_style: "gcc",
        module_params: &[
            ("enable_flag","-fmodules-ts"),
            ("compile_miu","-fmodule-output={pcm_path}"),
            ("import_module","-fmodule-file={name}={pcm_path}"),
            ("header_unit","-fmodule-header"),
        ],
        pch: &[
            ("compile","-x c++-header"),("use","-include {header_path}"),
            ("extension",".gch"),("clangd_flag","-include {header_path}"),
        ],
        linking: &[LinkDef {
            lang: "cpp", abi: "c++", compatible: &["c","fortran"],
            extensions: CPP_EXTS, linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_GNU
    }.build(&[], &[])
}

pub fn gcc() -> CompilerTemplate {
    TemplateDef {
        name: "gcc", binary: "gcc",
        extensions: C_EXTS,
        standards: &[
            ("c89","-std=c89"),("c99","-std=c99"),
            ("c11","-std=c11"),("c17","-std=c17"),("c23","-std=c23"),
        ],
        standard_min_versions: &[
            ("c99","3.4"),("c11","4.9"),("c17","8.0"),("c23","14.0"),
        ],
        defaults: &[("std","c11")],
        toolset: &[("ar","ar"),("strip","strip"),("cc","gcc"),("cxx","g++"),("ld","g++")],
        linking: &[LinkDef {
            lang: "c", abi: "c", compatible: &["fortran","asm"],
            extensions: C_EXTS, linker: "", compile_binary: Some("gcc"),
            whole_program:  false,
        }],
        ..BASE_GNU
    }.build(&[], &[])
}

pub fn gfortran() -> CompilerTemplate {
    TemplateDef {
        name: "gfortran", binary: "gfortran",
        extensions: F_EXTS,
        standards: &[
            ("f95","-std=f95"),("f2003","-std=f2003"),
            ("f2008","-std=f2008"),("f2018","-std=f2018"),
        ],
        standard_min_versions: &[
            ("f2003","4.4"),("f2008","4.6"),("f2018","8.0"),
        ],
        defaults: &[("std","f2018")],
        sanitizer_options: &["address","undefined"],
        stdlib_flags: &[],
        cpu_ext: "",
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-cpp -MMD -MF {path}"),
            ("sysroot","--sysroot={path}"),
        ],
        toolset: &[("ar","ar"),("strip","strip"),("ld","gfortran")],
        linking: &[LinkDef {
            lang: "fortran", abi: "fortran", compatible: &["c"],
            extensions: F_EXTS, linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_GNU
    }.build(&[], &[])
}

pub fn gdc() -> CompilerTemplate {
    TemplateDef {
        name: "gdc", binary: "gdc",
        extensions: &[".d"],
        warning_flags: &[
            ("none",""),("default",""),
            ("all","-Wall"),("error","-Wall -Werror"),
        ],
        structure: &[
            ("include_dir","-I{path}"),("define","-fversion={name}"),
            ("define_value","-fversion={name}"),("output","-o {path}"),
            ("compile_only","-c"),("dep_file","-MMD -MF {path}"),
        ],
        toolset: &[("ar","ar"),("strip","strip"),("ld","gdc")],
        linking: &[LinkDef {
            lang: "d", abi: "d", compatible: &["c"],
            extensions: &[".d"], linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_GNU
    }.build(&[], &[])
}

pub fn gas() -> CompilerTemplate {
    TemplateDef {
        name: "gas", binary: "as",
        version_regex: r"GNU assembler (?:\([^)]+\) )?(\d+\.\d+(?:\.\d+)?)",
        extensions: &[".s",".S"],
        requires_toolchain: &["c"],
        debug: "--gdwarf-2",
        lto: "",
        opt_flags: &[("0",""),("1",""),("2",""),("3",""),("s",""),("z","")],
        warning_flags: &[
            ("none","--no-warn"),("default",""),
            ("all","--warn"),("error","--warn --fatal-warnings"),
        ],
        stdlib_flags: &[],
        sanitize: "",
        sanitizer_options: &[],
        cpu_ext: "",
        structure: &[
            ("include_dir","-I{path}"),("define","--defsym {name}=1"),
            ("define_value","--defsym {name}={value}"),("output","-o {path}"),
            ("compile_only",""),
        ],
        arch_flags: &[
            ("x86_64.linux","--64"),("x86_64.macos","--64"),
            ("x86_64.windows","--64"),("x86","--32"),
        ],
        toolset: &[],
        linking: &[LinkDef {
            lang: "gas", abi: "c", compatible: &["c","cpp"],
            extensions: &[".s",".S"], linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..EMPTY
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![gpp(), gcc(), gfortran(), gdc(), gas()]
}
