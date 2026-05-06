// YASM — NASM-compatible x86/x86_64 assembler.
// Drop-in replacement for NASM; accepts the same -f output format flags.

name          = "yasm";
family        = "";
requires_toolchain = ["c"];
sanitizers = [];
homepage      = "https://yasm.tortall.net/";
binary        = "yasm";
version_arg   = "--version";
version_regex = "yasm (\\d+\\.\\d+\\.\\d+)";
extensions    = [".asm", ".nasm"];
passthrough        = false;
passthrough_prefix = "";
// YASM only assembles x86 syntax; useless on non-x86 hosts.
supported_archs = ["x86", "x86_64"];

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


include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "";
dep_file     = "";
target       = "";
sysroot      = "";

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
