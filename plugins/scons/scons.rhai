// Build a dependency with SCons (in-tree) and wire its headers + libraries in.
//
//   [dependencies]
//   libfoo        = { path = "vendor/libfoo", external = true }
//   scons-builder = "0.1"
//   [scons]
//   build = "libfoo"

let names = if type_of(CFG.build) == "string" { [CFG.build] } else { CFG.build };

for name in names {
    if !(name in PKGS) { throw "[scons] build: '" + name + "' is not a dependency"; }
    let src = PKGS[name].dir;
    if !is_dir(src) { throw "'" + name + "' is not fetched yet: " + src; }

    let args = [];
    if "defines" in CFG { for d in CFG.defines { args.push(d); } }
    if "target" in CFG { args.push(CFG.target); }
    run("scons", args, src);                       // cwd = the dep source

    let inc = if "include" in CFG {
        join(src, CFG.include)
    } else if is_dir(join(src, "include")) {
        join(src, "include")
    } else {
        src
    };
    add_include_dir(inc);
    for lib in glob(src + "/**/*.a") + glob(src + "/**/*.so") { link_lib(lib); }
}
