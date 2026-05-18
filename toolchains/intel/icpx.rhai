// Intel oneAPI DPC++ compiler — SYCL heterogeneous compute.
// -fsycl is injected as an always-flag; host code is standard C++.

include "_intel-base";

name          = "icpx";
homepage      = "https://www.intel.com/content/www/us/en/developer/tools/oneapi/dpc-compiler.html";
binary        = "icpx";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".cpp", ".cc", ".cxx", ".c++", ".sycl"];
sanitizer_options = ["address", "undefined", "thread", "leak"];
always_flags  = ["-fsycl"];

toolset["ld"] = "icpx";

opt["s"] = "-Os";
opt["z"] = "-Oz";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra";
warnings["error"]   = "-Wall -Wextra -Werror";

lto = "-flto";

sanitize = "-fsanitize={values}";

dep_file = "-MMD -MF {path}";
target   = "--target={triple}";
sysroot  = "--sysroot={path}";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

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
