// Shared LLVM/Clang base — loaded via include in clang, clang++, flang.
// Variables declared with `let` (_cxx, _cc) are visible in the calling file after eval.

// ── Binary detection ──────────────────────────────────────────────────────────

let _cxx = "clang++";
for _b in ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"] {
    if find_tool(_b) != () { _cxx = _b; break; }
}
// "clang++".len() == 7; sub_string(7) strips the "clang++" prefix.
let _cc = if _cxx == "clang++" { "clang" } else { "clang" + _cxx.sub_string(7) };

// ── Identity defaults ─────────────────────────────────────────────────────────

kind          = "compiler";
family        = "llvm";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
passthrough        = false;
passthrough_prefix = "";

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

// Clang supports selecting the C++ standard library at both compile and link time.
stdlib["libc++"]    = "-stdlib=libc++";
stdlib["libstdc++"] = "-stdlib=libstdc++";
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
target       = "--target={triple}";
sysroot      = "--sysroot={path}";
