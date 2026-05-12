include "_llvm-base";
include "../languages/_c";

name       = "clang";
homepage   = "https://clang.llvm.org/";
binary     = _cc;
extensions = [".c", ".s", ".S"];
sanitizer_options = ["address", "undefined", "thread", "memory", "leak", "hwaddress", "dataflow", "cfi", "safestack"];

toolset["cc"]  = _cc;
toolset["cxx"] = _cxx;
toolset["ld"]  = _cxx;

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _cc,
    linker:         "",
    extensions:     [".c", ".s", ".S"],
};

linking["objc"] = #{
    abi:            "objc",
    compatible:     ["c"],
    compile_binary: _cc,
    linker:         "",
    extensions:     [".m"],
};

fn check() { find_tool("clang") != () }
