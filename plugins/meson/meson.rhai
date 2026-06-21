// Build dependencies with Meson and wire their headers + libraries into the build.
//
//   [dependencies]
//   glib          = { version = "2", external = true }
//   meson-builder = "0.1"
//   [meson]
//   build   = "glib"
//   defines = ["default_library=static"]

let names = if type_of(CFG.build) == "string" { [CFG.build] } else { CFG.build };

for name in names {
    if !(name in PKGS) { throw "[meson] build: '" + name + "' is not a dependency"; }
    let src = PKGS[name].dir;
    if !is_dir(src) { throw "'" + name + "' is not fetched yet: " + src; }

    let build  = join(OUT_DIR, name);
    let prefix = join(build, "install");

    // ── Configure (meson setup) ──────────────────────────────────────────────
    let setup = ["setup", build, src,
                 "--prefix=" + prefix,
                 "--libdir=lib",          // force prefix/lib (distros vary: lib/<triple>)
                 "--buildtype=" + (if PROFILE == "release" { "release" } else { "debug" })];
    if is_dir(join(build, "meson-info")) { setup.push("--reconfigure"); }
    if "defines" in CFG { for d in CFG.defines { setup.push("-D" + d); } }
    run("meson", setup);

    // ── Build + install ──────────────────────────────────────────────────────
    run("meson", ["compile", "-C", build]);     // drives ninja
    run("meson", ["install", "-C", build]);

    // ── Wire results back into the freight build ───────────────────────────────
    add_include_dir(join(prefix, "include"));
    for d in [join(prefix, "lib"), join(prefix, "lib64")] {
        if is_dir(d) {
            link_dir(d);
            for lib in glob(d + "/*.a") + glob(d + "/*.so") { link_lib(lib); }
        }
    }
}
