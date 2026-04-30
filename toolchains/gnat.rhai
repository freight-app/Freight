compiler["name"]          = "gnat";
compiler["homepage"]      = "https://gcc.gnu.org/onlinedocs/gnat/";
compiler["binary"]        = "gnatmake";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
compiler["extensions"]    = [".adb", ".ads"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["ld"] = "gnatmake";
toolset["ar"] = "ar";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Os";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-gnatwa";
flags["warnings"]["all"]     = "-gnatwa -gnatwl";
flags["warnings"]["error"]   = "-gnatwa -gnatwl -gnatwe";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

standards["ada2005"] = "-gnat05";
standards["ada2012"] = "-gnat12";
standards["ada2022"] = "-gnat22";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "";
structure["target"]       = "";
structure["sysroot"]      = "";

modules["style"] = "none";

linking["ada"] = #{
    abi:        "ada",
    compatible: ["c"],
    linker:     "",
    extensions: [".adb", ".ads"],
};

fn check() {
    find_tool("gnatmake") != ()
}
