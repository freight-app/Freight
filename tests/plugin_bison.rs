//! Exercises the shipped `plugins/bison` reference plugin end-to-end:
//! a project with a `.y` grammar and `[bison]` gets the generated parser
//! compiled and linked automatically.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn bison_available() -> bool {
    Command::new("bison")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn bison_plugin_generates_and_links_a_parser() {
    if !bison_available() {
        eprintln!("skipping bison plugin test: bison not installed");
        return;
    }

    let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins/bison");

    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("calc");
    write(
        &app.join("freight.toml"),
        &format!(
            "[package]\nname = \"calc\"\nversion = \"0.1.0\"\n\n\
             [[bin]]\nname = \"calc\"\nsrc = \"src/main.c\"\n\n\
             [dependencies]\nbison = {{ path = \"{}\" }}\n\n\
             [bison]\n",
            plugin.display()
        ),
    );
    // A minimal self-contained grammar: provides yylex/yyerror and a wrapper.
    write(
        &app.join("src/grammar.y"),
        "%{\n#include <stdio.h>\nint yylex(void);\nvoid yyerror(const char *s);\n%}\n\
         %%\ninput: /* empty */ ;\n%%\n\
         int yylex(void) { return 0; }\n\
         void yyerror(const char *s) { (void)s; }\n\
         int run_parser(void) { return yyparse(); }\n",
    );
    write(
        &app.join("src/main.c"),
        "#include <stdio.h>\nint run_parser(void);\n\
         int main(void) { printf(\"parse=%d\\n\", run_parser()); return 0; }\n",
    );

    let out = common::freight(&app, &["run"]);
    if common::missing_toolchain(&out) {
        eprintln!("skipping bison plugin test: no C toolchain");
        return;
    }
    common::assert_success(&out, "freight run with bison plugin");
    common::assert_output_contains(&out, &["parse=0"]);
}
