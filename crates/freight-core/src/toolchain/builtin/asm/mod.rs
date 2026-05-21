use crate::toolchain::template::{CompilerTemplate, LinkDef, OptionHandlerFn, TemplateDef, EMPTY};

const BASE_ASM: TemplateDef = TemplateDef {
    extensions:      &[".asm",".nasm"],
    supported_archs: &["x86","x86_64"],
    requires_toolchain: &["c"],
    lto: "",
    opt_flags: &[("0",""),("1",""),("2",""),("3",""),("s",""),("z","")],
    warning_flags: &[("none",""),("default",""),("all","-w+all")],
    structure: &[
        ("include_dir","-I{path}"),("define","-D{name}"),("define_value","-D{name}={value}"),
        ("output","-o {path}"),("compile_only",""),
    ],
    arch_flags: &[
        ("x86_64.linux","-f elf64"),("x86_64.macos","-f macho64"),("x86_64.windows","-f win64"),
        ("x86.linux","-f elf32"),("x86.macos","-f macho32"),("x86.windows","-f win32"),
    ],
    linking: &[LinkDef {
        lang: "asm", abi: "c", compatible: &["c","cpp"],
        extensions: &[".asm",".nasm"], linker: "", compile_binary: None,
    }],
    ..EMPTY
};

fn arch_check_handler(v: &str, _: &str, arch: &str, _: &str, name: &str) -> Result<Vec<String>, String> {
    if !v.is_empty() && arch != v {
        Err(format!("assembler '{name}' requires arch '{v}' but the effective target is '{arch}'"))
    } else {
        Ok(vec![])
    }
}

pub fn nasm() -> CompilerTemplate {
    TemplateDef {
        name: "nasm", binary: "nasm",
        version_regex: r"NASM version (\d+\.\d+(?:\.\d+)?)",
        debug: "-g -F dwarf",
        warning_flags: &[("none",""),("default",""),("all","-w+all"),("error","-w+all -w+error")],
        toolset: &[("as","nasm")],
        ..BASE_ASM
    }.build(&[], &[("arch", arch_check_handler as OptionHandlerFn, None)])
}

pub fn yasm() -> CompilerTemplate {
    TemplateDef {
        name: "yasm", binary: "yasm",
        version_regex: r"yasm (\d+\.\d+\.\d+)",
        debug: "-g dwarf2",
        warning_flags: &[("none",""),("default",""),("all","-w+all"),("error","-w+all -Werror")],
        toolset: &[("as","yasm")],
        ..BASE_ASM
    }.build(&[], &[("arch", arch_check_handler as OptionHandlerFn, None)])
}

pub fn templates() -> Vec<CompilerTemplate> {
    vec![nasm(), yasm()]
}
