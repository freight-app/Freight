//! Regression: an optional dependency activated by a *non-root* package's own
//! feature must still be resolved transitively, so its include dirs (and lib /
//! exported defines) reach a consumer of that package.
//!
//! Shape: app → mid → (optional) leaf, where `mid` activates `leaf` via its default
//! feature `with-leaf = ["dep:leaf"]`, and `mid`'s public header includes `leaf`'s
//! header. Before the fix the graph walk resolved transitive deps with an empty
//! activated-deps set, so `leaf` was dropped and the build fell back to system
//! headers / failed to find `<leaf.h>`.

use std::fs;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn have_cc() -> bool {
    ["cc", "gcc", "clang"].iter().any(|t| {
        Command::new(t)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

#[test]
fn optional_dep_behind_feature_resolves_transitively() {
    if !have_cc() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();

    // leaf: a plain static lib exposing include/leaf.h.
    let leaf = tmp.path().join("leaf");
    write(
        &leaf.join("freight.toml"),
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\n\n[lib]\ntype = \"static\"\n",
    );
    write(&leaf.join("include/leaf.h"), "int leaf_val(void);\n");
    write(&leaf.join("src/leaf.c"), "int leaf_val(void){return 5;}\n");

    // mid: static lib whose PUBLIC header pulls in <leaf.h>; leaf is OPTIONAL and
    // activated only by mid's default feature.
    let mid = tmp.path().join("mid");
    write(
        &mid.join("freight.toml"),
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\n\n\
         [lib]\ntype = \"static\"\n\n\
         [features]\ndefault = [\"with-leaf\"]\nwith-leaf = [\"dep:leaf\"]\n\n\
         [dependencies]\nleaf = { path = \"../leaf\", optional = true }\n",
    );
    write(
        &mid.join("include/mid.h"),
        "#include <leaf.h>\nstatic inline int mid_val(void){return leaf_val()+1;}\n",
    );
    write(
        &mid.join("src/mid.c"),
        "#include <mid.h>\nint mid_anchor(void){return mid_val();}\n",
    );

    // app: depends only on mid; its main pulls in mid.h (→ leaf.h transitively).
    let app = tmp.path().join("app");
    write(
        &app.join("freight.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
         [[bin]]\nname = \"app\"\nsrc = \"src/main.c\"\n\n\
         [dependencies]\nmid = { path = \"../mid\" }\n",
    );
    write(
        &app.join("src/main.c"),
        "#include <mid.h>\nint main(void){return mid_val()==6?0:1;}\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_freight"))
        .arg("build")
        .current_dir(&app)
        .output()
        .expect("run freight build");
    assert!(
        out.status.success(),
        "app should build: mid's optional `leaf` (behind its default feature) must \
         resolve transitively so <leaf.h> is on the include path.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
