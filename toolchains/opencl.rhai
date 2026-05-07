// Compiles OpenCL C kernel files (.cl) using clang in OpenCL mode.
// Host code (C or C++) is compiled separately by the gcc/clang template.

name          = "opencl";
kind          = "compiler";
family        = "";
sanitizer_options    = [];
homepage      = "https://www.khronos.org/opencl/";
binary        = "clang";
version_arg   = "--version";
version_regex = "\\b(\\d+\\.\\d+\\.\\d+)\\b";
extensions    = [".cl"];
passthrough        = false;
passthrough_prefix = "";
always_flags = ["-x", "cl"];
// OpenCL kernel compilation uses clang in -x cl mode; the final binary
// is linked by the host C++ application that drives the OpenCL runtime.
requires_toolchain = ["cpp"];

toolset["ld"] = "clang";

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-Os";
opt["z"] = "-Oz";

dbg["true"]  = "-g";
dbg["false"] = "";

warnings["none"]    = "";
warnings["default"] = "-Wall";
warnings["all"]     = "-Wall -Wextra";
warnings["error"]   = "-Wall -Wextra -Werror";

lto["true"]  = "";
lto["false"] = "";


standards["CL1.0"] = "-cl-std=CL1.0";
standards["CL1.1"] = "-cl-std=CL1.1";
standards["CL1.2"] = "-cl-std=CL1.2";
standards["CL2.0"] = "-cl-std=CL2.0";
standards["CL3.0"] = "-cl-std=CL3.0";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MF {path}";
target       = "";
sysroot      = "";

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
