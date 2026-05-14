include "_gnu-base";
include "../languages/_cpp";

name       = "g++";
homepage   = "https://gcc.gnu.org/";
binary     = _gxx;

toolset["cc"]  = _gcc;
toolset["cxx"] = _gxx;
toolset["ld"]  = _gxx;

language_option("modules", "true", |ctx| {
    set_module("style",         "gcc");
    set_module("enable_flag",   "-fmodules-ts");
    set_module("compile_miu",   "-fmodule-output={pcm_path}");
    set_module("import_module", "-fmodule-file={name}={pcm_path}");
    set_module("header_unit",   "-fmodule-header");
});

language_option("pch", "", |ctx| {
    set_pch("compile",     "-x c++-header");
    set_pch("use",         "-include {header_path}");
    set_pch("extension",   ".gch");
    set_pch("clangd_flag", "-include {header_path}");
});

// ── compiler_option handlers ──────────────────────────────────────────────────
// Declare options users can set under [compiler.g++] in freight.toml.

// [compiler.g++] march = "native"  →  -march=native
compiler_option("march", |ctx| {
    add_flag(`-march=${ctx.value}`);
});

// [compiler.g++] min_version = "12.0"  →  error if g++ < 12.0
compiler_option("min_version", |ctx| {
    if !version_gte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " is below required minimum " + ctx.value;
    }
});

// [compiler.g++] max_version = "14.0"  →  error if g++ > 14.0
compiler_option("max_version", |ctx| {
    if !version_lte(ctx.version, ctx.value) {
        return ctx.name + " " + ctx.version + " exceeds required maximum " + ctx.value;
    }
});

fn check() {
    for b in ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"] {
        if find_tool(b) != () { return true; }
    }
    false
}
