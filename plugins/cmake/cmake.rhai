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

    // ── Configure ────────────────────────────────────────────────────────────
    let args = ["-S", src, "-B", build,
                "-DCMAKE_BUILD_TYPE=" + (if PROFILE == "release" { "Release" } else { "Debug" }),
                "-DCMAKE_INSTALL_LIBDIR=lib",     // force prefix/lib (distros vary: lib64)
                "-DCMAKE_INSTALL_PREFIX=" + prefix];
    if "defines" in CFG { for d in CFG.defines { args.push("-D" + d); } }
    // Cross builds: TARGET.os / TARGET.arch / TARGET.triple are in scope here;
    // hand CMake a -DCMAKE_TOOLCHAIN_FILE=... when TARGET.triple != "".
    run("cmake", args);

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
