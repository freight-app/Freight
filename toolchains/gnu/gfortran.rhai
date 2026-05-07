include "_gnu-base";
include "../languages/_fortran";

name       = "gfortran";
homepage   = "https://gcc.gnu.org/fortran/";
binary     = "gfortran";

toolset["ld"] = "gfortran";

// gfortran needs -cpp to enable the C preprocessor before dep tracking.
dep_file = "-cpp -MMD -MF {path}";

sanitizer_options = ["address", "undefined"];
sanitize = "-fsanitize={values}";
cpu_ext  = "";

fn check() { find_tool("gfortran") != () }
