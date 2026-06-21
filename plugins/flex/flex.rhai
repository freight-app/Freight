// Flex lexer-generator plugin.
//
//   [dependencies]
//   flex = { path = "…/examples/plugins/flex" }
//   [flex]
//
// Runs `flex` on every `.l` lexer, compiling the generated `<stem>.yy.c`.

for f in glob("**/*.l") {
    run("flex", ["-o", OUT_DIR + "/" + stem(f) + ".yy.c", f]);
}

add_include_dir(OUT_DIR);
for g in glob(OUT_DIR + "/*.yy.c") {
    add_source(g);
}
