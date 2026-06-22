// Build dependencies with CMake and wire their headers + libraries into the build.
//
//   [dependencies]
//   zlib          = { version = "1.3", external = true }  # fetched; core won't build it
//   cmake-builder = "0.1"
//   [cmake]
//   build   = "zlib"                  # a name, or ["zlib", "png"]
//   defines = ["BUILD_SHARED_LIBS=OFF"]

let names = if type_of(CFG.build) == "string" { [CFG.build] } else { CFG.build };

for name in names {
    if !(name in PKGS) { throw "[cmake] build: '" + name + "' is not a dependency"; }
    let src = PKGS[name].dir;                       // .pkgs/<name> or a path dep
    if !is_dir(src) { throw "'" + name + "' is not fetched yet: " + src; }

    let build  = join(OUT_DIR, name);               // out-of-source build tree
    let prefix = join(build, "install");            // staging prefix (under target/)

    // ── Freight.cmake: steer dependency resolution to freight + record deps ────
    // Injected via CMAKE_PROJECT_TOP_LEVEL_INCLUDES (runs before the project's
    // own logic). It prepends freight package prefixes to CMAKE_PREFIX_PATH (so
    // find_package / config mode / vcpkg's toolchain resolve freight's copy
    // first), points pkg-config at freight's .pc dirs, makes FetchContent prefer
    // find_package over a git fetch, and records every find_package to a report.
    let report = join(build, "freight-report.txt");
    let fc =
        "if(FREIGHT_PREFIXES)\n" +
        "  list(PREPEND CMAKE_PREFIX_PATH ${FREIGHT_PREFIXES})\n" +
        "endif()\n" +
        "if(FREIGHT_PC_PATH AND DEFINED ENV{PKG_CONFIG_PATH})\n" +
        "  set(ENV{PKG_CONFIG_PATH} ${FREIGHT_PC_PATH}:$ENV{PKG_CONFIG_PATH})\n" +
        "elseif(FREIGHT_PC_PATH)\n" +
        "  set(ENV{PKG_CONFIG_PATH} ${FREIGHT_PC_PATH})\n" +
        "endif()\n" +
        "set(FETCHCONTENT_TRY_FIND_PACKAGE_MODE ALWAYS)\n" +
        "macro(find_package)\n" +
        "  if(FREIGHT_REPORT)\n" +
        "    file(APPEND \"${FREIGHT_REPORT}\" \"find_package ${ARGV0}\\n\")\n" +
        "  endif()\n" +
        "  _find_package(${ARGV})\n" +
        "endmacro()\n";
    makedirs(build);
    let freight_cmake = join(build, "Freight.cmake");
    write_text(freight_cmake, fc);
    write_text(report, "");

    // ── Configure ────────────────────────────────────────────────────────────
    let args = ["-S", src, "-B", build,
                "-DCMAKE_BUILD_TYPE=" + (if PROFILE == "release" { "Release" } else { "Debug" }),
                "-DCMAKE_INSTALL_LIBDIR=lib",     // force prefix/lib (distros vary: lib64)
                "-DCMAKE_INSTALL_PREFIX=" + prefix,
                "-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES=" + freight_cmake,
                "-DFREIGHT_REPORT=" + report];
    // Freight package prefixes / .pc dirs to resolve against (when the pipeline
    // provides them for transitive foreign deps).
    if "prefixes" in CFG { args.push("-DFREIGHT_PREFIXES=" + CFG.prefixes); }
    if "pc_path" in CFG { args.push("-DFREIGHT_PC_PATH=" + CFG.pc_path); }
    if "defines" in CFG { for d in CFG.defines { args.push("-D" + d); } }
    // Cross builds: TARGET.os / TARGET.arch / TARGET.triple are in scope here;
    // hand CMake a -DCMAKE_TOOLCHAIN_FILE=... when TARGET.triple != "".
    run("cmake", args);

    // Report what the project resolved (so freight learns its dependencies).
    if exists(report) {
        for line in lines(read_text(report)) {
            if line != "" { print("freight[cmake] " + name + ": " + line); }
        }
    }

    // ── Build + install ────────────────────────────────────────────────────────
    run("cmake", ["--build", build, "--parallel"]);
    run("cmake", ["--install", build]);

    // ── Wire results back into the freight build ───────────────────────────────
    add_include_dir(join(prefix, "include"));
    for d in [join(prefix, "lib"), join(prefix, "lib64")] {
        if is_dir(d) {
            link_dir(d);
            for lib in glob(d + "/*.a") + glob(d + "/*.so") { link_lib(lib); }
        }
    }
}
