// D reference compiler. Flag conventions also apply to LDC/GDC.
// D has no -std= flag; language version tracks the compiler version.

name          = "dmd";
family        = "";
supported_sanitizers = [];
homepage      = "https://dlang.org/";
binary        = "dmd";
version_arg   = "--version";
version_regex = "v(\\d+\\.\\d+\\.\\d+)";
extensions    = [".d"];
passthrough        = false;
passthrough_prefix = "";

toolset["ld"] = "dmd";

flags["opt"]["0"] = "";
flags["opt"]["1"] = "-O";
flags["opt"]["2"] = "-O";
flags["opt"]["3"] = "-O -release";
flags["opt"]["s"] = "-O";
flags["opt"]["z"] = "-O";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-wi";
flags["warnings"]["all"]     = "-w";
flags["warnings"]["error"]   = "-w";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-L-s";
flags["strip"]["false"] = "";

include_dir  = "-I{path}";
define       = "-version={name}";
define_value = "-version={name}";
output       = "-of={path}";
compile_only = "-c";
dep_file     = "";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["d"] = #{
    abi:        "d",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".d"],
};

fn check() {
    find_tool("dmd") != ()
}
