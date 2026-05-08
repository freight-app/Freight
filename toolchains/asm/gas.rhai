// GNU Assembler (GAS) — binutils `as`.
// Handles GAS-syntax .s / .S files on any architecture.
// The host C toolchain provides linking; `as` always produces .o files directly.

name               = "gas";
kind               = "compiler";
family             = "";
sanitizer_options  = [];
homepage           = "https://www.gnu.org/software/binutils/";
binary             = "as";
version_arg        = "--version";
version_regex      = "GNU assembler (?:\\([^)]+\\) )?(\\d+\\.\\d+(?:\\.\\d+)?)";
extensions         = [".s", ".S"];
passthrough        = false;
passthrough_prefix = "";
requires_toolchain = ["c"];

// Assemblers have no optimisation, LTO, sanitize or CPU extension flags.
opt["0"] = "";
opt["1"] = "";
opt["2"] = "";
opt["3"] = "";
opt["s"] = "";
opt["z"] = "";

dbg["true"]  = "--gdwarf-2";
dbg["false"] = "";

warnings["none"]    = "--no-warn";
warnings["default"] = "";
warnings["all"]     = "--warn";
warnings["error"]   = "--warn --fatal-warnings";

lto["true"]  = "";   // GAS assembles individual files; LTO is a linker concern.
lto["false"] = "";

include_dir  = "-I{path}";
// `as` uses --defsym for symbol definitions; no C preprocessor available.
define       = "--defsym {name}=1";
define_value = "--defsym {name}={value}";
output       = "-o {path}";
compile_only = "";   // `as` always produces an object file; no separate link step.
dep_file     = "";   // GAS does not support dep file generation.
target       = "";
sysroot      = "";

// x86: --32 forces 32-bit output on a 64-bit host.
// x86_64: `as` defaults to 64-bit on 64-bit systems; no flag needed.
arch_flags["x86"] = "--32";

modules["style"] = "none";

linking["gas"] = #{
    abi:        "c",
    compatible: ["c", "cpp"],
    linker:     "",
    extensions: [".s", ".S"],
};

fn check() {
    find_tool("as") != ()
}
