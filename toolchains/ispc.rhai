// Intel SPMD Program Compiler — vectorising compiler for CPU SIMD.
// Outputs a .o + a C header. The host links the .o from C or C++.

set_name("ispc");
set_homepage("https://ispc.github.io/");
set_binary("ispc");
set_version_arg("--version");
set_version_regex("(\\d+\\.\\d+\\.\\d+)");

set_toolset("ld", "ispc");

set_extensions([".ispc"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-O2");
set_flag("opt", "z", "-O2");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "--woff");
set_flag("warnings", "default", "");
set_flag("warnings", "all",     "");
set_flag("warnings", "error",   "--werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "");
set_flag("strip", "false", "");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "");
set_structure("dep_file",     "-MMM {path}");
set_structure("target",       "");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(false, "");

// ISPC compiler binaries are published for x86_64 and aarch64.
set_supported_archs(["x86_64", "aarch64"]);

set_linking("ispc", #{
    abi:        "ispc",
    compatible: ["c++", "c"],
    linker:     "",
    extensions: [".ispc"],
});

fn check() {
    let h = arch();
    if h != "x86_64" && h != "aarch64" { return false; }
    find_tool("ispc") != ()
}
