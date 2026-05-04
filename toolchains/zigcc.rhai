// zig cc — Zig's built-in Clang-backed C/C++ compiler.
//
// `zig cc` is a drop-in GCC/Clang replacement that adds:
//   • Zero-setup cross-compilation (`-target aarch64-linux-musl` just works)
//   • Bundled musl and glibc headers — no sysroot installation needed
//   • Reproducible builds — zig cc hashes all inputs
//
// Use this instead of `clang` when you need cross-compilation without a full
// sysroot, or when targeting musl instead of glibc.

name          = "zig-cc";
family        = "";
supported_sanitizers = ["address", "undefined", "thread"];
homepage      = "https://ziglang.org/learn/overview/#cross-compiling-is-a-first-class-use-case";
binary        = "zig";
version_arg   = "version";
version_regex = "(\\d+\\.\\d+\\.\\d+)";
extensions    = [".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"];
passthrough        = false;
passthrough_prefix = "";
// Subcommand injected as the first argument: `zig cc [flags]`
always_flags       = ["cc"];

toolset["cc"]    = "zig";
toolset["cxx"]   = "zig";
toolset["ld"]    = "zig";
toolset["ar"]    = "zig";
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
flags["warnings"]["all"]     = "-Wall -Wextra -Wpedantic";
flags["warnings"]["error"]   = "-Wall -Wextra -Wpedantic -Werror";

flags["lto"]["true"]  = "-flto";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

flags["sanitize"]["template"] = "-fsanitize={values}";
flags["cpu_ext"]["template"]  = "-m{name}";

standards["c11"]   = "-std=c11";
standards["c17"]   = "-std=c17";
standards["c23"]   = "-std=c23";
standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
// Cross-compilation: `zig cc -target aarch64-linux-musl`
target       = "-target {triple}";
sysroot      = "--sysroot={path}";

modules["style"] = "none";

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: "zig",
    linker:         "",
    extensions:     [".c", ".s", ".S"],
};

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
};

fn check() {
    find_tool("zig") != ()
}
