// GCC — GNU Compiler Collection. Handles C, C++, and AT&T assembly (.s/.S).
// Probes versioned binaries (g++-14, g++-13, …) when the plain g++ symlink is absent,
// which is common on Debian/Ubuntu where versioned packages don't create the generic name.

let _gxx_bins = ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"];
let _gxx = "g++";
for _b in _gxx_bins {
    if find_tool(_b) != () {
        _gxx = _b;
        break;
    }
}
// Derive the C-compiler binary: g++-13 → gcc-13, g++ → gcc.
let _gcc = if _gxx == "g++" { "gcc" } else { "gcc" + _gxx.sub_string(3) };

set_name("gcc");
set_homepage("https://gcc.gnu.org/");
set_binary(_gxx);
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("cc",    _gcc);
set_toolset("cxx",   _gxx);
set_toolset("ld",    _gxx);
set_toolset("ar",    "ar");
set_toolset("strip", "strip");

set_extensions([".cpp", ".cppm", ".cc", ".cxx", ".c++", ".c"]);

set_flag("opt", "0", "-O0");
set_flag("opt", "1", "-O1");
set_flag("opt", "2", "-O2");
set_flag("opt", "3", "-O3");
set_flag("opt", "s", "-Os");
set_flag("opt", "z", "-Oz");

set_flag("debug", "true",  "-g");
set_flag("debug", "false", "");

set_flag("warnings", "none",    "");
set_flag("warnings", "default", "-Wall");
set_flag("warnings", "all",     "-Wall -Wextra -Wpedantic");
set_flag("warnings", "error",   "-Wall -Wextra -Wpedantic -Werror");

set_flag("lto", "true",  "-flto");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_flag("sanitize", "template", "-fsanitize={values}");
set_flag("cpu_ext",  "template", "-m{name}");

set_standard("c11",   "-std=c11");
set_standard("c17",   "-std=c17");
set_standard("c23",   "-std=c23");
set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");
set_standard("c++23", "-std=c++23");

set_structure("include_dir",  "-I{path}");
set_structure("define",       "-D{name}");
set_structure("define_value", "-D{name}={value}");
set_structure("output",       "-o {path}");
set_structure("compile_only", "-c");
set_structure("dep_file",     "-MMD -MF {path}");
set_structure("target",       "");
set_structure("sysroot",      "--sysroot={path}");

set_module_style("gcc", #{
    enable_flag:   "-fmodules-ts",
    compile_miu:   "-fmodule-output={pcm_path}",
    import_module: "-fmodule-file={name}={pcm_path}",
    header_unit:   "-fmodule-header",
});

set_passthrough(false, "");

set_linking("c", #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _gcc,
    linker:         "",
    extensions:     [".c", ".s", ".S"],
});

set_linking("cpp", #{
    abi:        "c++",
    compatible: ["c", "fortran"],
    linker:     "",
    extensions: [".cpp", ".cppm", ".cc", ".cxx", ".c++"],
});

fn check() {
    let bins = ["g++", "g++-14", "g++-13", "g++-12", "g++-11", "g++-10"];
    for b in bins {
        if find_tool(b) != () { return true; }
    }
    false
}

fn load() {
    if arch == "x86_64" || arch == "x64" {
        add_flags("cc",  "-m64");
        add_flags("cxx", "-m64");
        add_flags("ld",  "-m64");
    } else if arch == "i386" || arch == "x86" {
        add_flags("cc",  "-m32");
        add_flags("cxx", "-m32");
        add_flags("ld",  "-m32");
    }
}
