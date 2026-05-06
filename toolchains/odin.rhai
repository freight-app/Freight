// Odin — systems programming language by Bill Hall (gingerbill).
// Uses `odin build <file> -file -build-mode:obj` to produce a relocatable
// object file, allowing freight to link it together with other languages.
//
// Cross-compilation: `-target:<arch>-<os>` (e.g. `arm64-macos`, `amd64-linux`).
// Debug info: `-debug` flag.
//
// NOTE: Odin does not have a preprocessor. Conditional compilation uses
// `when` expressions. The `define` field produces `-define:<name>=<value>`
// which maps to Odin build-time constant overrides.

name          = "odin";
family        = "";
sanitizers = [];
homepage      = "https://odin-lang.org/";
binary        = "odin";
version_arg   = "version";
version_regex = "odin version (\\S+)";
extensions    = [".odin"];
passthrough        = false;
passthrough_prefix = "";
// Always pass `-file` so that a single source file is compiled, not a whole package.
always_flags       = ["build", "%src%", "-file", "-build-mode:obj"];

toolset["ld"]    = "odin";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

// Odin optimisation levels.
flags["opt"]["0"] = "-o:none";
flags["opt"]["1"] = "-o:minimal";
flags["opt"]["2"] = "-o:speed";
flags["opt"]["3"] = "-o:aggressive";
flags["opt"]["s"] = "-o:size";
flags["opt"]["z"] = "-o:size";

flags["debug"]["true"]  = "-debug";
flags["debug"]["false"] = "";

// Odin does not have -Wall; it reports all issues by default.
flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "";
flags["warnings"]["error"]   = "";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

include_dir  = "-collection:pkg={path}";
define       = "-define:{name}=true";
define_value = "-define:{name}={value}";
output       = "-out:{path}";
compile_only = "";
dep_file     = "";
target       = "-target:{triple}";
sysroot      = "";

modules["style"] = "none";

linking["odin"] = #{
    abi:        "c",
    compatible: ["c"],
    linker:     "",
    extensions: [".odin"],
};

fn check() {
    find_tool("odin") != ()
}
