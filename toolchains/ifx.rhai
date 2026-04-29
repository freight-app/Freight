// Intel Fortran Compiler (ifx) — LLVM-based, replaces the classic ifort.
// Part of Intel oneAPI. Supports Fortran 2018 and OpenMP 5.x offloading.

set_name("ifx");
set_homepage("https://www.intel.com/content/www/us/en/developer/tools/oneapi/fortran-compiler.html");
set_binary("ifx");
set_version_arg("--version");
set_version_regex("(\\d+\\.\\d+\\.\\d+)");

set_toolset("ld",    "ifx");
set_toolset("ar",    "ar");
set_toolset("strip", "strip");

set_extensions([".f90", ".f95", ".f03", ".f08", ".f", ".F90"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-O2");
set_flag("opt", "z", "-O2");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "-warn none");
set_flag("warnings", "default", "");
set_flag("warnings", "all",     "-warn all");
set_flag("warnings", "error",   "-warn all -warn errors");

// -ipo = inter-procedural optimisation (Intel's LTO).
set_flag("lto", "true",  "-ipo");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_standard("f95",   "-std=f95");
set_standard("f2003", "-std=f2003");
set_standard("f2008", "-std=f2008");
set_standard("f2018", "-std=f2018");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
// -cpp enables the C preprocessor; -MMD/-MF generate Makefile dep files.
set_structure("dep_file",     "-cpp -MMD -MF {path}");
set_structure("target",       "");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(false, "");

// Intel Fortran (ifx) is available on x86/x86_64 Linux and Windows.
set_supported_archs(["x86", "x86_64"]);
set_supported_os(["linux", "windows"]);

// ONEAPI_ROOT is exported by setvars.sh / setvars.bat.
set_required_env(["ONEAPI_ROOT"]);

set_linking("fortran", #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
});

fn check() {
    let h = arch();
    if h != "x86_64" && h != "x86" { return false; }
    let o = os();
    if o != "linux" && o != "windows" { return false; }
    if find_tool("ifx") == () { return false; }
    env("ONEAPI_ROOT") != ()
}
