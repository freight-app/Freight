// AMD ROCm HIP compiler — AMD's CUDA equivalent.

compiler["name"]          = "hipcc";
compiler["homepage"]      = "https://rocm.docs.amd.com/";
compiler["binary"]        = "hipcc";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "HIP version: (\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".hip", ".cpp", ".cc", ".cxx"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
// AMD ROCm supports x86_64 Linux only.
compiler["supported_archs"] = ["x86_64"];
compiler["supported_os"]    = ["linux"];
// hipconfig is bundled with every ROCm installation and confirms a complete install.
compiler["required_tools"] = ["hipconfig"];
// hipcc wraps clang and delegates final linking to the host C++ linker.
compiler["requires_toolchain"] = ["cpp"];

toolset["ld"] = "hipcc";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Oz";

flags["debug"]["true"]  = "-g -ggdb";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra";
flags["warnings"]["error"]   = "-Wall -Wextra -Werror";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

standards["c++14"] = "-std=c++14";
standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MMD -MF {path}";
structure["target"]       = "--target={triple}";
structure["sysroot"]      = "--sysroot={path}";

modules["style"] = "none";

linking["hip"] = #{
    abi:        "hip",
    compatible: ["c++", "c", "fortran"],
    linker:     "c++",
    extensions: [".hip"],
};

fn check() {
    if arch != "x86_64" || os != "linux" { return false; }
    find_tool("hipcc") != () && find_tool("hipconfig") != ()
}
