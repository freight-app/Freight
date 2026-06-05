/// Integration test: flat `.pkgs/` pool for transitive deps.
///
/// Scenario: root → vecmath → mathlib (all fetched as version/registry deps).
/// Without the fix, mathlib would be built at `.pkgs/vecmath/.pkgs/mathlib/`.
/// With the fix, mathlib is built at the root's `.pkgs/mathlib/`.
mod common;
use common::*;
use std::path::Path;

/// Create a minimal C static library project under `dir` with a given name
/// and optional version-dep on another library already in the flat pool.
fn write_lib_project(dir: &Path, name: &str, dep: Option<(&str, &str)>) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("include").join(name)).unwrap();

    // Header — does NOT expose the dep in public API to avoid transitive includes.
    let hdr = format!("#pragma once\nint {name}_value(void);\n");
    std::fs::write(
        dir.join("include").join(name).join(format!("{name}.h")),
        hdr,
    )
    .unwrap();

    // Source — includes dep's header for compilation (tests include-path threading)
    // but does not call dep functions (avoids transitive link-dep propagation issue).
    let src = if let Some((dep_name, _)) = dep {
        format!(
            "#include <{dep_name}/{dep_name}.h>\n\
             /* dep header included to verify include path; not called at link time */\n\
             int {name}_value(void) {{ return 10; }}\n"
        )
    } else {
        format!("int {name}_value(void) {{ return 42; }}\n")
    };
    std::fs::write(dir.join("src").join(format!("{name}.c")), src).unwrap();

    // freight.toml
    let dep_section = if let Some((dep_name, dep_ver)) = dep {
        format!("\n[dependencies]\n{dep_name} = \"{dep_ver}\"\n")
    } else {
        String::new()
    };
    let toml = format!(
        r#"[package]
name    = "{name}"
version = "0.1.0"

[language.c]
std = "c11"

[compiler]
includes = ["include"]

[lib]
type = "static"
srcs = ["src/{name}.c"]
hdrs = ["include/{name}/{name}.h"]
{dep_section}
"#
    );
    std::fs::write(dir.join("freight.toml"), toml).unwrap();
    // Sentinel written by `freight fetch` to mark a dep as available.
    std::fs::write(dir.join(".freight-fetched"), "").unwrap();
}

/// Write the root binary project that depends on vecmath.
fn write_root_project(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();

    std::fs::write(
        dir.join("src").join("main.c"),
        r#"#include <vecmath/vecmath.h>
#include <stdio.h>
int main(void) {
    printf("value=%d\n", vecmath_value());
    return 0;
}
"#,
    )
    .unwrap();

    std::fs::write(
        dir.join("freight.toml"),
        r#"[package]
name    = "app"
version = "0.1.0"

[language.c]
std = "c11"

[[bin]]
name = "app"
src  = "src/main.c"

[dependencies]
vecmath = "0.1.0"
"#,
    )
    .unwrap();
}

#[test]
fn flat_pkgs_transitive_dep_at_root_level() {
    let root = tempfile::tempdir().unwrap();
    let root_dir = root.path();

    // Root project: app → vecmath (version dep, in .pkgs/).
    write_root_project(root_dir);

    // Pre-populate .pkgs/vecmath/ (simulates `freight fetch` having run).
    // vecmath itself depends on mathlib via a version dep.
    let vecmath_dir = root_dir.join(".pkgs/vecmath");
    write_lib_project(&vecmath_dir, "vecmath", Some(("mathlib", "0.1.0")));

    // Pre-populate root's .pkgs/mathlib/ — mathlib is in the flat pool, NOT
    // inside .pkgs/vecmath/.pkgs/mathlib/.
    let mathlib_dir = root_dir.join(".pkgs/mathlib");
    write_lib_project(&mathlib_dir, "mathlib", None);

    let out = freight(root_dir, &["build"]);
    assert_success(&out, "flat_pkgs: app build");

    // mathlib was built at the root flat pool (profile = version constraint "0.1.0").
    let mathlib_built = mathlib_dir.join("target/0.1.0/libmathlib.a");
    assert!(
        mathlib_built.exists(),
        "mathlib should be built in root .pkgs/mathlib/target/0.1.0/, not nested"
    );

    // No nested .pkgs/ should have been created inside vecmath.
    let nested = vecmath_dir.join(".pkgs");
    assert!(
        !nested.exists(),
        "vecmath/.pkgs/ should not exist — transitive deps must use the flat root pool"
    );
}

#[test]
fn flat_pkgs_two_deps_share_transitive() {
    // root → vecmath → mathlib
    //      → geometry → mathlib
    // mathlib should only be built once, from root's .pkgs/mathlib/.
    let root = tempfile::tempdir().unwrap();
    let root_dir = root.path();

    std::fs::create_dir_all(root_dir.join("src")).unwrap();
    std::fs::write(
        root_dir.join("src/main.c"),
        "#include <vecmath/vecmath.h>\n#include <stdio.h>\nint main(void){printf(\"%d\\n\",vecmath_value());return 0;}\n",
    ).unwrap();
    std::fs::write(
        root_dir.join("freight.toml"),
        r#"[package]
name    = "app"
version = "0.1.0"

[language.c]
std = "c11"

[[bin]]
name = "app"
src  = "src/main.c"

[dependencies]
vecmath  = "0.1.0"
geometry = "0.1.0"
"#,
    ).unwrap();

    let vecmath_dir = root_dir.join(".pkgs/vecmath");
    write_lib_project(&vecmath_dir, "vecmath", Some(("mathlib", "0.1.0")));

    let geometry_dir = root_dir.join(".pkgs/geometry");
    write_lib_project(&geometry_dir, "geometry", Some(("mathlib", "0.1.0")));

    let mathlib_dir = root_dir.join(".pkgs/mathlib");
    write_lib_project(&mathlib_dir, "mathlib", None);

    let out = freight(root_dir, &["build"]);
    assert_success(&out, "flat_pkgs: two-dep shared transitive");

    assert!(
        mathlib_dir.join("target/0.1.0/libmathlib.a").exists(),
        "mathlib must be built from the flat root pool (target/0.1.0/)"
    );
    assert!(
        !vecmath_dir.join(".pkgs").exists(),
        "vecmath must not create its own .pkgs/"
    );
    assert!(
        !geometry_dir.join(".pkgs").exists(),
        "geometry must not create its own .pkgs/"
    );
}
