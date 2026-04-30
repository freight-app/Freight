// Intel oneAPI DPC++ compiler — SYCL heterogeneous compute.
// -fsycl is injected as an always-flag; host code is standard C++.

name          = "icpx";
homepage      = "https://www.intel.com/content/www/us/en/developer/tools/oneapi/dpc-compiler.html";
binary        = "icpx";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".sycl"];
passthrough        = false;
passthrough_prefix = "";
// Intel oneAPI is available on x86/x86_64 Linux and Windows.
supported_archs = ["x86", "x86_64"];
supported_os    = ["linux", "windows"];
// ONEAPI_ROOT is exported by setvars.sh / setvars.bat; its absence means
// the environment has not been initialised and compilation will fail.
required_env  = ["ONEAPI_ROOT"];
always_flags  = ["-fsycl"];

toolset["ld"] = "icpx";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Oz";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra";
flags["warnings"]["error"]   = "-Wall -Wextra -Werror";

flags["lto"]["true"]  = "-flto";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

flags["sanitize"]["template"] = "-fsanitize={values}";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "--target={triple}";
sysroot      = "--sysroot={path}";

modules["style"] = "none";

linking["sycl"] = #{
    abi:        "sycl",
    compatible: ["c++", "c", "fortran"],
    linker:     "",
    extensions: [".sycl", ".cpp", ".cc", ".cxx"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "x86" { return false; }
    let o = os;
    if o != "linux" && o != "windows" { return false; }
    if find_tool("icpx") == () { return false; }
    env["ONEAPI_ROOT"] != ()
}
