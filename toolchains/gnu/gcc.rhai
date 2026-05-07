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
family        = "gnu";
sanitizer_options = ["address", "undefined", "thread", "leak"];
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

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-Os";
opt["z"] = "-Oz";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra -Wpedantic";
warnings["error"]   = "-Wall -Wextra -Wpedantic -Werror";

lto["true"]  = "-flto";
lto["false"] = "";


// GCC always uses libstdc++; no flag to select it. "-nostdlib" disables all libs.
stdlib["libstdc++"] = "";
stdlib["none"]      = "-nostdlib";

// GCC runtime selection: glibc is the default (no flag). musl requires a
// musl-targeted toolchain (e.g. x86_64-linux-musl-gcc); "-static" is added as a
// hint for fully-static musl builds, but the real switch is the cross toolchain.
runtime["glibc"]   = "";
runtime["musl"]    = "-static";
runtime["bionic"]  = "";
runtime["none"]    = "-nostdlib -nodefaultlibs";

sanitize = "-fsanitize={values}";
cpu_ext  = "-m{name}";

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

pch["compile"]   = "-x c++-header";
pch["use"]       = "-include {header_path}";
pch["extension"] = ".gch";

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
