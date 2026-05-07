// Flang — LLVM Fortran compiler. The binary is named `flang` in recent releases
// but `flang-new` on distributions that package older transitional builds.

include "_llvm-base";
include "../languages/_fortran";

let _flang = if find_tool("flang") != () { "flang" } else { "flang-new" };

name          = "flang";
homepage      = "https://flang.llvm.org/";
binary        = _flang;
version_regex = "flang(?:-new)? version (\\d+\\.\\d+\\.\\d+)";
sanitizer_options = ["address", "undefined"];

toolset["ld"] = _flang;

// Flang maps size/speed to -O2; no dedicated -Os/-Oz support yet.
opt["s"] = "-O2";
opt["z"] = "-O2";

// Fortran compiler warnings are less granular than C/C++.
warnings["all"]   = "-Wall -Wextra";
warnings["error"] = "-Wall -Wextra -Werror";

modules["style"] = "none";

fn check() {
    find_tool("flang") != () || find_tool("flang-new") != ()
}
