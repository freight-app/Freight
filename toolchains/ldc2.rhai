// LDC — LLVM-based D compiler. Flag-compatible with DMD for common settings;
// adds LLVM ThinLTO, better optimisation, and native cross-compilation via --mtriple.

compiler["name"]          = "ldc2";
compiler["homepage"]      = "https://github.com/ldc-developers/ldc";
compiler["binary"]        = "ldc2";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "LDC - the LLVM D compiler \\((\\d+\\.\\d+\\.\\d+)\\)";
compiler["extensions"]    = [".d"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["ld"]    = "ldc2";
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
flags["warnings"]["default"] = "-wi";
flags["warnings"]["all"]     = "-w";
flags["warnings"]["error"]   = "-w";

flags["lto"]["true"]  = "--flto=full";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "--strip";
flags["strip"]["false"] = "";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-d-version={name}";
structure["define_value"] = "-d-version={name}";
structure["output"]       = "-of={path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "";
structure["target"]       = "--mtriple={triple}";
structure["sysroot"]      = "";

modules["style"] = "none";

linking["d"] = #{
    abi:        "d",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".d"],
};

fn check() {
    find_tool("ldc2") != ()
}
