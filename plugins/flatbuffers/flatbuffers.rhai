// FlatBuffers codegen plugin.
//
//   [dependencies]
//   flatbuffers = { path = "…/examples/plugins/flatbuffers" }
//   [flatbuffers]
//
// flatc's C++ output is header-only (`*_generated.h`), so this plugin only
// exposes an include directory — there are no generated sources to compile.

for f in glob("**/*.fbs") {
    run("flatc", ["--cpp", "-o", OUT_DIR, f]);
}

add_include_dir(OUT_DIR);
