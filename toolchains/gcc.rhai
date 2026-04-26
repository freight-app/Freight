set_name("gcc");
set_homepage("https://gcc.gnu.org/");
set_binary("g++");
set_version_arg("--version");
set_version_regex("\\b(\\d+\\.\\d+\\.\\d+)\\b");

set_toolset("cc",    "gcc");
set_toolset("cxx",   "g++");
set_toolset("ld",    "g++");
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
    compile_binary: "gcc",
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
    find_tool("g++") != ()
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
