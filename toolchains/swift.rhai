// Swift — Apple's Swift compiler.
// Supports the standard compile-to-object model: `swiftc -c foo.swift -o foo.o`.
// Conditional compilation flags use -D but accept only identifiers (no values).
// Cross-compilation is via `-target <triple>` (e.g. `aarch64-apple-macosx14.0`).

name          = "swift";
family        = "";
homepage      = "https://swift.org/";
binary        = "swiftc";
version_arg   = "--version";
version_regex = "Swift version (\\d+\\.\\d+(?:\\.\\d+)?)";
extensions    = [".swift"];
passthrough        = false;
passthrough_prefix = "";
supported_os    = ["macos", "linux"];

toolset["ld"]    = "swiftc";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-Onone";
flags["opt"]["1"] = "-O";
flags["opt"]["2"] = "-O";
flags["opt"]["3"] = "-O";
flags["opt"]["s"] = "-Osize";
flags["opt"]["z"] = "-Osize";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

// Swift uses -warnings-as-errors rather than -Werror.
// There is no -Wall equivalent; the compiler always emits its built-in set.
flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "";
flags["warnings"]["error"]   = "-warnings-as-errors";

flags["lto"]["true"]  = "-lto=full";
flags["lto"]["false"] = "";

// `swiftc` does not have a standalone strip flag; `strip` is a separate tool.
flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

// Swift conditional compilation: -DFLAG (identifier only, no =value form).
include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}";
output       = "-o {path}";
compile_only = "-c";
// -emit-dependencies writes a GCC-style .d file; path is derived from -o.
dep_file     = "-emit-dependencies-path {path}";
target       = "-target {triple}";
sysroot      = "-sdk {path}";

modules["style"] = "none";

linking["swift"] = #{
    abi:        "swift",
    compatible: ["c"],
    linker:     "",
    extensions: [".swift"],
};

fn check() {
    find_tool("swiftc") != ()
}
