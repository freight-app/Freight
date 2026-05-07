// AMD ROCm HIP compiler — AMD's CUDA equivalent.

name          = "hipcc";
kind          = "compiler";
family        = "";
sanitizer_options = ["address", "undefined"];
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

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-Os";
opt["z"] = "-Oz";

dbg["true"]  = "-g -ggdb";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra";
warnings["error"]   = "-Wall -Wextra -Werror";

lto["true"]  = "";
lto["false"] = "";


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
