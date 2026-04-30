// Flang — LLVM Fortran compiler. The binary is named `flang` in recent releases
// but `flang-new` on distributions that package older transitional builds.
// This script probes both names and uses whichever is found.

let _flang = if find_tool("flang") != () { "flang" } else { "flang-new" };

compiler["name"]          = "flang";
compiler["homepage"]      = "https://flang.llvm.org/";
compiler["binary"]        = _flang;
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "flang(?:-new)? version (\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["ld"]    = _flang;
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-O2";
flags["opt"]["z"] = "-O2";

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

standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MMD -MF {path}";
structure["target"]       = "--target={triple}";
structure["sysroot"]      = "--sysroot={path}";

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
