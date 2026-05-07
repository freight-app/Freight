// Shared Fortran language parameters — identical across gfortran and flang.
// nvfortran is excluded: different extensions, different standard flags (-Mstandard).

extensions = [".f90", ".f95", ".f03", ".f08", ".f", ".F90"];

defaults["std"] = "f2018";

standards["f95"]   = "-std=f95";
standards["f2003"] = "-std=f2003";
standards["f2008"] = "-std=f2008";
standards["f2018"] = "-std=f2018";

// Fortran does not use C++ stdlib or runtime flags.
stdlib  = #{};
runtime = #{};

linking["fortran"] = #{
    abi:        "fortran",
    compatible: ["c"],
    linker:     "",
    extensions: [".f90", ".f95", ".f03", ".f08", ".f", ".F90"],
};
