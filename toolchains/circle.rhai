// Circle — experimental C++23+ compiler with extended compile-time features.
// Clang-compatible flag set; fully supports C++20 modules via the Clang two-step.
// https://www.circle-lang.org/

set_name("circle");
set_homepage("https://www.circle-lang.org/");
set_binary("circle");
set_version_arg("--version");
// Circle versions are typically reported as a build number ("build 212") or
// embed the underlying Clang version — accept either form.
set_version_regex("(?:build (\\d+))|(\\d+\\.\\d+\\.\\d+)");

set_toolset("ld",    "circle");
set_toolset("ar",    "ar");
set_toolset("strip", "strip");

set_extensions([".cpp", ".cc", ".cxx", ".c++", ".c"]);

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
set_flag("warnings", "all",     "-Wall -Wextra");
set_flag("warnings", "error",   "-Wall -Wextra -Werror");

set_flag("lto", "true",  "-flto");
set_flag("lto", "false", "");

set_flag("strip", "true",  "-s");
set_flag("strip", "false", "");

set_flag("sanitize", "template", "-fsanitize={values}");
set_flag("cpu_ext",  "template", "-m{name}");

set_standard("c++17", "-std=c++17");
set_standard("c++20", "-std=c++20");
set_standard("c++23", "-std=c++23");
set_standard("c11",   "-std=c11");
set_standard("c17",   "-std=c17");

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

// Circle prebuilt binaries are published for x86_64 and aarch64 only.
set_supported_archs(["x86_64", "aarch64"]);

set_linking("c", #{
    abi:            "c",
    compatible:     ["asm"],
    compile_binary: "circle",
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
    let h = arch();
    if h != "x86_64" && h != "aarch64" { return false; }
    find_tool("circle") != ()
}
