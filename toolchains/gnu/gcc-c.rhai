include("_gnu-base");

let _gxx = detect_gxx();
let _gcc = gcc_from_gxx(_gxx);

name       = "gcc-c";
homepage   = "https://gcc.gnu.org/";
binary     = _gcc;
extensions = [".c", ".s", ".S"];

toolset["cc"]  = _gcc;
toolset["cxx"] = _gxx;
toolset["ld"]  = _gxx;

standards["c11"] = "-std=c11";
standards["c17"] = "-std=c17";
standards["c23"] = "-std=c23";

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _gcc,
    linker:         "",
    extensions:     [".c", ".s", ".S"],
};

fn check() { find_tool("gcc") != () }
