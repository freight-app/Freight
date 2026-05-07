// Shared GNU compiler defaults — included into per-language files via `include "_gnu-base"`.
// Top-level code runs in the calling file's scope; `let` variables (e.g. _gxx, _gcc)
// are visible after the include returns. Any field can be overridden after the include.

// ── Binary detection ──────────────────────────────────────────────────────────
// _gxx / _gcc are available to the language file after eval returns.

let _gxx = "g++";
for _b in ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"] {
    if find_tool(_b) != () { _gxx = _b; break; }
}
let _gcc = if _gxx == "g++" { "gcc" } else { "gcc" + _gxx.sub_string(3) };

// ── Identity defaults ─────────────────────────────────────────────────────────

kind          = "compiler";
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

if arch == "x86_64" || arch == "x64" {
    load_flags["cc"]  += ["-m64"];
    load_flags["cxx"] += ["-m64"];
    load_flags["ld"]  += ["-m64"];
} else if arch == "i386" || arch == "x86" {
    load_flags["cc"]  += ["-m32"];
    load_flags["cxx"] += ["-m32"];
    load_flags["ld"]  += ["-m32"];
}
