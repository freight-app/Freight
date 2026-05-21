use crate::toolchain::template::{CompilerTemplate, LinkDef, TemplateDef, EMPTY};

pub fn hipcc() -> CompilerTemplate {
    TemplateDef {
        name: "hipcc", binary: "hipcc",
        version_regex: r"HIP version: (\d+\.\d+\.\d+)",
        extensions: &[".hip"],
        supported_archs: &["x86_64"],
        supported_os:    &["linux"],
        required_tools:  &["hipconfig"],
        requires_toolchain: &["cpp"],
        sanitizer_options: &["address","undefined"],
        debug:    "-g -ggdb",
        lto:      "-flto",
        sanitize: "-fsanitize={values}",
        opt_flags: &[("0","-O0"),("1","-O1"),("2","-O2"),("3","-O3"),("s","-Os"),("z","-Oz")],
        warning_flags: &[
            ("none",""),("default","-Wall"),
            ("all","-Wall -Wextra"),("error","-Wall -Wextra -Werror"),
        ],
        standards: &[("c++14","-std=c++14"),("c++17","-std=c++17"),("c++20","-std=c++20")],
        structure: &[
            ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
            ("output","-o {path}"),("compile_only","-c"),("dep_file","-MMD -MF {path}"),
            ("target","--target={triple}"),("sysroot","--sysroot={path}"),
        ],
        toolset: &[("ld","hipcc")],
        linking: &[LinkDef {
            lang: "hip", abi: "hip", compatible: &["c++","c","fortran"],
            extensions: &[".hip"], linker: "c++", compile_binary: None,
        }],
        ..EMPTY
    }.build(&[], &[])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![hipcc()]
}
