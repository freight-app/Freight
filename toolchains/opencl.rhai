// Compiles OpenCL C kernel files (.cl) using clang in OpenCL mode.
// Host code (C or C++) is compiled separately by the gcc/clang template.

compiler["name"]          = "opencl";
compiler["homepage"]      = "https://www.khronos.org/opencl/";
compiler["binary"]        = "clang";
compiler["version_arg"]   = "--version";
compiler["version_regex"] = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
compiler["extensions"]    = [".cl"];
compiler["passthrough"]        = false;
compiler["passthrough_prefix"] = "";
compiler["always_flags"] = ["-x", "cl"];
// OpenCL kernel compilation uses clang in -x cl mode; the final binary
// is linked by the host C++ application that drives the OpenCL runtime.
compiler["requires_toolchain"] = ["cpp"];

toolset["ld"] = "clang";

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

flags["lto"]["true"]  = "";
flags["lto"]["false"] = "";

flags["strip"]["true"]  = "";
flags["strip"]["false"] = "";

standards["CL1.0"] = "-cl-std=CL1.0";
standards["CL1.1"] = "-cl-std=CL1.1";
standards["CL1.2"] = "-cl-std=CL1.2";
standards["CL2.0"] = "-cl-std=CL2.0";
standards["CL3.0"] = "-cl-std=CL3.0";

structure["include_dir"]  = "-I{path}";
structure["define"]       = "-D{name}";
structure["define_value"] = "-D{name}={value}";
structure["output"]       = "-o {path}";
structure["compile_only"] = "-c";
structure["dep_file"]     = "-MF {path}";
structure["target"]       = "";
structure["sysroot"]      = "";

modules["style"] = "none";

linking["opencl"] = #{
    abi:        "opencl",
    compatible: ["c++", "c"],
    linker:     "c++",
    extensions: [".cl"],
};

fn check() {
    find_tool("clang") != ()
}
