// Shared C++ language parameters — identical across all C++ compilers.
// Compiler-specific things (modules, pch, sanitizers) stay in the compiler file.

extensions = [".cpp", ".cppm", ".ixx", ".mpp", ".cc", ".cxx", ".c++"];

defaults["std"] = "c++17";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".ixx", ".mpp", ".cc", ".cxx", ".c++"],
};
