//! End-to-end test for build plugins: a path-dependency plugin runs a Rhai
//! script that generates a C source (via an allow-listed tool), which the
//! consumer then compiles, links, and runs.

mod common;

use std::fs;
use std::path::Path;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

#[test]
fn plugin_generates_and_links_a_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // ── The plugin package ───────────────────────────────────────────────────
    let plugin = root.join("genplugin");
    write(
        &plugin.join("freight.toml"),
        "[package]\nname = \"genplugin\"\nversion = \"0.1.0\"\n\n\
         [plugin]\nentry = \"gen.freight\"\nhandles = [\"codegen\"]\ntools = [\"cp\"]\n",
    );
    // Template the plugin copies into the consumer's build as a generated source.
    write(
        &plugin.join("template.c"),
        "int plugin_value(void) { return 42; }\n",
    );
    // The plugin script: copy the template into out_dir, register it, add a define.
    write(
        &plugin.join("gen.freight"),
        r#"run("cp", [CFG.template, OUT_DIR + "/generated.c"]);
           add_source(OUT_DIR + "/generated.c");
           define("HAS_PLUGIN");"#,
    );

    // ── The consumer app ─────────────────────────────────────────────────────
    let app = root.join("app");
    write(
        &app.join("freight.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
         [[bin]]\nname = \"app\"\nsrc = \"src/main.c\"\n\n\
         [dependencies]\ngenplugin = { path = \"../genplugin\" }\n\n\
         [codegen]\ntemplate = \"../genplugin/template.c\"\n",
    );
    write(
        &app.join("src/main.c"),
        "#include <stdio.h>\n\
         int plugin_value(void);\n\
         int main(void) {\n\
         #ifdef HAS_PLUGIN\n\
             printf(\"%d\\n\", plugin_value());\n\
         #else\n\
             printf(\"no plugin\\n\");\n\
         #endif\n\
             return 0;\n\
         }\n",
    );

    let out = common::freight(&app, &["run"]);
    if common::missing_toolchain(&out) {
        eprintln!("skipping plugin_codegen: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with codegen plugin");
    // The plugin's define + generated function both took effect.
    common::assert_output_contains(&out, &["42"]);
    common::assert_output_missing(&out, "no plugin");
}
