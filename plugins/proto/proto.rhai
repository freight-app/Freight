// Protobuf codegen plugin.
//
// Consume it with:
//   [dependencies]
//   proto = { path = "…/examples/plugins/proto" }   # or a registry version
//   [proto]
//   # proto_path = "src"   # import root (default: src/)
//
// Runs `protoc --cpp_out` on every `.proto` under src/, compiles the generated
// `.pb.cc`, and exposes the generated headers.

let proto_path = if "proto_path" in CFG { PROJECT_DIR + "/" + CFG.proto_path } else { SRC_DIR };

for f in glob("src/**/*.proto") {
    run("protoc", ["--proto_path=" + proto_path, "--cpp_out=" + OUT_DIR, f]);
}

add_include_dir(OUT_DIR);
for g in glob(OUT_DIR + "/**/*.pb.cc") {
    add_source(g);
}
