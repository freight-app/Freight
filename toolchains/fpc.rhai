// Free Pascal Compiler — open-source Pascal and Object Pascal.
// fpc compiles and links in a single step; there is no separate -c flag.
// Set `compile_only = ""` and let fpc produce the final binary directly.
//
// Unit files (.ppu + .o) are produced automatically alongside the binary.
// Object Pascal classes, generics, and RTTI are all supported.
//
// Supported targets: Linux, Windows, macOS, FreeBSD, Haiku, embedded (AVR, ARM).

name          = "fpc";
family        = "";
sanitizers = [];
homepage      = "https://www.freepascal.org/";
binary        = "fpc";
version_arg   = "-iV";
version_regex = "(\\d+\\.\\d+\\.\\d+)";
extensions    = [".pas", ".pp", ".p", ".lpr"];
passthrough        = false;
passthrough_prefix = "";

toolset["ld"] = "fpc";
toolset["ar"] = "ar";

flags["opt"]["0"] = "";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-O2 -Os";
flags["opt"]["z"] = "-O2 -Os";

flags["debug"]["true"]  = "-g -gd -gl";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "-w-";
flags["warnings"]["default"] = "";
flags["warnings"]["all"]     = "-vw";
flags["warnings"]["error"]   = "-vw -Sew";

// FPC has no LTO; -O3 already enables cross-unit inlining.
flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-Xs";
flags["strip"]["false"] = "";

standards["delphi7"] = "-Mdelphi";
standards["objfpc"]  = "-Mobjfpc";
standards["fpc"]     = "-Mfpc";
standards["iso"]     = "-Miso";

// -Fu adds a unit path; -Fi adds an include path; -I is an alias for -Fi.
include_dir  = "-I{path}";
define       = "-d{name}";
define_value = "-d{name}";
output       = "-o{path}";
// fpc always compiles and links in one pass; compile_only is not supported.
compile_only = "";
dep_file     = "";
target       = "";
sysroot      = "";

modules["style"] = "none";

linking["pascal"] = #{
    abi:        "c",
    compatible: ["c"],
    linker:     "",
    extensions: [".pas", ".pp", ".p", ".lpr"],
};

fn check() {
    find_tool("fpc") != ()
}
