// Shared base for x86/x86_64 assemblers (NASM, YASM).
// Both assemblers produce object files linked by the host C toolchain.

kind               = "compiler";
family             = "";
sanitizer_options  = [];
version_arg        = "--version";
passthrough        = false;
passthrough_prefix = "";
supported_archs    = ["x86", "x86_64"];
requires_toolchain = ["c"];
extensions         = [".asm", ".nasm"];

// Assemblers have no optimisation, LTO, sanitize or CPU extension flags.
opt["0"] = "";
opt["1"] = "";
opt["2"] = "";
opt["3"] = "";
opt["s"] = "";
opt["z"] = "";

dbg = "";

warnings["none"]    = "";
warnings["default"] = "";
warnings["all"]     = "-w+all";
// warnings["error"] is assembler-specific: set in nasm.rhai / yasm.rhai.

lto = "";

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
