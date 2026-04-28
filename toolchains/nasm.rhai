// NASM assembler — x86/x86_64 assembly.
// Output format is arch+OS dependent; arch_flags selects -f elf64 etc.

set_name("nasm");
set_homepage("https://www.nasm.us/");
set_binary("nasm");
set_version_arg("--version");
set_version_regex("NASM version (\\d+\\.\\d+\\.\\d+)");

set_toolset("as", "nasm");

set_extensions([".asm", ".nasm"]);

// NASM has no optimisation, LTO, sanitize or CPU extension flags.
set_flag("opt", "0", "");
set_flag("opt", "1", "");
set_flag("opt", "2", "");
set_flag("opt", "3", "");
set_flag("opt", "s", "");
set_flag("opt", "z", "");

set_flag("debug", "true",  "-g -F dwarf");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "");
set_flag("warnings", "all",     "-w+all");
set_flag("warnings", "error",   "-w+all -Werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "");
set_flag("strip", "false", "");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "");
set_structure("dep_file",     "");
set_structure("target",       "");
set_structure("sysroot",      "");

// Output format is arch+OS dependent.
// crane picks the most-specific key ("arch.os" first, then "arch").
set_arch_flag("x86_64.linux",   "-f elf64");
set_arch_flag("x86_64.macos",   "-f macho64");
set_arch_flag("x86_64.windows", "-f win64");
set_arch_flag("x86.linux",      "-f elf32");
set_arch_flag("x86.macos",      "-f macho32");
set_arch_flag("x86.windows",    "-f win32");

set_module_style("none");
set_passthrough(false, "");

// NASM only understands x86 assembly syntax; useless on non-x86 hosts.
set_supported_archs(["x86", "x86_64"]);

set_linking("asm", #{
    abi:        "c",
    compatible: ["c", "cpp"],
    linker:     "",
    extensions: [".asm", ".nasm"],
});

fn check() {
    let h = arch();
    if h != "x86_64" && h != "x86" { return false; }
    find_tool("nasm") != ()
}
