// D reference compiler. Flag conventions also apply to LDC/GDC.
// D has no -std= flag; language version tracks the compiler version.

compiler["name"]          = "dmd";
compiler["homepage"]      = "https://dlang.org/";
compiler["binary"]        = "dmd";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "v(\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".d"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";

toolset["ld"] = "dmd";

flags["opt"]["0"] = "";
flags["opt"]["1"] = "-O";
flags["opt"]["2"] = "-O";
flags["opt"]["3"] = "-O -release";
flags["opt"]["s"] = "-O";
flags["opt"]["z"] = "-O";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-wi";
flags["warnings"]["all"]     = "-w";
flags["warnings"]["error"]   = "-w";

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-L-s";
flags["strip"]["false"] = "";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-version={name}";
structure["define_value"] = "-version={name}";
structure["output"]       = "-of={path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "";
structure["target"]       = "";
structure["sysroot"]      = "";

modules["style"] = "none";

linking["d"] = #{
    abi:        "d",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".d"],
};

fn check() {
    find_tool("dmd") != ()
}
