// NASM assembler — x86/x86_64 assembly.
// Output format is arch+OS dependent; arch_flags selects -f elf64 etc.

name          = "nasm";
family        = "";
requires_toolchain = ["c"];
sanitizer_options = [];
homepage      = "https://www.nasm.us/";
binary        = "nasm";
version_arg   = "--version";
version_regex = "NASM version (\\d+\\.\\d+(?:\\.\\d+)?)";
extensions    = [".asm", ".nasm"];
passthrough        = false;
passthrough_prefix = "";
// NASM only understands x86 assembly syntax; useless on non-x86 hosts.
supported_archs = ["x86", "x86_64"];

toolset["as"] = "nasm";

// NASM has no optimisation, LTO, sanitize or CPU extension flags.
opt["0"] = "";
opt["1"] = "";
opt["2"] = "";
opt["3"] = "";
opt["s"] = "";
opt["z"] = "";

dbg["true"]  = "-g -F dwarf";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "";
warnings["all"]     = "-w+all";
warnings["error"]   = "-w+all -Werror";

lto["true"]  = "";
lto["false"] = "";


include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "";
dep_file     = "";
target       = "";
sysroot      = "";

// Output format is arch+OS dependent.
// freight picks the most-specific key ("arch.os" first, then "arch").
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
    find_tool("nasm") != ()
}
