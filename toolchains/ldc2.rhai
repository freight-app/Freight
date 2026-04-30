// LDC — LLVM-based D compiler. Flag-compatible with DMD for common settings;
// adds LLVM ThinLTO, better optimisation, and native cross-compilation via --mtriple.

set_name("ldc2");
set_homepage("https://github.com/ldc-developers/ldc");
set_binary("ldc2");
set_version_arg("--version");
set_version_regex("LDC - the LLVM D compiler \\((\\d+\\.\\d+\\.\\d+)\\)");

set_toolset("ld",    "ldc2");
set_toolset("ar",    "ar");
set_toolset("strip", "strip");

set_extensions([".d"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");
set_flag("opt", "z", "-Oz");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-wi");
set_flag("warnings", "all",     "-w");
set_flag("warnings", "error",   "-w");

set_flag("lto", "true",  "--flto=full");
set_flag("lto", "false", "");

set_flag("strip", "true",  "--strip");
set_flag("strip", "false", "");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-d-version={name}");
set_structure("define_value", "-d-version={name}");
set_structure("output",       "-of={path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "");
set_structure("target",       "--mtriple={triple}");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(false, "");

set_linking("d", #{
    abi:        "d",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".d"],
});

fn check() {
    find_tool("ldc2") != ()
}
