// Build dependencies with Autotools (./configure && make && make install) and
// wire their headers + libraries into the build. Uses an out-of-source (VPATH)
// build under OUT_DIR, so the dependency's own tree stays clean.
//
//   [dependencies]
//   libfoo            = { url = "…/libfoo-1.2.tar.gz", external = true }
//   autotools-builder = "0.1"
//   [autotools]
//   build   = "libfoo"
//   defines = ["--enable-shared=no"]

let names = if type_of(CFG.build) == "string" { [CFG.build] } else { CFG.build };

for name in names {
    if !(name in PKGS) { throw "[autotools] build: '" + name + "' is not a dependency"; }
    let src = PKGS[name].dir;
    if !is_dir(src) { throw "'" + name + "' is not fetched yet: " + src; }
    if !is_file(join(src, "configure")) {
        throw "'" + name + "' has no ./configure script at " + src;
    }

    let build  = join(OUT_DIR, name);
    let prefix = join(build, "install");
    makedirs(build);

    // ── Configure (VPATH build from `build`, referencing the srcdir) ──────────
    let conf = [join(src, "configure"), "--prefix=" + prefix];
    if "defines" in CFG { for d in CFG.defines { conf.push(d); } }
    run("sh", conf, build);                       // cwd = build (out-of-source)

    // ── Build + install ──────────────────────────────────────────────────────
    run("make", ["-j"], build);
    run("make", ["install"], build);

    // ── Wire results back into the freight build ───────────────────────────────
    add_include_dir(join(prefix, "include"));
    for d in [join(prefix, "lib"), join(prefix, "lib64")] {
        if is_dir(d) {
            link_dir(d);
            for lib in glob(d + "/*.a") + glob(d + "/*.so") { link_lib(lib); }
        }
    }
}
