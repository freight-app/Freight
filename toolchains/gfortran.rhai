set_name("gfortran");
set_homepage("https://gcc.gnu.org/fortran/");
set_binary("gfortran");
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("ld",    "gfortran");
set_toolset("ar",    "ar");
set_toolset("strip", "strip");

set_extensions([".f90", ".f95", ".f03", ".f08", ".f", ".F90"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");
set_flag("opt", "z", "-Os");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall -Wextra -Wpedantic");
set_flag("warnings", "error",   "-Wall -Wextra -Wpedantic -Werror");

set_flag("lto", "true",  "-flto");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_flag("sanitize", "template", "-fsanitize={values}");

set_standard("f95",   "-std=f95");
set_standard("f2003", "-std=f2003");
set_standard("f2008", "-std=f2008");
set_standard("f2018", "-std=f2018");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-cpp -MMD -MF {path}");
set_structure("target",       "");
set_structure("sysroot",      "--sysroot={path}");

set_module_style("none");
set_passthrough(false, "");

set_linking("fortran", #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
});

fn check() {
    find_tool("gfortran") != ()
}
