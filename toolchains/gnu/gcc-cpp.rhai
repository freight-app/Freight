include("_gnu-base");

let _gxx = detect_gxx();
let _gcc = gcc_from_gxx(_gxx);

name       = "gcc-cpp";
homepage   = "https://gcc.gnu.org/";
binary     = _gxx;
extensions = [".cpp", ".cppm", ".cc", ".cxx", ".c++"];

toolset["cc"]  = _gcc;
toolset["cxx"] = _gxx;
toolset["ld"]  = _gxx;

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";

modules["style"]         = "gcc";
modules["enable_flag"]   = "-fmodules-ts";
modules["compile_miu"]   = "-fmodule-output={pcm_path}";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-fmodule-header";

pch["compile"]   = "-x c++-header";
pch["use"]       = "-include {header_path}";
pch["extension"] = ".gch";

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
};

fn check() {
    for b in ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"] {
        if find_tool(b) != () { return true; }
    }
    false
}
