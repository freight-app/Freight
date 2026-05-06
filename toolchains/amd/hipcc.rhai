// AMD ROCm HIP compiler — AMD's CUDA equivalent.

name          = "hipcc";
family        = "amd";
sanitizers = ["address", "undefined"];
homepage      = "https://rocm.docs.amd.com/";
binary        = "hipcc";
version_arg   = "--version";
version_regex = "HIP version: (\\d+\\.\\d+\\.\\d+)";
extensions    = [".hip", ".cpp", ".cc", ".cxx"];
passthrough        = false;
passthrough_prefix = "";
// AMD ROCm supports x86_64 Linux only.
supported_archs = ["x86_64"];
supported_os    = ["linux"];
// hipconfig is bundled with every ROCm installation and confirms a complete install.
required_tools = ["hipconfig"];
// hipcc wraps clang and delegates final linking to the host C++ linker.
requires_toolchain = ["cpp"];

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

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "--target={triple}";
sysroot      = "--sysroot={path}";

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
