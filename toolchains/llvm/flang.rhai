// Flang — LLVM Fortran compiler. The binary is named `flang` in recent releases
// but `flang-new` on distributions that package older transitional builds.
// This script probes both names and uses whichever is found.

let _flang = if find_tool("flang") != () { "flang" } else { "flang-new" };

name          = "flang";
family        = "llvm";
sanitizers = ["address", "undefined"];
homepage      = "https://flang.llvm.org/";
binary        = _flang;
version_arg   = "--version";
version_regex = "flang(?:-new)? version (\\d+\\.\\d+\\.\\d+)";
extensions    = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];
passthrough        = false;
passthrough_prefix = "";

toolset["ld"]    = _flang;
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-O2";
opt["z"] = "-O2";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra";
warnings["error"]   = "-Wall -Wextra -Werror";

lto["true"]  = "-flto";
lto["false"] = "";


standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "--target={triple}";
sysroot      = "--sysroot={path}";

modules["style"] = "none";

linking["fortran"] = #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
};

fn check() {
    find_tool("flang") != () || find_tool("flang-new") != ()
}
