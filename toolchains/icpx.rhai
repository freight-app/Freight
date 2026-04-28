// Intel oneAPI DPC++ compiler — SYCL heterogeneous compute.
// -fsycl is injected as an always-flag; host code is standard C++.

set_name("icpx");
set_homepage("https://www.intel.com/content/www/us/en/developer/tools/oneapi/dpc-compiler.html");
set_binary("icpx");
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("ld", "icpx");

set_extensions([".cpp", ".cc", ".cxx", ".c++", ".sycl"]);

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

set_flag("lto", "true",  "-flto");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_flag("sanitize", "template", "-fsanitize={values}");

set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");
set_standard("c++23", "-std=c++23");

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

// Intel oneAPI is available on x86/x86_64 Linux and Windows.
set_supported_archs(["x86", "x86_64"]);
set_supported_os(["linux", "windows"]);

add_always_flag("-fsycl");

set_linking("sycl", #{
    abi:        "sycl",
    compatible: ["c++", "c", "fortran"],
    linker:     "",
    extensions: [".sycl", ".cpp", ".cc", ".cxx"],
});

fn check() {
    let h = arch();
    if h != "x86_64" && h != "x86" { return false; }
    let o = os();
    if o != "linux" && o != "windows" { return false; }
    if find_tool("icpx") == () { return false; }
    // oneAPI environment must be initialised (setvars.sh / setvars.bat).
    env("ONEAPI_ROOT") != () || env("SETVARS_COMPLETED") != ()
}
