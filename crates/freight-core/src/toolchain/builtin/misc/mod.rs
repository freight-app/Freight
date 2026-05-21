use crate::toolchain::template::{CompilerTemplate, LinkDef, OptionHandlerFn, TemplateDef, EMPTY};

fn dip1000_h(v: &str, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<String>, String> {
    if v == "true" { Ok(vec!["-preview=dip1000".into()]) } else { Ok(vec![]) }
}

// ── TCC ───────────────────────────────────────────────────────────────────────

pub fn tcc() -> CompilerTemplate {
    TemplateDef {
        name: "tcc", binary: "tcc",
        version_arg:   "-v",
        version_regex: r"version (\d+\.\d+\.\d+)",
        extensions: &[".c"],
        debug: "-g",
        opt_flags: &[("0",""),("1",""),("2",""),("3",""),("s",""),("z","")],
        warning_flags: &[("none",""),("default","-Wall"),("all","-Wall"),("error","-Wall -Werror")],
        standards: &[("c99","-std=c99"),("c11","-std=c11"),("c17","-std=c17")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),
        ],
        toolset: &[("cc","tcc"),("ld","tcc"),("ar","tcc")],
        linking: &[LinkDef {
            lang: "c", abi: "c", compatible: &[],
            extensions: &[".c"], linker: "", compile_binary: Some("tcc"),
        }],
        ..EMPTY
    }.build(&[], &[])
}

// ── DMD ───────────────────────────────────────────────────────────────────────

pub fn dmd() -> CompilerTemplate {
    TemplateDef {
        name: "dmd", binary: "dmd",
        version_regex: r"v(\d+\.\d+\.\d+)",
        extensions: &[".d"],
        debug: "-g",
        opt_flags: &[("0",""),("1","-O"),("2","-O"),("3","-O -release"),("s","-O -release"),("z","-O -release")],
        warning_flags: &[("none",""),("default",""),("all","-wi"),("error","-w")],
        structure: &[
            ("include_dir","-I{path}"),("define","-version={name}"),("define_value","-version={name}"),
            ("output","-of{path}"),("compile_only","-c"),("dep_file_mode","none"),("system_lib","-L-l{name}"),
        ],
        toolset: &[("ld","dmd"),("ar","ar"),("strip","strip")],
        linking: &[LinkDef {
            lang: "d", abi: "d", compatible: &["c"],
            extensions: &[".d"], linker: "", compile_binary: None,
        }],
        ..EMPTY
    }.build(&[("dip1000", dip1000_h as OptionHandlerFn, Some("false"))], &[])
}

// ── OpenCL ────────────────────────────────────────────────────────────────────

pub fn opencl() -> CompilerTemplate {
    TemplateDef {
        name: "opencl", binary: "clang",
        version_regex: r"\b(\d+\.\d+\.\d+)\b",
        extensions: &[".cl"],
        always_flags: &["-x","cl"],
        requires_toolchain: &["cpp"],
        debug: "-g",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
        warning_flags: &[
            ("none",""),("default","-Wall"),
            ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
        ],
        standards: &[
            ("CL1.0","-cl-std=CL1.0"),("CL1.1","-cl-std=CL1.1"),("CL1.2","-cl-std=CL1.2"),
            ("CL2.0","-cl-std=CL2.0"),("CL3.0","-cl-std=CL3.0"),
        ],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
        ],
        toolset: &[("ld","clang")],
        linking: &[LinkDef {
            lang: "opencl", abi: "opencl", compatible: &["c++","c"],
            extensions: &[".cl"], linker: "c++", compile_binary: None,
        }],
        ..EMPTY
    }.build(&[], &[])
}

// ── Circle ────────────────────────────────────────────────────────────────────

pub fn circle() -> CompilerTemplate {
    TemplateDef {
        name: "circle", binary: "circle",
        family: "llvm",
        version_regex: r"version (\d+)",
        extensions: &[".cpp",".cc",".cxx",".c++"],
        debug:    "-g",
        lto:      "-flto",
        sanitize: "-fsanitize={values}",
        sanitizer_options: &["address","undefined"],
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
        warning_flags: &[
            ("none",""),("default","-Wall"),
            ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
        ],
        standards: &[("c++20","-std=c++20"),("c++23","-std=c++23")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
            ("target","--target={triple}"),
        ],
        toolset: &[("ld","circle"),("ar","ar")],
        linking: &[LinkDef {
            lang: "cpp", abi: "c++", compatible: &["c"],
            extensions: &[".cpp",".cc",".cxx",".c++"], linker: "", compile_binary: None,
        }],
        ..EMPTY
    }.build(&[], &[])
}

// ── NAG Fortran ───────────────────────────────────────────────────────────────

pub fn nagfor() -> CompilerTemplate {
    const F_EXTS: &[&str] = &[".f90",".f95",".f03",".f08",".f",".F90"];
    TemplateDef {
        name: "nagfor", binary: "nagfor",
        version_arg:   "-V",
        // "NAG Fortran Compiler Release 7.2(Morzine) Build 7202"
        version_regex: r"Release (\d+\.\d+)",
        extensions: F_EXTS,
        debug: "-g",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O4"),("s","-O2"),("z","-O2")],
        warning_flags: &[
            ("none","-w=all -quiet"),("default",""),
            ("all","-w=obs -w=unused -w=undef"),
            ("error","-w=obs -w=unused -w=undef -halt=error"),
        ],
        standards: &[("f95","-f95"),("f2003","-f2003"),("f2008","-f2008"),("f2018","-f2018")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file_mode","none"),
        ],
        toolset: &[("ld","nagfor"),("ar","ar")],
        linking: &[LinkDef {
            lang: "fortran", abi: "fortran", compatible: &["c"],
            extensions: F_EXTS, linker: "", compile_binary: None,
        }],
        ..EMPTY
    }.build(&[], &[])
}

// ── GNAT (Ada) ────────────────────────────────────────────────────────────────

pub fn gnat() -> CompilerTemplate {
    TemplateDef {
        name: "gnat", binary: "gnat",
        family: "gnu",
        // "GNAT Community Edition 2021 (20210519-103)" or "GNAT 13.2.0"
        version_regex: r"(?:GNAT.*?(\d{4})|GNAT \w+ (\d+\.\d+))",
        extensions: &[".adb",".ads"],
        debug: "-g",
        lto:   "-flto",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Os")],
        warning_flags: &[
            ("none","-gnatws"),("default",""),
            ("all","-gnatwa"),("error","-gnatwa -gnatwe"),
        ],
        standards: &[
            ("ada83","-gnat83"),("ada95","-gnat95"),("ada2005","-gnat2005"),
            ("ada2012","-gnat2012"),("ada2022","-gnat2022"),
        ],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file_mode","none"),
        ],
        toolset: &[("ld","gnat"),("ar","ar")],
        linking: &[LinkDef {
            lang: "ada", abi: "ada", compatible: &["c"],
            extensions: &[".adb",".ads"], linker: "", compile_binary: Some("gnat"),
        }],
        ..EMPTY
    }.build(&[], &[])
}

// ── Swift ─────────────────────────────────────────────────────────────────────

pub fn swiftc() -> CompilerTemplate {
    TemplateDef {
        name: "swiftc", binary: "swiftc",
        // "Swift version 5.10.1 (swift-5.10.1-RELEASE)"
        version_regex: r"Swift version (\d+\.\d+(?:\.\d+)?)",
        extensions: &[".swift"],
        debug: "-g",
        lto:   "-lto=llvm-full",
        opt_flags: &[
            ("0","-Onone"),("1","-O"),("2","-O"),
            ("3","-O -whole-module-optimization"),("s","-Osize"),("z","-Osize"),
        ],
        warning_flags: &[
            ("none","-suppress-warnings"),("default",""),
            ("all","-warnings-as-notes"),("error","-warnings-as-errors"),
        ],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file_mode","none"),
        ],
        toolset: &[("ld","swiftc")],
        linking: &[LinkDef {
            lang: "swift", abi: "swift", compatible: &["c"],
            extensions: &[".swift"], linker: "", compile_binary: None,
        }],
        ..EMPTY
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![tcc(), dmd(), opencl(), circle(), nagfor(), gnat(), swiftc()]
}
