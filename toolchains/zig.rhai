// Zig — systems programming language with a self-hosted compiler.
// Uses `zig build-obj` to compile a single .zig file to a relocatable object.
// Linking uses `zig build-exe`; both invocations are driven via always_flags.
//
// Cross-compilation is a first-class feature: `-target aarch64-linux-musl`
// works without installing a sysroot — Zig ships its own libc headers.
//
// NOTE: Zig's import system differs from C — there is no header search path
// in the traditional sense. `-I` passes paths to the C translator (@cImport).
//
// NOTE: Zig defines (-Dname=value) are for the build system, not the compiler.
// Use comptime constants instead of preprocessor defines in .zig source.

name          = "zig";
family        = "";
homepage      = "https://ziglang.org/";
binary        = "zig";
version_arg   = "version";
version_regex = "(\\d+\\.\\d+\\.\\d+)";
extensions    = [".zig"];
passthrough        = false;
passthrough_prefix = "";
always_flags       = ["build-obj"];

toolset["ld"]    = "zig";
toolset["ar"]    = "zig";
toolset["strip"] = "strip";

// Zig optimisation modes.
flags["opt"]["0"] = "-O Debug";
flags["opt"]["1"] = "-O ReleaseSafe";
flags["opt"]["2"] = "-O ReleaseSafe";
flags["opt"]["3"] = "-O ReleaseFast";
flags["opt"]["s"] = "-O ReleaseSmall";
flags["opt"]["z"] = "-O ReleaseSmall";

flags["debug"]["true"]  = "";
flags["debug"]["false"] = "";

// Zig does not have -Wall; it has its own always-on diagnostics.
flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "";
flags["warnings"]["error"]   = "";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "--strip";
flags["strip"]["false"] = "";

// -I passes include paths to @cImport's C preprocessor.
include_dir  = "-I{path}";
define       = "";
define_value = "";
output       = "-femit-bin={path}";
compile_only = "";
dep_file     = "";
target       = "-target {triple}";
sysroot      = "";

modules["style"] = "none";

linking["zig"] = #{
    abi:        "zig",
    compatible: ["c"],
    linker:     "",
    extensions: [".zig"],
};

fn check() {
    find_tool("zig") != ()
}
