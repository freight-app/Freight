name          = "nvcc";
family        = "";
sanitizers = [];
homepage      = "https://developer.nvidia.com/cuda-toolkit";
binary        = "nvcc";
version_arg   = "--version";
version_regex = "release (\\d+\\.\\d+)";
extensions    = [".cu", ".cuh"];
passthrough        = true;
passthrough_prefix = "-Xcompiler";
// CUDA Toolkit is available on x86_64 and aarch64 (Jetson / Grace) Linux and Windows.
// macOS support was dropped in CUDA 11.6.
supported_archs = ["x86_64", "aarch64"];
supported_os    = ["linux", "windows"];
// ptxas and fatbinary are bundled with the CUDA Toolkit alongside nvcc.
// Their absence means a partial or broken toolkit installation.
required_tools = ["ptxas", "fatbinary"];
// nvcc produces device code but delegates final linking to the host C++ linker.
requires_toolchain = ["cpp"];
always_flags = ["--expt-relaxed-constexpr", "--extended-lambda"];

toolset["ld"] = "nvcc";

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-O2";
opt["z"] = "-O2";

dbg["true"]  = "-g -G";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra";
warnings["error"]   = "-Wall -Wextra -Werror";

lto["true"]  = "";
lto["false"] = "";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MD -MF {path}";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["cuda"] = #{
    abi:        "cuda",
    compatible: ["c++", "c", "fortran"],
    linker:     "c++",
    extensions: [".cu", ".cuh"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "aarch64" { return false; }
    let o = os;
    if o != "linux" && o != "windows" { return false; }
    find_tool("nvcc") != () && find_tool("ptxas") != () && find_tool("fatbinary") != ()
}
