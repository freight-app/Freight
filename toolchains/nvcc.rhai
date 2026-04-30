compiler["name"]          = "nvcc";
compiler["homepage"]      = "https://developer.nvidia.com/cuda-toolkit";
compiler["binary"]        = "nvcc";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "release (\\d+\\.\\d+)";
compiler["extensions"]    = [".cu", ".cuh"];
compiler["passthrough"]        = true;
compiler["passthrough_prefix"] = "-Xcompiler";
// CUDA Toolkit is available on x86_64 and aarch64 (Jetson / Grace) Linux and Windows.
// macOS support was dropped in CUDA 11.6.
compiler["supported_archs"] = ["x86_64", "aarch64"];
compiler["supported_os"]    = ["linux", "windows"];
// ptxas and fatbinary are bundled with the CUDA Toolkit alongside nvcc.
// Their absence means a partial or broken toolkit installation.
compiler["required_tools"] = ["ptxas", "fatbinary"];
// nvcc produces device code but delegates final linking to the host C++ linker.
compiler["requires_toolchain"] = ["cpp"];
compiler["always_flags"] = ["--expt-relaxed-constexpr", "--extended-lambda"];

toolset["ld"] = "nvcc";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-O2";
flags["opt"]["z"] = "-O2";

flags["debug"]["true"]  = "-g -G";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra";
flags["warnings"]["error"]   = "-Wall -Wextra -Werror";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MD -MF {path}";
structure["target"]       = "";
structure["sysroot"]      = "";

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
