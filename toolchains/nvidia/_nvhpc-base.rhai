// Shared NVIDIA HPC SDK base — nvc (C), nvc++ (C++), nvfortran (Fortran).
// Successor to the Portland Group (PGI) compilers. Supports OpenACC, OpenMP offloading.

kind          = "compiler";
family        = "nvidia";
version_arg   = "--version";
// Generic X.Y match works for nvc, nvc++, and nvfortran version output alike.
version_regex = "(\\d+\\.\\d+)";
passthrough        = false;
passthrough_prefix = "";
supported_archs = ["x86_64", "aarch64"];
supported_os    = ["linux"];

toolset["ar"] = "ar";

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";
opt["s"] = "-O2";
opt["z"] = "-O2";

dbg["true"]  = "-g";
dbg["false"] = "";

// NVHPC uses -Minform= for verbosity; -Werror is supported for many diagnostics.
warnings["none"]    = "";
warnings["default"] = "";
warnings["all"]     = "-Minform=warn";
warnings["error"]   = "-Minform=warn -Werror";

// NVHPC LTO (IPA) requires a two-stage build: compile with -Mipa, then link with -Mipa.
// A single-flag mapping would inject -Mipa at compile time but not at link time, which
// produces incorrect output. Use [compiler.nvc++] always_flags for manual IPA setup.
lto["true"]  = "";
lto["false"] = "";

cpu_ext = "-m{name}";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";
dep_file     = "-MMD -MF {path}";
target       = "";
sysroot      = "";

modules["style"] = "none";
