// Shared Intel oneAPI base — loaded via include in icpx.rhai and ifx.rhai.
// opt["s"], opt["z"], warnings, lto, dep_file, target, sysroot, and version_regex
// differ between compilers and must be set in the individual files.

kind               = "compiler";
family             = "intel";
version_arg        = "--version";
passthrough        = false;
passthrough_prefix = "";
// Intel oneAPI is available on x86/x86_64 Linux and Windows.
supported_archs = ["x86", "x86_64"];
supported_os    = ["linux", "windows"];
// ONEAPI_ROOT is exported by setvars.sh / setvars.bat; its absence means
// the environment has not been initialised and compilation will fail.
required_env = ["ONEAPI_ROOT"];

opt["0"] = "-O0";
opt["1"] = "-O1";
opt["2"] = "-O2";
opt["3"] = "-O3";

dbg = "-g";

include_dir  = "-I{path}";
define       = "-D{name}";
define_value = "-D{name}={value}";
output       = "-o {path}";
compile_only = "-c";

modules["style"] = "none";
