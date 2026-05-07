// Intel Fortran Compiler (ifx) — LLVM-based, replaces the classic ifort.
// Part of Intel oneAPI. Supports Fortran 2018 and OpenMP 5.x offloading.

name          = "ifx";
family        = "intel";
sanitizer_options = ["address", "undefined"];
homepage      = "https://www.intel.com/content/www/us/en/developer/tools/oneapi/fortran-compiler.html";
binary        = "ifx";
version_arg   = "--version";
version_regex = "(\\d+\\.\\d+\\.\\d+)";
extensions    = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];
passthrough        = false;
passthrough_prefix = "";
// Intel Fortran (ifx) is available on x86/x86_64 Linux and Windows.
supported_archs = ["x86", "x86_64"];
supported_os    = ["linux", "windows"];
// ONEAPI_ROOT is exported by setvars.sh / setvars.bat.
required_env = ["ONEAPI_ROOT"];

toolset["ld"]    = "ifx";
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

warnings["none"]    = "-warn none";
warnings["default"] = "";
warnings["all"]     = "-warn all";
warnings["error"]   = "-warn all -warn errors";

// -ipo = inter-procedural optimisation (Intel's LTO).
lto["true"]  = "-ipo";
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
// -cpp enables the C preprocessor; -MMD/-MF generate Makefile dep files.
dep_file     = "-cpp -MMD -MF {path}";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["fortran"] = #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "x86" { return false; }
    let o = os;
    if o != "linux" && o != "windows" { return false; }
    if find_tool("ifx") == () { return false; }
    env["ONEAPI_ROOT"] != ()
}
