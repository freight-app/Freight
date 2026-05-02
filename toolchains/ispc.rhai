// Intel SPMD Program Compiler — vectorising compiler for CPU SIMD.
// Outputs a .o + a C header. The host links the .o from C or C++.

name          = "ispc";
family        = "";
homepage      = "https://ispc.github.io/";
binary        = "ispc";
version_arg   = "--version";
version_regex = "(\\d+\\.\\d+\\.\\d+)";
extensions    = [".ispc"];
passthrough        = false;
passthrough_prefix = "";
// ISPC compiler binaries are published for x86_64 and aarch64.
supported_archs = ["x86_64", "aarch64"];
// ISPC outputs .o + a C header; the host C++ application links everything.
requires_toolchain = ["cpp"];

toolset["ld"] = "ispc";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-O2";
flags["opt"]["z"] = "-O2";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "--woff";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "";
flags["warnings"]["error"]   = "--werror";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "";
dep_file     = "-MMM {path}";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["ispc"] = #{
    abi:        "ispc",
    compatible: ["c++", "c"],
    linker:     "",
    extensions: [".ispc"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "aarch64" { return false; }
    find_tool("ispc") != ()
}
