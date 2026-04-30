// Clang/LLVM — C, C++, and AT&T assembly. Natively multi-target via --target=.
// Probes versioned binaries (clang++-18, clang++-17, …) when the plain clang++ symlink
// is absent, which is common on Debian/Ubuntu versioned packages.

let _cxx_bins = ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"];
let _cxx = "clang++";
for _b in _cxx_bins {
    if find_tool(_b) != () {
        _cxx = _b;
        break;
    }
}
// Derive the C-compiler binary: clang++-17 → clang-17, clang++ → clang.
// "clang++".len() == 7, so sub_string(7) strips the "clang++" prefix.
let _cc = if _cxx == "clang++" { "clang" } else { "clang" + _cxx.sub_string(7) };

compiler["name"]          = "clang";
compiler["homepage"]      = "https://clang.llvm.org/";
compiler["binary"]        = _cxx;
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
compiler["extensions"]    = [".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["cc"]    = _cc;
toolset["cxx"]   = _cxx;
toolset["ld"]    = _cxx;
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Oz";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra -Wpedantic";
flags["warnings"]["error"]   = "-Wall -Wextra -Wpedantic -Werror";

flags["lto"]["true"]  = "-flto";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

flags["sanitize"]["template"] = "-fsanitize={values}";
flags["cpu_ext"]["template"]  = "-m{name}";

standards["c11"]   = "-std=c11";
standards["c17"]   = "-std=c17";
standards["c23"]   = "-std=c23";
standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MMD -MF {path}";
structure["target"]       = "--target={triple}";
structure["sysroot"]      = "--sysroot={path}";

modules["style"]         = "clang";
modules["precompile"]    = "--precompile";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-x c++-header";

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _cc,
    linker:         "",
    extensions:     [".c", ".s", ".S"],
};

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
};

fn check() {
    let bins = ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"];
    for b in bins {
        if find_tool(b) != () { return true; }
    }
    false
}
