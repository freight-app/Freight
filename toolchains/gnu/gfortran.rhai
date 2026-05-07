include("_gnu-base");

name       = "gfortran";
homepage   = "https://gcc.gnu.org/fortran/";
binary     = "gfortran";
extensions = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];

toolset["ld"]  = "gfortran";

// gfortran needs -cpp to enable the C preprocessor before dep tracking.
dep_file = "-cpp -MMD -MF {path}";

// Fortran doesn't use C++ stdlib or runtime flags.
stdlib  = #{};
runtime = #{};

sanitizer_options = ["address", "undefined"];
sanitize = "-fsanitize={values}";
cpu_ext  = "";

standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

linking["fortran"] = #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
};

fn check() { find_tool("gfortran") != () }
