// Intel SPMD Program Compiler — vectorising compiler for CPU SIMD.
// Outputs a .o + a C header. The host links the .o from C or C++.

compiler["name"]          = "ispc";
compiler["homepage"]      = "https://ispc.github.io/";
compiler["binary"]        = "ispc";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "(\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".ispc"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
// ISPC compiler binaries are published for x86_64 and aarch64.
compiler["supported_archs"] = ["x86_64", "aarch64"];
// ISPC outputs .o + a C header; the host C++ application links everything.
compiler["requires_toolchain"] = ["cpp"];

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

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "";
structure["dep_file"]     = "-MMM {path}";
structure["target"]       = "";
structure["sysroot"]      = "";

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
