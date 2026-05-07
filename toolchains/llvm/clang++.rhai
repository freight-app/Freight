include "_llvm-base";
include "../languages/_cpp";

name       = "clang++";
homepage   = "https://clang.llvm.org/";
binary     = _cxx;
sanitizer_options = ["address", "undefined", "thread", "memory", "leak", "hwaddress", "dataflow", "cfi", "safestack"];

toolset["cc"]  = _cc;
toolset["cxx"] = _cxx;
toolset["ld"]  = _cxx;

modules["style"]         = "clang";
modules["precompile"]    = "--precompile";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-x c++-header";

pch["compile"]   = "-x c++-header";
pch["use"]       = "-include-pch {pch_path}";
pch["extension"] = ".pch";

// ── compiler_option handlers ──────────────────────────────────────────────────
// Declare options users can set under [compiler.clang++] in freight.toml.

// [compiler.clang++] march = "native"  →  -march=native
compiler_option("march", |val, _ver| {
    add_flag(`-march=${val}`);
});

// [compiler.clang++] lto_mode = "thin"  →  -flto=thin  (requires Clang ≥ 3.9)
compiler_option("lto_mode", |val, ver| {
    if val == "thin" && version_gte(ver, "3.9") {
        add_flag("-flto=thin");
    } else if val == "full" {
        add_flag("-flto=full");
    }
});

fn check() {
    for b in ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"] {
        if find_tool(b) != () { return true; }
    }
    false
}
