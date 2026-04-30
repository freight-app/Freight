// NVIDIA HPC SDK — nvc (C), nvc++ (C++), nvfortran (Fortran).
// Successor to the Portland Group (PGI) compilers. Used in scientific and HPC workloads.
// Supports OpenACC, OpenMP offloading, and CUDA Fortran.

compiler["name"]          = "nvhpc";
compiler["homepage"]      = "https://developer.nvidia.com/hpc-sdk";
compiler["binary"]        = "nvc++";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "nvc\\+\\+ (\\d+\\.\\d+)";
compiler["extensions"]    = [".cpp", ".cc", ".cxx", ".c++", ".c", ".f90", ".f95", ".f", ".F90"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
// NVIDIA HPC SDK is available on x86_64 and aarch64 (Grace) Linux only.
compiler["supported_archs"] = ["x86_64", "aarch64"];
compiler["supported_os"]    = ["linux"];

toolset["cc"]  = "nvc";
toolset["cxx"] = "nvc++";
toolset["ld"]  = "nvc++";
toolset["ar"]  = "ar";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-O2";
flags["opt"]["z"] = "-O2";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

// NVHPC uses -Minform= for verbosity; -Werror is supported for many diagnostics.
flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "-Minform=warn";
flags["warnings"]["error"]   = "-Minform=warn -Werror";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

flags["cpu_ext"]["template"] = "-m{name}";

standards["c11"]   = "-std=c11";
standards["c17"]   = "-std=c17";
standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["f2003"] = "-Mstandard";
standards["f2008"] = "-Mstandard";
standards["f2018"] = "-Mstandard";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MMD -MF {path}";
structure["target"]       = "";
structure["sysroot"]      = "";

modules["style"] = "none";

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran"],
    compile_binary: "nvc",
    linker:         "",
    extensions:     [".c"],
};

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cc", ".cxx", ".c++"],
};

linking["fortran"] = #{
    abi:            "fortran",
    compatible:     ["c"],
    compile_binary: "nvfortran",
    linker:         "",
    extensions:     [".f90", ".f95", ".f", ".F90"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "aarch64" { return false; }
    if os != "linux" { return false; }
    find_tool("nvc++") != ()
}
