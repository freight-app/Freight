use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CraneError;
use crate::output::print_success;

// (alias, canonical_name, toml_key, std)
const SUPPORTED_LANGS: &[(&str, &str, &str, &str)] = &[
    ("c",       "c",       "c",       "c17"),
    ("c++",     "c++",     "cpp",     "c++20"),
    ("cpp",     "c++",     "cpp",     "c++20"),
    ("fortran", "fortran", "fortran", "f2018"),
    ("ada",     "ada",     "ada",     "ada2012"),
    ("d",       "d",       "d",       ""),
    ("cuda",    "cuda",    "cuda",    "c++20"),
    ("opencl",  "opencl",  "opencl",  "CL3.0"),
    ("hip",     "hip",     "hip",     "c++20"),
    ("sycl",    "sycl",    "sycl",    "c++20"),
    ("ispc",    "ispc",    "ispc",    ""),
];

pub fn scaffold_project(name: &str, lang_arg: &str) -> Result<(), CraneError> {
    let (lang_name, lang_key, lang_std) = resolve_lang(lang_arg)?;

    let root = Path::new(name);
    if root.exists() {
        return Err(CraneError::ProjectExists(name.to_string()));
    }

    fs::create_dir_all(root.join("src"))?;

    write_manifest(root, name, lang_name, lang_key, lang_std)?;
    write_hello(root, lang_name)?;
    write_gitignore(root)?;

    print_success(&format!("created `{name}` ({lang_name} project)"));
    println!();
    println!("  cd {name}");
    println!("  crane build");
    println!();

    Ok(())
}

/// `crane init [--lang <lang>]` — initialize crane in the current directory.
pub fn init_project(lang_arg: Option<&str>) -> Result<(), CraneError> {
    let cwd = std::env::current_dir()?;

    if cwd.join("crane.toml").exists() {
        return Err(CraneError::ProjectExists(
            "crane.toml already exists in this directory".into(),
        ));
    }

    let name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let lang = lang_arg
        .map(str::to_string)
        .or_else(|| detect_language(&cwd))
        .unwrap_or_else(|| "c++".into());

    let (lang_name, lang_key, lang_std) = resolve_lang(&lang)?;

    if !cwd.join("src").exists() {
        fs::create_dir(cwd.join("src"))?;
    }

    write_manifest(&cwd, &name, lang_name, lang_key, lang_std)?;

    // Only scaffold a hello-world if src/ is empty
    let src_is_empty = fs::read_dir(cwd.join("src"))
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if src_is_empty {
        write_hello(&cwd, lang_name)?;
    }

    if !cwd.join(".gitignore").exists() {
        write_gitignore(&cwd)?;
    }

    print_success(&format!("initialized `{name}` ({lang_name} project)"));
    Ok(())
}

/// Guess the language from file extensions found in the project root and `src/`.
fn detect_language(dir: &Path) -> Option<String> {
    let mut dirs_to_scan: Vec<PathBuf> = vec![dir.to_path_buf()];
    if dir.join("src").is_dir() {
        dirs_to_scan.push(dir.join("src"));
    }

    for scan_dir in dirs_to_scan {
        let Ok(entries) = fs::read_dir(&scan_dir) else { continue };
        for entry in entries.flatten() {
            match entry.path().extension().and_then(|e| e.to_str()) {
                Some("cpp" | "cc" | "cxx") => return Some("c++".into()),
                Some("c") => return Some("c".into()),
                Some("f90" | "f95" | "f03") => return Some("fortran".into()),
                Some("adb" | "ads") => return Some("ada".into()),
                Some("d") => return Some("d".into()),
                _ => {}
            }
        }
    }
    None
}

fn resolve_lang(arg: &str) -> Result<(&'static str, &'static str, &'static str), CraneError> {
    let lower = arg.to_lowercase();
    for (alias, name, key, std) in SUPPORTED_LANGS {
        if *alias == lower {
            return Ok((name, key, std));
        }
    }
    Err(CraneError::UnsupportedLanguage(arg.to_string()))
}

fn write_manifest(root: &Path, name: &str, lang: &str, lang_key: &str, std: &str) -> Result<(), CraneError> {
    let std_line = if std.is_empty() {
        String::new()
    } else {
        format!("std = \"{std}\"\n")
    };

    let contents = format!(
        r#"[package]
name        = "{name}"
version     = "0.1.0"
description = ""
license     = "MIT"

[language.{lang_key}]
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
        lang_key = lang_key,
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
