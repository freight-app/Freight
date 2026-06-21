// Build a dependency with Bazel and wire its headers + libraries in. Bazel
// writes artifacts under `bazel-bin/` (a symlink in the workspace after a build).
//
//   [dependencies]
//   libfoo        = { path = "vendor/libfoo", external = true }
//   bazel-builder = "0.1"
//   [bazel]
//   build = "libfoo"

let names = if type_of(CFG.build) == "string" { [CFG.build] } else { CFG.build };

for name in names {
    if !(name in PKGS) { throw "[bazel] build: '" + name + "' is not a dependency"; }
    let src = PKGS[name].dir;
    if !is_dir(src) { throw "'" + name + "' is not fetched yet: " + src; }

    let target = if "target" in CFG { CFG.target } else { "//..." };
    run("bazel", ["build", target], src);          // cwd = the dep workspace

    let inc = if "include" in CFG { join(src, CFG.include) } else { src };
    add_include_dir(inc);
    for lib in glob(src + "/bazel-bin/**/*.a") + glob(src + "/bazel-bin/**/*.so") {
        link_lib(lib);
    }
}
