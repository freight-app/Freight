// LDC — LLVM-based D compiler.

include "_llvm-base";

name          = "ldc2";
homepage      = "https://github.com/ldc-developers/ldc";
binary        = "ldc2";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".d"];
sanitizer_options = ["address", "thread", "memory", "undefined"];

toolset["ld"] = "ldc2";

warnings["none"]    = "";
warnings["default"] = "";
warnings["all"]     = "-wi";
warnings["error"]   = "-w";

lto["true"]  = "-flto=full";
lto["false"] = "";
sanitize = "-fsanitize={values}";

include_dir   = "-I{path}";
define        = "-d-version={name}";
define_value  = "-d-version={name}";
output        = "-of={path}";
compile_only  = "-c";
dep_file      = "";
dep_file_mode = "none";
system_lib    = "-L-l{name}";
target        = "-mtriple={triple}";
sysroot       = "";

modules["style"] = "none";

linking["d"] = #{
    abi:        "d",
    compatible: ["c"],
    linker:     "",
    extensions: [".d"],
};

compiler_option("dip1000", "false", |ctx| {
    if ctx.value == "true" { add_flag("-preview=dip1000"); }
});

fn check() { find_tool("ldc2") != () }
