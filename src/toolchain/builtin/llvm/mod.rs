use crate::toolchain::template::{CompilerTemplate, LinkDef, OptionHandlerFn, TemplateDef, EMPTY};

const BASE_LLVM: TemplateDef = TemplateDef {
    family:        "llvm",
    version_regex: r"\b(\d+\.\d+\.\d+)\b",
    debug:  "-g",
    lto:    "-flto",
    sanitize: "-fsanitize={values}",
    cpu_ext: "-m{name}",
    opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
    warning_flags: &[
        ("none",""),("default","-Wall"),
        ("all","-Wall -Wextra -Wpedantic"),("error","-Wall -Wextra -Wpedantic -Werror"),
    ],
    stdlib_flags: &[("libc++","-stdlib=libc++"),("libstdc++","-stdlib=libstdc++"),("none","-nostdlib")],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
        ("target","--target={triple}"),("sysroot","--sysroot={path}"),
    ],
    toolset: &[("ar","ar"),("strip","strip")],
    ..EMPTY
};

const CPP_EXTS: &[&str] = &[".cpp",".cppm",".ixx",".mpp",".cc",".cxx",".c++"];
const C_EXTS:   &[&str] = &[".c",".s",".S"];
const F_EXTS:   &[&str] = &[".f90",".f95",".f03",".f08",".f",".F90"];

fn lto_mode_h(v: &str, ver: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
    if v == "thin" {
        let major: u32 = ver.split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor: u32 = ver.split('.').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        if major > 3 || (major == 3 && minor >= 9) {
            return Ok(vec!["-flto=thin".into()]);
        }
    } else if v == "full" {
        return Ok(vec!["-flto=full".into()]);
    }
    Ok(vec![])
}

fn dip1000_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
    if v == "true" { Ok(vec!["-preview=dip1000".into()]) } else { Ok(vec![]) }
}

pub fn clangpp() -> CompilerTemplate {
    TemplateDef {
        name: "clang++", binary: "clang++",
        alias: Some("clang"),
        extensions: CPP_EXTS,
        sanitizer_options: &["address","undefined","thread","memory","leak","hwaddress","dataflow","cfi","safestack"],
        standards: &[
            ("c++11","-std=c++11"),("c++14","-std=c++14"),
            ("c++17","-std=c++17"),("c++20","-std=c++20"),
            ("c++23","-std=c++23"),("c++26","-std=c++26"),
        ],
        standard_min_versions: &[
            ("c++11","3.3"),("c++14","3.4"),("c++17","5.0"),
            ("c++20","10.0"),("c++23","14.0"),("c++26","17.0"),
        ],
        defaults: &[("std","c++17")],
        toolset: &[("ar","ar"),("strip","strip"),("cc","clang"),("cxx","clang++"),("ld","clang++")],
        module_style: "clang",
        module_params: &[
            ("precompile","--precompile"),
            ("import_module","-fmodule-file={name}={pcm_path}"),
            ("header_unit","-x c++-header"),
        ],
        pch: &[
            ("compile","-x c++-header"),("use","-include-pch {pch_path}"),
            ("extension",".pch"),("clangd_flag","-include {header_path}"),
        ],
        linking: &[
            LinkDef { lang: "cpp",    abi: "c++", compatible: &["c","fortran"],  extensions: CPP_EXTS,    linker: "", compile_binary: None , whole_program: false },
            LinkDef { lang: "objcpp", abi: "c++", compatible: &["c","objc"],     extensions: &[".mm"],    linker: "", compile_binary: None , whole_program: false },
        ],
        ..BASE_LLVM
    }.build(&[("lto_mode", lto_mode_h as OptionHandlerFn, None)], &[])
}

pub fn clang() -> CompilerTemplate {
    TemplateDef {
        name: "clang", binary: "clang",
        extensions: C_EXTS,
        sanitizer_options: &["address","undefined","thread","memory","leak","hwaddress","dataflow","cfi","safestack"],
        standards: &[
            ("c89","-std=c89"),("c99","-std=c99"),
            ("c11","-std=c11"),("c17","-std=c17"),("c23","-std=c23"),
        ],
        standard_min_versions: &[
            ("c99","3.1"),("c11","3.1"),("c17","6.0"),("c23","17.0"),
        ],
        defaults: &[("std","c11")],
        toolset: &[("ar","ar"),("strip","strip"),("cc","clang"),("cxx","clang++"),("ld","clang++")],
        linking: &[
            LinkDef { lang: "c",    abi: "c",    compatible: &["fortran","asm"], extensions: C_EXTS,   linker: "", compile_binary: Some("clang") , whole_program: false },
            LinkDef { lang: "objc", abi: "objc", compatible: &["c"],             extensions: &[".m"],  linker: "", compile_binary: Some("clang") , whole_program: false },
        ],
        ..BASE_LLVM
    }.build(&[], &[])
}

pub fn flang() -> CompilerTemplate {
    TemplateDef {
        name: "flang", binary: "flang",
        version_regex: r"flang(?:-new)? version (\d+\.\d+\.\d+)",
        extensions: F_EXTS,
        sanitizer_options: &["address","undefined"],
        stdlib_flags: &[],
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-O2"),("z","-O2")],
        warning_flags: &[
            ("none",""),("default","-Wall"),
            ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
        ],
        standards: &[("f95","-std=f95"),("f2003","-std=f2003"),("f2008","-std=f2008"),("f2018","-std=f2018")],
        defaults: &[("std","f2018")],
        toolset: &[("ar","ar"),("strip","strip"),("ld","flang")],
        linking: &[LinkDef {
            lang: "fortran", abi: "fortran", compatible: &["c"],
            extensions: F_EXTS, linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_LLVM
    }.build(&[], &[])
}

pub fn ldc2() -> CompilerTemplate {
    TemplateDef {
        name: "ldc2", binary: "ldc2",
        extensions: &[".d"],
        sanitizer_options: &["address","thread","memory","undefined"],
        lto: "-flto=full",
        warning_flags: &[("none",""),("default",""),("all","-wi"),("error","-w")],
        structure: &[
            ("include_dir","-I{path}"),("define","-d-version={name}"),("define_value","-d-version={name}"),
            ("output","-of={path}"),("compile_only","-c"),("dep_file",""),
            ("dep_file_mode","none"),("system_lib","-L-l{name}"),
            ("target","-mtriple={triple}"),
        ],
        toolset: &[("ar","ar"),("strip","strip"),("ld","ldc2")],
        linking: &[LinkDef {
            lang: "d", abi: "d", compatible: &["c"],
            extensions: &[".d"], linker: "", compile_binary: None,
            whole_program:  false,
        }],
        ..BASE_LLVM
    }.build(&[("dip1000", dip1000_h as OptionHandlerFn, Some("false"))], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![clangpp(), clang(), flang(), ldc2()]
}
