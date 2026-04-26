set_name("clang");
set_homepage("https://clang.llvm.org/");
set_binary("clang++");
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("cc",    "clang");
set_toolset("cxx",   "clang++");
set_toolset("ld",    "clang++");
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
    compile_binary: "clang",
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
    find_tool("clang++") != ()
}
