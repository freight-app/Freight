// Intel Fortran Compiler (ifx) — LLVM-based, replaces the classic ifort.
// Part of Intel oneAPI. Supports Fortran 2018 and OpenMP 5.x offloading.

include "_intel-base";

name          = "ifx";
homepage      = "https://www.intel.com/content/www/us/en/developer/tools/oneapi/fortran-compiler.html";
binary        = "ifx";
version_regex = "(\\d+\\.\\d+\\.\\d+)";
extensions    = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];
sanitizer_options = ["address", "undefined"];

toolset["ld"]    = "ifx";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

opt["s"] = "-O2";
opt["z"] = "-O2";

warnings["none"]    = "-warn none";
warnings["default"] = "";
warnings["all"]     = "-warn all";
warnings["error"]   = "-warn all -warn errors";

// -ipo = inter-procedural optimisation (Intel's LTO).
lto = "-ipo";

// -cpp enables the C preprocessor; -MMD/-MF generate Makefile dep files.
dep_file = "-cpp -MMD -MF {path}";
target   = "";
sysroot  = "";

standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

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
