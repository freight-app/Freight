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

pch["compile"]      = "-x c++-header";
pch["use"]          = "-include-pch {pch_path}";
pch["extension"]    = ".pch";
pch["clangd_flag"]  = "-include {header_path}";

linking["objcpp"] = #{
    abi:        "c++",
    compatible: ["c", "objc"],
    linker:     "",
    extensions: [".mm"],
};

// ── compiler_option handlers ──────────────────────────────────────────────────
// Declare options users can set under [compiler.clang++] in freight.toml.

// [compiler.clang++] march = "native"  →  -march=native
compiler_option("march", |ctx| {
    add_flag(`-march=${ctx.value}`);
});

// [compiler.clang++] lto_mode = "thin"  →  -flto=thin  (requires Clang ≥ 3.9)
compiler_option("lto_mode", |ctx| {
    if ctx.value == "thin" && version_gte(ctx.version, "3.9") {
        add_flag("-flto=thin");
    } else if ctx.value == "full" {
        add_flag("-flto=full");
    }
});

// [compiler.clang++] min_version = "14.0"  →  error if clang++ < 14.0
compiler_option("min_version", |ctx| {
    if !version_gte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " is below required minimum " + ctx.value;
    }
});

// [compiler.clang++] max_version = "17.0"  →  error if clang++ > 17.0
compiler_option("max_version", |ctx| {
    if !version_lte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " exceeds required maximum " + ctx.value;
    }
});

fn check() {
    for b in ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"] {
        if find_tool(b) != () { return true; }
    }
    false
}
