// Intel SPMD Program Compiler — vectorising compiler for CPU SIMD.
// Outputs a .o + a C header. The host links the .o from C or C++.

name          = "ispc";
family        = "";
sanitizer_options = [];
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

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-O2";
opt["z"] = "-O2";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "--woff";
warnings["default"] = "";
warnings["all"]     = "";
warnings["error"]   = "--werror";

lto["true"]  = "";
lto["false"] = "";


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
