// AMD ROCm HIP compiler — AMD's CUDA equivalent.

set_name("hipcc");
set_homepage("https://rocm.docs.amd.com/");
set_binary("hipcc");
set_version_arg("--version");
set_version_regex("HIP version: (\\d+\\.\\d+\\.\\d+)");

set_toolset("ld", "hipcc");

set_extensions([".hip", ".cpp", ".cc", ".cxx"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");
set_flag("opt", "z", "-Oz");

set_flag("debug", "true",  "-g -ggdb");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall -Wextra");
set_flag("warnings", "error",   "-Wall -Wextra -Werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_standard("c++14", "-std=c++14");
set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-MMD -MF {path}");
set_structure("target",       "--target={triple}");
set_structure("sysroot",      "--sysroot={path}");

set_module_style("none");
set_passthrough(false, "");

set_linking("hip", #{
    abi:        "hip",
    compatible: ["c++", "c", "fortran"],
    linker:     "c++",
    extensions: [".hip"],
});

fn check() {
    find_tool("hipcc") != ()
}
