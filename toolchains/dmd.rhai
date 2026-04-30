// D reference compiler. Flag conventions also apply to LDC/GDC.
// D has no -std= flag; language version tracks the compiler version.

set_name("dmd");
set_homepage("https://dlang.org/");
set_binary("dmd");
set_version_arg("--version");
set_version_regex("v(\\d+\\.\\d+\\.\\d+)");

set_toolset("ld", "dmd");

set_extensions([".d"]);

set_flag("opt", "0", "");
set_flag("opt", "1", "-O");
set_flag("opt", "2", "-O");
set_flag("opt", "3", "-O -release");
set_flag("opt", "s", "-O");
set_flag("opt", "z", "-O");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-wi");
set_flag("warnings", "all",     "-w");
set_flag("warnings", "error",   "-w");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-L-s");
set_flag("strip", "false", "");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-version={name}");
set_structure("define_value", "-version={name}");
set_structure("output",       "-of={path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "");
set_structure("target",       "");
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
    find_tool("dmd") != ()
}
