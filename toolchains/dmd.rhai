// DMD — reference D compiler.
// Produces native objects for D source files and drives final linking through dmd.

name          = "dmd";
kind          = "compiler";
family        = "";
sanitizer_options = [];
homepage      = "https://dlang.org/dmd.html";
binary        = "dmd";
version_arg   = "--version";
version_regex = "v(\\d+\\.\\d+\\.\\d+)";
extensions    = [".d"];
passthrough        = false;
passthrough_prefix = "";

toolset["ld"]    = "dmd";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

opt["0"] = "";
opt["1"] = "-O";
opt["2"] = "-O";
opt["3"] = "-O -release";
opt["s"] = "-O -release";
opt["z"] = "-O -release";

dbg = "-g";

warnings["none"]    = "";
warnings["default"] = "";
warnings["all"]     = "-wi";
warnings["error"]   = "-w";

lto = "";

include_dir  = "-I{path}";
define       = "-version={name}";
define_value = "-version={name}";
output       = "-of{path}";
compile_only = "-c";
dep_file     = "";
dep_file_mode = "none";
system_lib   = "-L-l{name}";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["d"] = #{
    abi:        "d",
    compatible: ["c"],
    linker:     "",
    extensions: [".d"],
};

compiler_option("dip1000", "false", |ctx| {
    if ctx.value == "true" { add_flag("-preview=dip1000"); }
});

fn check() { find_tool("dmd") != () }
