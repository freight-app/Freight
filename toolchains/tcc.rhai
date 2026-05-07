// TCC — Tiny C Compiler. Extremely fast single-pass C compiler.
// No optimisation levels, no LTO, no sanitizers, and no dependency file generation
// (dep_file is empty — freight falls back to mtime-only dirty checking for TCC projects).

name          = "tcc";
family        = "";
sanitizers    = [];
homepage      = "https://bellard.org/tcc/";
binary        = "tcc";
version_arg   = "-v";
version_regex = "version (\\d+\\.\\d+\\.\\d+)";
extensions    = [".c"];
passthrough        = false;
passthrough_prefix = "";

toolset["cc"]  = "tcc";
toolset["ld"]  = "tcc";
toolset["ar"]  = "tcc";

// TCC ignores -O flags; all levels map to empty to avoid noise.
opt["0"] = "";
opt["1"] = "";
opt["2"] = "";
opt["3"] = "";
opt["s"] = "";
opt["z"] = "";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall";
warnings["error"]   = "-Wall -Werror";

lto["true"]  = "";
lto["false"] = "";


standards["c99"] = "-std=c99";
standards["c11"] = "-std=c11";
standards["c17"] = "-std=c17";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["c"] = #{
    abi:            "c",
    compatible:     [],
    compile_binary: "tcc",
    linker:         "",
    extensions:     [".c"],
};

fn check() {
    find_tool("tcc") != ()
}
