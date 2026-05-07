// Intel oneAPI DPC++ compiler — SYCL heterogeneous compute.
// -fsycl is injected as an always-flag; host code is standard C++.

name          = "icpx";
family        = "intel";
sanitizers = ["address", "undefined", "thread", "leak"];
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

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-Os";
opt["z"] = "-Oz";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra";
warnings["error"]   = "-Wall -Wextra -Werror";

lto["true"]  = "-flto";
lto["false"] = "";


sanitize = "-fsanitize={values}";

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
