name          = "gfortran";
family        = "gnu";
sanitizers = ["address", "undefined"];
homepage      = "https://gcc.gnu.org/fortran/";
binary        = "gfortran";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];
passthrough        = false;
passthrough_prefix = "";

toolset["ld"]    = "gfortran";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-Os";
opt["z"] = "-Os";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra -Wpedantic";
warnings["error"]   = "-Wall -Wextra -Wpedantic -Werror";

lto["true"]  = "-flto";
lto["false"] = "";


sanitize = "-fsanitize={values}";

standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-cpp -MMD -MF {path}";
target       = "";
sysroot      = "--sysroot={path}";

modules["style"] = "none";

linking["fortran"] = #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
};

fn check() {
    find_tool("gfortran") != ()
}
