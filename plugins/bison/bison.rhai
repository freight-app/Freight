// Bison parser-generator plugin.
//
//   [dependencies]
//   bison = { path = "…/examples/plugins/bison" }
//   [bison]
//
// Runs `bison -d` on every `.y` grammar, compiling the generated `<stem>.tab.c`
// and exposing the `<stem>.tab.h` header.

for f in glob("**/*.y") {
    run("bison", ["-d", "-o", OUT_DIR + "/" + stem(f) + ".tab.c", f]);
}

add_include_dir(OUT_DIR);
for g in glob(OUT_DIR + "/*.tab.c") {
    add_source(g);
}
