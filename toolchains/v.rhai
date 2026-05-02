// V — fast, simple, safe language that compiles via a C back-end.
//
// V compiles source to an executable in one pass; object-file mode is not
// supported. `compile_only = ""` means freight treats V like a whole-program
// compiler: it builds the complete binary rather than individual object files.
//
// Cross-compilation is achieved via V's own `-os` and `-arch` flags rather
// than a target triple.

name          = "v";
family        = "";
homepage      = "https://vlang.io/";
binary        = "v";
version_arg   = "version";
version_regex = "V (\\d+\\.\\d+\\.\\d+)";
extensions    = [".v", ".vv"];
passthrough        = false;
passthrough_prefix = "";

toolset["ld"] = "v";

flags["opt"]["0"] = "";
flags["opt"]["1"] = "";
flags["opt"]["2"] = "-prod";
flags["opt"]["3"] = "-prod";
flags["opt"]["s"] = "-prod";
flags["opt"]["z"] = "-prod";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "";
flags["warnings"]["error"]   = "";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

include_dir  = "";
define       = "-d {name}";
define_value = "-d {name}";
output       = "-o {path}";
compile_only = "";
dep_file     = "";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["v"] = #{
    abi:        "c",
    compatible: [],
    linker:     "",
    extensions: [".v", ".vv"],
};

fn check() {
    find_tool("v") != ()
}
