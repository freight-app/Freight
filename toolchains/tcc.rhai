// TCC — Tiny C Compiler. Extremely fast single-pass C compiler.
// No optimisation levels, no LTO, no sanitizers, and no dependency file generation
// (dep_file is empty — crane falls back to mtime-only dirty checking for TCC projects).

set_name("tcc");
set_homepage("https://bellard.org/tcc/");
set_binary("tcc");
set_version_arg("-v");
set_version_regex("version (\\d+\\.\\d+\\.\\d+)");

set_toolset("cc",  "tcc");
set_toolset("ld",  "tcc");
set_toolset("ar",  "tcc");

set_extensions([".c"]);

// TCC ignores -O flags; all levels map to empty to avoid noise.
set_flag("opt", "0", "");
set_flag("opt", "1", "");
set_flag("opt", "2", "");
set_flag("opt", "3", "");
set_flag("opt", "s", "");
set_flag("opt", "z", "");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall");
set_flag("warnings", "error",   "-Wall -Werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "");
set_flag("strip", "false", "");

set_standard("c99",  "-std=c99");
set_standard("c11",  "-std=c11");
set_standard("c17",  "-std=c17");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "");
set_structure("target",       "");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(false, "");

set_linking("c", #{
    abi:        "c",
    compatible: [],
    linker:     "",
    extensions: [".c"],
});

fn check() {
    find_tool("tcc") != ()
}
