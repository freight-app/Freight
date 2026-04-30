compiler["name"]          = "gfortran";
compiler["homepage"]      = "https://gcc.gnu.org/fortran/";
compiler["binary"]        = "gfortran";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
compiler["extensions"]    = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["ld"]    = "gfortran";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Os";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra -Wpedantic";
flags["warnings"]["error"]   = "-Wall -Wextra -Wpedantic -Werror";

flags["lto"]["true"]  = "-flto";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

flags["sanitize"]["template"] = "-fsanitize={values}";

standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-cpp -MMD -MF {path}";
structure["target"]       = "";
structure["sysroot"]      = "--sysroot={path}";

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
