set_name("gnat");
set_homepage("https://gcc.gnu.org/onlinedocs/gnat/");
set_binary("gnatmake");
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("ld", "gnatmake");
set_toolset("ar", "ar");

set_extensions([".adb", ".ads"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");
set_flag("opt", "z", "-Os");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-gnatwa");
set_flag("warnings", "all",     "-gnatwa -gnatwl");
set_flag("warnings", "error",   "-gnatwa -gnatwl -gnatwe");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_standard("ada2005", "-gnat05");
set_standard("ada2012", "-gnat12");
set_standard("ada2022", "-gnat22");

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

set_linking("ada", #{
    abi:        "ada",
    compatible: ["c"],
    linker:     "",
    extensions: [".adb", ".ads"],
});

fn check() {
    find_tool("gnatmake") != ()
}
