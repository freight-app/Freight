include "_gnu-base";
include "../languages/_cpp";

name       = "g++";
homepage   = "https://gcc.gnu.org/";
binary     = _gxx;

toolset["cc"]  = _gcc;
toolset["cxx"] = _gxx;
toolset["ld"]  = _gxx;

modules["style"]         = "gcc";
modules["enable_flag"]   = "-fmodules-ts";
modules["compile_miu"]   = "-fmodule-output={pcm_path}";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";

pch["compile"]   = "-x c++-header";
pch["use"]       = "-include {header_path}";
pch["extension"] = ".gch";

// ── compiler_option handlers ──────────────────────────────────────────────────
// Declare options users can set under [compiler.g++] in freight.toml.

// [compiler.g++] march = "native"  →  -march=native
compiler_option("march", |val, _ver| {
    add_flag(`-march=${val}`);
});

fn check() {
    for b in ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"] {
        if find_tool(b) != () { return true; }
    }
    false
}
