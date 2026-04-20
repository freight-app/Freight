use std::fs;
use std::path::Path;

use crate::error::CraneError;
use crate::output::print_success;

const SUPPORTED_LANGS: &[(&str, &str, &str)] = &[
    ("c",       "c",       "c17"),
    ("c++",     "c++",     "c++20"),
    ("cpp",     "c++",     "c++20"),
    ("fortran", "fortran", "f2018"),
    ("ada",     "ada",     "ada2012"),
    ("d",       "d",       ""),
];

pub fn scaffold_project(name: &str, lang_arg: &str) -> Result<(), CraneError> {
    let (lang_name, lang_std) = resolve_lang(lang_arg)?;

    let root = Path::new(name);
    if root.exists() {
        return Err(CraneError::ProjectExists(name.to_string()));
    }

    fs::create_dir_all(root.join("src"))?;

    write_manifest(root, name, lang_name, lang_std)?;
    write_hello(root, lang_name)?;
    write_gitignore(root)?;

    print_success(&format!("created `{name}` ({lang_name} project)"));
    println!();
    println!("  cd {name}");
    println!("  crane build");
    println!();

    Ok(())
}

fn resolve_lang(arg: &str) -> Result<(&'static str, &'static str), CraneError> {
    let lower = arg.to_lowercase();
    for (alias, name, std) in SUPPORTED_LANGS {
        if *alias == lower {
            return Ok((name, std));
        }
    }
    Err(CraneError::UnsupportedLanguage(arg.to_string()))
}

fn write_manifest(root: &Path, name: &str, lang: &str, std: &str) -> Result<(), CraneError> {
    let std_line = if std.is_empty() {
        String::new()
    } else {
        format!("std  = \"{std}\"\n")
    };

    let contents = format!(
        r#"[package]
name        = "{name}"
version     = "0.1.0"
description = ""
license     = "MIT"

[language]
name = "{lang}"
{std_line}
[[bin]]
name = "{name}"
src  = "src/main.{ext}"

[compiler]
backend   = "auto"
opt-level = 2
debug     = false
warnings  = "all"

[profile.dev]
opt-level = 0
debug     = true

[profile.release]
opt-level = 3
lto       = true
strip     = true
debug     = false
"#,
        name = name,
        lang = lang,
        std_line = std_line,
        ext = lang_extension(lang),
    );

    fs::write(root.join("crane.toml"), contents)?;
    Ok(())
}

fn write_hello(root: &Path, lang: &str) -> Result<(), CraneError> {
    let (filename, contents) = hello_world_src(lang);
    fs::write(root.join("src").join(filename), contents)?;
    Ok(())
}

fn write_gitignore(root: &Path) -> Result<(), CraneError> {
    fs::write(root.join(".gitignore"), "/target\n")?;
    Ok(())
}

fn lang_extension(lang: &str) -> &'static str {
    match lang {
        "c++"     => "cpp",
        "c"       => "c",
        "fortran" => "f90",
        "ada"     => "adb",
        "d"       => "d",
        _         => "cpp",
    }
}

fn hello_world_src(lang: &str) -> (&'static str, &'static str) {
    match lang {
        "c++" => ("main.cpp", "#include <iostream>\n\nint main() {\n    std::cout << \"Hello, world!\\n\";\n    return 0;\n}\n"),
        "c"   => ("main.c",   "#include <stdio.h>\n\nint main(void) {\n    printf(\"Hello, world!\\n\");\n    return 0;\n}\n"),
        "fortran" => ("main.f90", "program main\n    implicit none\n    print *, \"Hello, world!\"\nend program main\n"),
        "ada" => ("main.adb", "with Ada.Text_IO; use Ada.Text_IO;\nprocedure Main is\nbegin\n   Put_Line (\"Hello, world!\");\nend Main;\n"),
        "d"   => ("main.d",   "import std.stdio;\nvoid main() {\n    writeln(\"Hello, world!\");\n}\n"),
        _     => ("main.cpp", "#include <iostream>\n\nint main() {\n    std::cout << \"Hello, world!\\n\";\n    return 0;\n}\n"),
    }
}
