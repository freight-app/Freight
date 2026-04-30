// NVIDIA HPC SDK — nvc (C), nvc++ (C++), nvfortran (Fortran).
// Successor to the Portland Group (PGI) compilers. Used in scientific and HPC workloads.
// Supports OpenACC, OpenMP offloading, and CUDA Fortran.

set_name("nvhpc");
set_homepage("https://developer.nvidia.com/hpc-sdk");
set_binary("nvc++");
set_version_arg("--version");
set_version_regex("nvc\\+\\+ (\\d+\\.\\d+)");

set_toolset("cc",  "nvc");
set_toolset("cxx", "nvc++");
set_toolset("ld",  "nvc++");
set_toolset("ar",  "ar");

set_extensions([".cpp", ".cc", ".cxx", ".c++", ".c", ".f90", ".f95", ".f", ".F90"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-O2");
set_flag("opt", "z", "-O2");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

// NVHPC uses -Minform= for verbosity; -Werror is supported for many diagnostics.
set_flag("warnings", "none",    "");
set_flag("warnings", "default", "");
set_flag("warnings", "all",     "-Minform=warn");
set_flag("warnings", "error",   "-Minform=warn -Werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_flag("strip", "true",  "");
set_flag("strip", "false", "");

set_flag("cpu_ext", "template", "-m{name}");

set_standard("c11",   "-std=c11");
set_standard("c17",   "-std=c17");
set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");
set_standard("f2003", "-Mstandard");
set_standard("f2008", "-Mstandard");
set_standard("f2018", "-Mstandard");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-MMD -MF {path}");
set_structure("target",       "");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(false, "");

// NVIDIA HPC SDK is available on x86_64 and aarch64 (Grace) Linux only.
set_supported_archs(["x86_64", "aarch64"]);
set_supported_os(["linux"]);

set_linking("c", #{
    abi:            "c",
    compatible:     ["fortran"],
    compile_binary: "nvc",
    linker:         "",
    extensions:     [".c"],
});

set_linking("cpp", #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cc", ".cxx", ".c++"],
});

set_linking("fortran", #{
    abi:            "fortran",
    compatible:     ["c"],
    compile_binary: "nvfortran",
    linker:         "",
    extensions:     [".f90", ".f95", ".f", ".F90"],
});

fn check() {
    let h = arch();
    if h != "x86_64" && h != "aarch64" { return false; }
    if os() != "linux" { return false; }
    find_tool("nvc++") != ()
}
