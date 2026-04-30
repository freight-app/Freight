// TCC — Tiny C Compiler. Extremely fast single-pass C compiler.
// No optimisation levels, no LTO, no sanitizers, and no dependency file generation
// (dep_file is empty — freight falls back to mtime-only dirty checking for TCC projects).

compiler["name"]          = "tcc";
compiler["homepage"]      = "https://bellard.org/tcc/";
compiler["binary"]        = "tcc";
compiler["version_arg"]   = "-v";
compiler["version_regex"] = "version (\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".c"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["cc"]  = "tcc";
toolset["ld"]  = "tcc";
toolset["ar"]  = "tcc";

// TCC ignores -O flags; all levels map to empty to avoid noise.
flags["opt"]["0"] = "";
flags["opt"]["1"] = "";
flags["opt"]["2"] = "";
flags["opt"]["3"] = "";
flags["opt"]["s"] = "";
flags["opt"]["z"] = "";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall";
flags["warnings"]["error"]   = "-Wall -Werror";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

standards["c99"] = "-std=c99";
standards["c11"] = "-std=c11";
standards["c17"] = "-std=c17";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "";
structure["target"]       = "";
structure["sysroot"]      = "";

modules["style"] = "none";

linking["c"] = #{
    abi:        "c",
    compatible: [],
    linker:     "",
    extensions: [".c"],
};

fn check() {
    find_tool("tcc") != ()
}
