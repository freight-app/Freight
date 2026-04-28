set_name("nvcc");
set_homepage("https://developer.nvidia.com/cuda-toolkit");
set_binary("nvcc");
set_version_arg("--version");
set_version_regex("release (\\d+\\.\\d+)");

set_toolset("ld", "nvcc");

set_extensions([".cu", ".cuh"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-O2");
set_flag("opt", "z", "-O2");

set_flag("debug", "true",  "-g -G");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall -Wextra");
set_flag("warnings", "error",   "-Wall -Wextra -Werror");

set_flag("lto", "true",  "");
set_flag("lto", "false", "");

set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-MD -MF {path}");
set_structure("target",       "");
set_structure("sysroot",      "");

set_module_style("none");
set_passthrough(true, "-Xcompiler");

// CUDA Toolkit is available on x86_64 and aarch64 (Jetson / Grace) Linux and Windows.
// macOS support was dropped in CUDA 11.6.
set_supported_archs(["x86_64", "aarch64"]);
set_supported_os(["linux", "windows"]);

add_always_flag("--expt-relaxed-constexpr");
add_always_flag("--extended-lambda");

set_linking("cuda", #{
    abi:        "cuda",
    compatible: ["c++", "c", "fortran"],
    linker:     "c++",
    extensions: [".cu", ".cuh"],
});

fn check() {
    let h = arch();
    if h != "x86_64" && h != "aarch64" { return false; }
    let o = os();
    if o != "linux" && o != "windows" { return false; }
    if find_tool("nvcc") == () { return false; }
    // CUDA_PATH (Windows) or CUDA_HOME / CUDA_ROOT (Linux) indicate a full toolkit install.
    env("CUDA_PATH") != () || env("CUDA_HOME") != () || env("CUDA_ROOT") != ()
}
