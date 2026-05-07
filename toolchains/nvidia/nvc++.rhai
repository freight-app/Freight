include "_nvhpc-base";

name       = "nvc++";
homepage   = "https://developer.nvidia.com/hpc-sdk";
binary     = "nvc++";
extensions = [".cpp", ".cc", ".cxx", ".c++"];
sanitizer_options = ["address"];

toolset["cc"]  = "nvc";
toolset["cxx"] = "nvc++";
toolset["ld"]  = "nvc++";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cc", ".cxx", ".c++"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "aarch64" { return false; }
    if os != "linux" { return false; }
    find_tool("nvc++") != ()
}
