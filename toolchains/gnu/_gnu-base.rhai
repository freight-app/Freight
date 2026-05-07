// Shared GNU compiler defaults — included by gcc-cpp.rhai, gcc-c.rhai, gfortran.rhai.
// Top-level assignments here run in the including file's scope; any field can be
// overridden by an assignment after the include() line.

// ── Binary detection helpers ──────────────────────────────────────────────────

fn detect_gxx() {
    for b in ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"] {
        if find_tool(b) != () { return b; }
    }
    "g++"
}

fn gcc_from_gxx(gxx) {
    if gxx == "g++" { "gcc" } else { "gcc" + gxx.sub_string(3) }
}

// ── Identity defaults ─────────────────────────────────────────────────────────

family        = "gnu";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
passthrough        = false;
passthrough_prefix = "";
sanitizer_options  = ["address", "undefined", "thread", "leak"];

toolset["ar"]    = "ar";
toolset["strip"] = "strip";

// ── Flags ─────────────────────────────────────────────────────────────────────

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

sanitize = "-fsanitize={values}";
cpu_ext  = "-m{name}";

stdlib["libstdc++"] = "";
stdlib["none"]      = "-nostdlib";

runtime["glibc"]  = "";
runtime["musl"]   = "-static";
runtime["bionic"] = "";
runtime["none"]   = "-nostdlib -nodefaultlibs";

// ── Structure ─────────────────────────────────────────────────────────────────

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "";
sysroot      = "--sysroot={path}";

// ── Architecture flags ────────────────────────────────────────────────────────

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
