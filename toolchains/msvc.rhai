// MSVC — Microsoft Visual C++ (cl.exe / link.exe)
//
// Requires a Developer Command Prompt environment (vcvarsall.bat already sourced).
// If the INCLUDE / LIB variables are not set, cl.exe cannot find the CRT headers;
// crane will emit a warning in that case.
//
// Dependency tracking uses /showIncludes (stdout mode) instead of -MMD.
// LTO uses two separate flags: /GL at compile time and /LTCG at link time.

set_name("msvc");
set_homepage("https://visualstudio.microsoft.com/");

set_toolset("cc",    "cl.exe");
set_toolset("cxx",   "cl.exe");
set_toolset("ld",    "link.exe");
set_toolset("ar",    "lib.exe");
set_toolset("strip", "");

set_binary("link.exe");
set_version_arg("");           // cl.exe prints version on stderr with no args
set_version_regex("Version (\\d+\\.\\d+\\.\\d+\\.\\d+)");

set_extensions([".cpp", ".cc", ".cxx", ".c++", ".c"]);

set_flag("opt", "0", "/Od");
set_flag("opt", "1", "/O1");
set_flag("opt", "2", "/O2");
set_flag("opt", "3", "/Ox");
set_flag("opt", "s", "/O1 /Os");
set_flag("opt", "z", "/O1 /Os");

set_flag("debug", "true",  "/Zi /FS");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "/W0");
set_flag("warnings", "default", "/W3");
set_flag("warnings", "all",     "/W4");
set_flag("warnings", "error",   "/W4 /WX");

// Compile-step LTO flag — link-step uses lto_link below
set_flag("lto", "true",  "/GL");
set_flag("lto", "false", "");

// Link-step LTO flag (replaces lto at link time)
set_flag("lto_link", "true",  "/LTCG");
set_flag("lto_link", "false", "");

set_flag("strip", "true",  "");   // no strip flag; omit PDB at link time instead
set_flag("strip", "false", "");

set_flag("sanitize", "template", "/fsanitize={values}");
set_flag("cpu_ext",  "template", "");   // /arch:AVX2 etc. set via compiler.flags

set_standard("c++17",   "/std:c++17");
set_standard("c++20",   "/std:c++20");
set_standard("c++23",   "/std:c++latest");
set_standard("c17",     "/std:c17");
set_standard("c11",     "/std:c11");

set_structure("include_dir",   "/I{path}");
set_structure("define",        "/D{name}");
set_structure("define_value",  "/D{name}={value}");
set_structure("output_obj",    "/Fo{path}");   // compile-step output
set_structure("output_bin",    "/Fe{path}");   // link-step output (also used by link.exe /OUT)
set_structure("compile_only",  "/c");
set_structure("dep_file",      "/showIncludes");
set_structure("dep_file_mode", "stdout");       // parse Note: including file: lines
set_structure("system_lib",    "{name}.lib");   // MSVC links by filename, not -l
set_structure("target",        "");             // cross via dedicated toolchain
set_structure("sysroot",       "");

set_module_style("none");   // C++20 modules not yet supported in this template

set_passthrough(false, "");

set_supported_os(["windows"]);

set_linking("c", #{
    abi:            "c",
    compatible:     [],
    compile_binary: "cl.exe",
    linker:         "",
    extensions:     [".c"],
});

set_linking("cpp", #{
    abi:        "c++",
    compatible: ["c"],
    linker:     "",
    extensions: [".cpp", ".cc", ".cxx", ".c++"],
});

fn check() {
    if find_tool("cl.exe") == () { return false; }
    // Warn if the vcvars environment is not set up
    if env("INCLUDE") == () {
        // Detection still succeeds; crane will fail at compile time with a clear cl.exe error
    }
    true
}
