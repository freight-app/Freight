// Mojo — high-performance AI/systems language by Modular.
// A superset of Python that compiles to native code via MLIR/LLVM.
// Requires the Mojo SDK (`modular install mojo`).
//
// Mojo can produce object files: `mojo build --emit-object file.mojo -o file.o`
// The `-emit-object` flag enables the compile-to-object workflow.

name          = "mojo";
family        = "";
supported_sanitizers = [];
homepage      = "https://www.modular.com/mojo";
binary        = "mojo";
version_arg   = "--version";
version_regex = "mojo (\\d+\\.\\d+\\.\\d+)";
extensions    = [".mojo", ".🔥"];
passthrough        = false;
passthrough_prefix = "";
always_flags       = ["build"];

toolset["ld"]    = "mojo";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-O2";
flags["opt"]["z"] = "-O2";

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

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "--emit-object";
dep_file     = "";
target       = "--target={triple}";
sysroot      = "";

modules["style"] = "none";

linking["mojo"] = #{
    abi:        "c",
    compatible: ["c", "cpp"],
    linker:     "",
    extensions: [".mojo"],
};

fn check() {
    find_tool("mojo") != ()
}
