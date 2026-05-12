// GDC — GCC-based D compiler.

include "_gnu-base";

name       = "gdc";
homepage   = "https://gcc.gnu.org/";
binary     = "gdc";
extensions = [".d"];
sanitizer_options = ["address", "undefined", "thread", "leak"];

toolset["ld"] = "gdc";

warnings["none"]    = "";
warnings["default"] = "";
warnings["all"]     = "-Wall";
warnings["error"]   = "-Wall -Werror";

include_dir  = "-I{path}";
define       = "-fversion={name}";
define_value = "-fversion={name}";

modules["style"] = "none";

linking["d"] = #{
    abi:        "d",
    compatible: ["c"],
    linker:     "",
    extensions: [".d"],
};

fn check() { find_tool("gdc") != () }
