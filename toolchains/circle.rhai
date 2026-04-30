// Circle — experimental C++23+ compiler with extended compile-time features.
// Clang-compatible flag set; fully supports C++20 modules via the Clang two-step.
// https://www.circle-lang.org/

compiler["name"]          = "circle";
compiler["homepage"]      = "https://www.circle-lang.org/";
compiler["binary"]        = "circle";
compiler["version_arg"]   = "--version";
// Circle versions are typically reported as a build number ("build 212") or
// embed the underlying Clang version — accept either form.
compiler["version_regex"] = "(?:build (\\d+))|(\\d+\\.\\d+\\.\\d+)";
compiler["extensions"]    = [".cpp", ".cc", ".cxx", ".c++", ".c"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
// Circle prebuilt binaries are published for x86_64 and aarch64 Linux only.
compiler["supported_archs"] = ["x86_64", "aarch64"];
compiler["supported_os"]    = ["linux"];

toolset["ld"]    = "circle";
toolset["ar"]    = "ar";
toolset["strip"] = "strip";

flags["opt"]["0"] = "-O0";
flags["opt"]["1"] = "-O1";
flags["opt"]["2"] = "-O2";
flags["opt"]["3"] = "-O3";
flags["opt"]["s"] = "-Os";
flags["opt"]["z"] = "-Oz";

flags["debug"]["true"]  = "-g";
flags["debug"]["false"] = "";

flags["warnings"]["none"]    = "";
flags["warnings"]["default"] = "-Wall";
flags["warnings"]["all"]     = "-Wall -Wextra";
flags["warnings"]["error"]   = "-Wall -Wextra -Werror";

flags["lto"]["true"]  = "-flto";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "-s";
flags["strip"]["false"] = "";

flags["sanitize"]["template"] = "-fsanitize={values}";
flags["cpu_ext"]["template"]  = "-m{name}";

standards["c++17"] = "-std=c++17";
standards["c++20"] = "-std=c++20";
standards["c++23"] = "-std=c++23";
standards["c11"]   = "-std=c11";
standards["c17"]   = "-std=c17";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MMD -MF {path}";
structure["target"]       = "--target={triple}";
structure["sysroot"]      = "--sysroot={path}";

modules["style"]         = "clang";
modules["precompile"]    = "--precompile";
modules["import_module"] = "-fmodule-file={name}={pcm_path}";
modules["header_unit"]   = "-x c++-header";

linking["c"] = #{
    abi:            "c",
    compatible:     ["asm"],
    compile_binary: "circle",
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
    let h = arch;
    if h != "x86_64" && h != "aarch64" { return false; }
    if os != "linux" { return false; }
    find_tool("circle") != ()
}
