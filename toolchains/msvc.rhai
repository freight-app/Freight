// MSVC — Microsoft Visual C++ (cl.exe / link.exe)
//
// Requires a Developer Command Prompt environment (vcvarsall.bat already sourced).
// If the INCLUDE / LIB variables are not set, cl.exe cannot find the CRT headers;
// freight will emit a warning in that case.
//
// Dependency tracking uses /showIncludes (stdout mode) instead of -MMD.
// LTO uses two separate flags: /GL at compile time and /LTCG at link time.

compiler["name"]          = "msvc";
compiler["homepage"]      = "https://visualstudio.microsoft.com/";
compiler["binary"]        = "link.exe";
compiler["version_arg"]   = "";           // cl.exe prints version on stderr with no args
compiler["version_regex"] = "Version (\\d+\\.\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".cpp", ".cc", ".cxx", ".c++", ".c"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
compiler["supported_os"] = ["windows"];

toolset["cc"]    = "cl.exe";
toolset["cxx"]   = "cl.exe";
toolset["ld"]    = "link.exe";
toolset["ar"]    = "lib.exe";
toolset["strip"] = "";

flags["opt"]["0"] = "/Od";
flags["opt"]["1"] = "/O1";
flags["opt"]["2"] = "/O2";
flags["opt"]["3"] = "/Ox";
flags["opt"]["s"] = "/O1 /Os";
flags["opt"]["z"] = "/O1 /Os";

flags["debug"]["true"]  = "/Zi /FS";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "/W0";
flags["warnings"]["default"] = "/W3";
flags["warnings"]["all"]     = "/W4";
flags["warnings"]["error"]   = "/W4 /WX";

// Compile-step LTO flag — link-step uses lto_link below
flags["lto"]["true"]  = "/GL";
flags["lto"]["false"] = "";

// Link-step LTO flag (replaces lto at link time)
flags["lto_link"]["true"]  = "/LTCG";
flags["lto_link"]["false"] = "";

flags["strip"]["true"]  = "";   // no strip flag; omit PDB at link time instead
flags["strip"]["false"] = "";

flags["sanitize"]["template"] = "/fsanitize={values}";
flags["cpu_ext"]["template"]  = "";   // /arch:AVX2 etc. set via compiler.flags

standards["c++17"]   = "/std:c++17";
standards["c++20"]   = "/std:c++20";
standards["c++23"]   = "/std:c++latest";
standards["c17"]     = "/std:c17";
standards["c11"]     = "/std:c11";

structure["include_dir"]   = "/I{path}";
structure["define"]        = "/D{name}";
structure["define_value"]  = "/D{name}={value}";
structure["output_obj"]    = "/Fo{path}";   // compile-step output
structure["output_bin"]    = "/Fe{path}";   // link-step output (also used by link.exe /OUT)
structure["compile_only"]  = "/c";
structure["dep_file"]      = "/showIncludes";
structure["dep_file_mode"] = "stdout";       // parse Note: including file: lines
structure["system_lib"]    = "{name}.lib";   // MSVC links by filename, not -l
structure["target"]        = "";             // cross via dedicated toolchain
structure["sysroot"]       = "";

modules["style"] = "none";   // C++20 modules not yet supported in this template

linking["c"] = #{
    abi:            "c",
    compatible:     [],
    compile_binary: "cl.exe",
    linker:         "",
    extensions:     [".c"],
};

linking["cpp"] = #{
    abi:        "c++",
    compatible: ["c"],
    linker:     "",
    extensions: [".cpp", ".cc", ".cxx", ".c++"],
};

fn check() {
    if find_tool("cl.exe") == () { return false; }
    // Warn if the vcvars environment is not set up
    if env["INCLUDE"] == () {
        // Detection still succeeds; freight will fail at compile time with a clear cl.exe error
    }
    true
}
