include "_gnu-base";
include "../languages/_c";

name       = "gcc";
homepage   = "https://gcc.gnu.org/";
binary     = _gcc;
extensions = [".c", ".s", ".S"];

toolset["cc"]  = _gcc;
toolset["cxx"] = _gxx;
toolset["ld"]  = _gxx;

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _gcc,
    linker:         "",
    extensions:     [".c", ".s", ".S"],
};

fn check() { find_tool("gcc") != () }
