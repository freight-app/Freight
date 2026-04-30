// Clang/LLVM — C, C++, and AT&T assembly. Natively multi-target via --target=.
// Probes versioned binaries (clang++-18, clang++-17, …) when the plain clang++ symlink
// is absent, which is common on Debian/Ubuntu versioned packages.

let _cxx_bins = ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"];
let _cxx = "clang++";
for _b in _cxx_bins {
    if find_tool(_b) != () {
        _cxx = _b;
        break;
    }
}
// Derive the C-compiler binary: clang++-17 → clang-17, clang++ → clang.
// "clang++".len() == 7, so sub_string(7) strips the "clang++" prefix.
let _cc = if _cxx == "clang++" { "clang" } else { "clang" + _cxx.sub_string(7) };

set_name("clang");
set_homepage("https://clang.llvm.org/");
set_binary(_cxx);
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("cc",    _cc);
set_toolset("cxx",   _cxx);
set_toolset("ld",    _cxx);
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
set_structure("target",       "--target={triple}");
set_structure("sysroot",      "--sysroot={path}");

set_module_style("clang", #{
    precompile:    "--precompile",
    import_module: "-fmodule-file={name}={pcm_path}",
    header_unit:   "-x c++-header",
});

set_passthrough(false, "");

set_linking("c", #{
    abi:            "c",
    compatible:     ["fortran", "asm"],
    compile_binary: _cc,
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
    let bins = ["clang++", "clang++-18", "clang++-17", "clang++-16", "clang++-15", "clang++-14"];
    for b in bins {
        if find_tool(b) != () { return true; }
    }
    false
}
