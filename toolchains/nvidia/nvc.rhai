include "_nvhpc-base";

name       = "nvc";
homepage   = "https://developer.nvidia.com/hpc-sdk";
binary     = "nvc";
extensions = [".c"];
sanitizer_options = ["address"];

toolset["cc"] = "nvc";
toolset["ld"] = "nvc";

standards["c11"] = "-std=c11";
standards["c17"] = "-std=c17";

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran"],
    compile_binary: "nvc",
    linker:         "",
    extensions:     [".c"],
};

fn check() {
    let h = arch;
    if h != "x86_64" && h != "aarch64" { return false; }
    if os != "linux" { return false; }
    find_tool("nvc") != ()
}
