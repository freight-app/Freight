// Compiles OpenCL C kernel files (.cl) using clang in OpenCL mode.
// Host code (C or C++) is compiled separately by the gcc/clang template.

set_name("opencl");
set_homepage("https://www.khronos.org/opencl/");
set_binary("clang");
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("ld", "clang");

set_extensions([".cl"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");
set_flag("opt", "z", "-Oz");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall -Wextra");
set_flag("warnings", "error",   "-Wall -Wextra -Werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "");
set_flag("strip", "false", "");

set_standard("CL1.0", "-cl-std=CL1.0");
set_standard("CL1.1", "-cl-std=CL1.1");
set_standard("CL1.2", "-cl-std=CL1.2");
set_standard("CL2.0", "-cl-std=CL2.0");
set_standard("CL3.0", "-cl-std=CL3.0");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-MF {path}");
set_structure("target",       "");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(false, "");

add_always_flag("-x");
add_always_flag("cl");

// OpenCL kernel compilation uses clang in -x cl mode; the final binary
// is linked by the host C++ application that drives the OpenCL runtime.
set_requires_toolchain(["cpp"]);

set_linking("opencl", #{
    abi:        "opencl",
    compatible: ["c++", "c"],
    linker:     "c++",
    extensions: [".cl"],
});

fn check() {
    find_tool("clang") != ()
}
