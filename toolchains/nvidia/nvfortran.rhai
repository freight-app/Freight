include "_nvhpc-base";

name       = "nvfortran";
homepage   = "https://developer.nvidia.com/hpc-sdk";
binary     = "nvfortran";
extensions = [".f90", ".f95", ".f", ".F90"];
sanitizer_options = ["address"];

toolset["ld"] = "nvfortran";

// Fortran doesn't use C++ stdlib or runtime flags.
stdlib  = #{};
runtime = #{};

standards["f2003"] = "-Mstandard";
standards["f2008"] = "-Mstandard";
standards["f2018"] = "-Mstandard";

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
    find_tool("nvfortran") != ()
}
