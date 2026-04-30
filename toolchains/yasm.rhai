// YASM — NASM-compatible x86/x86_64 assembler.
// Drop-in replacement for NASM; accepts the same -f output format flags.

compiler["name"]          = "yasm";
compiler["homepage"]      = "https://yasm.tortall.net/";
compiler["binary"]        = "yasm";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "yasm (\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".asm", ".nasm"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
// YASM only assembles x86 syntax; useless on non-x86 hosts.
compiler["supported_archs"] = ["x86", "x86_64"];

toolset["as"] = "yasm";

// YASM has no optimisation, LTO, sanitize or CPU extension flags.
flags["opt"]["0"] = "";
flags["opt"]["1"] = "";
flags["opt"]["2"] = "";
flags["opt"]["3"] = "";
flags["opt"]["s"] = "";
flags["opt"]["z"] = "";

flags["debug"]["true"]  = "-g dwarf2";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "-w+all";
flags["warnings"]["error"]   = "-w+all -Werror";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "";
structure["dep_file"]     = "";
structure["target"]       = "";
structure["sysroot"]      = "";

// Output format is arch+OS dependent — same table as NASM.
arch_flags["x86_64.linux"]   = "-f elf64";
arch_flags["x86_64.macos"]   = "-f macho64";
arch_flags["x86_64.windows"] = "-f win64";
arch_flags["x86.linux"]      = "-f elf32";
arch_flags["x86.macos"]      = "-f macho32";
arch_flags["x86.windows"]    = "-f win32";

modules["style"] = "none";

linking["asm"] = #{
    abi:        "c",
    compatible: ["c", "cpp"],
    linker:     "",
    extensions: [".asm", ".nasm"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "x86" { return false; }
    find_tool("yasm") != ()
}
