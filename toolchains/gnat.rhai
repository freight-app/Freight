name          = "gnat";
homepage      = "https://gcc.gnu.org/onlinedocs/gnat/";
binary        = "gnatmake";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".adb", ".ads"];
passthrough        = false;
passthrough_prefix = "";

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

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "";
target       = "";
sysroot      = "";

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
