// GCC — GNU Compiler Collection. Handles C, C++, and AT&T assembly (.s/.S).
// Probes versioned binaries (g++-14, g++-13, …) when the plain g++ symlink is absent,
// which is common on Debian/Ubuntu where versioned packages don't create the generic name.

let _gxx_bins = ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"];
let _gxx = "g++";
for _b in _gxx_bins {
    if find_tool(_b) != () {
        _gxx = _b;
        break;
    }
}
// Derive the C-compiler binary: g++-13 → gcc-13, g++ → gcc.
let _gcc = if _gxx == "g++" { "gcc" } else { "gcc" + _gxx.sub_string(3) };

name          = "gcc";
homepage      = "https://gcc.gnu.org/";
binary        = _gxx;
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"];
passthrough        = false;
passthrough_prefix = "";

toolset["cc"]    = _gcc;
toolset["cxx"]   = _gxx;
toolset["ld"]    = _gxx;
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

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "";
sysroot      = "--sysroot={path}";

modules["style"]         = "gcc";
modules["enable_flag"]   = "-fmodules-ts";
modules["compile_miu"]   = "-fmodule-output={pcm_path}";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";

linking["c"] = #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _gcc,
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
    let bins = ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"];
    for b in bins {
        if find_tool(b) != () { return true; }
    }
    false
}

fn load() {
    if arch == "x86_64" || arch == "x64" {
        load_flags["cc"]  += ["-m64"];
        load_flags["cxx"] += ["-m64"];
        load_flags["ld"]  += ["-m64"];
    } else if arch == "i386" || arch == "x86" {
        load_flags["cc"]  += ["-m32"];
        load_flags["cxx"] += ["-m32"];
        load_flags["ld"]  += ["-m32"];
    }
}
