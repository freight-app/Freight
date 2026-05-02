// Objective-C / Objective-C++ via Clang.
// Uses the same clang binary as the C/C++ template but registers .m/.mm
// extensions so freight routes them through this template automatically.
//
// ARC (Automatic Reference Counting) is enabled by default via always_flags.
// Pass `flags = ["-fno-objc-arc"]` in [compiler] to opt out.
//
// On macOS, link with -framework Foundation (and others as needed):
//   [dependencies]
//   Foundation = { system = "Foundation" }

name          = "objc";
family        = "";
homepage      = "https://clang.llvm.org/docs/ObjectiveCLiterals.html";
binary        = "clang";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".m", ".mm"];
passthrough        = false;
passthrough_prefix = "";
supported_os    = ["macos", "ios", "linux"];
// ARC is enabled by default; required for modern Objective-C code.
always_flags    = ["-fobjc-arc"];

toolset["cc"]    = "clang";
toolset["cxx"]   = "clang++";
toolset["ld"]    = "clang";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Oz";

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

flags["sanitize"]["template"] = "-fsanitize={values}";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "--target={triple}";
sysroot      = "--sysroot={path}";

modules["style"] = "none";

linking["objc"] = #{
    abi:        "c",
    compatible: ["c", "cpp"],
    linker:     "",
    extensions: [".m"],
};

linking["objcpp"] = #{
    abi:        "c++",
    compatible: ["c", "cpp", "objc"],
    compile_binary: "clang++",
    linker:     "",
    extensions: [".mm"],
};

fn check() {
    // Objective-C is supported wherever clang is installed.
    find_tool("clang") != ()
}
