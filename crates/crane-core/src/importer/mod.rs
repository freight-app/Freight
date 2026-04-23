//! Migrate an existing CMake / Makefile / Meson project into a `crane.toml`.
//!
//! Each importer parses its source format into the shared [`ImportedProject`]
//! intermediate representation, then [`emit::to_toml`] serialises it into the
//! crane manifest. Constructs the importer could not translate are preserved
//! as `# CRANE: …` comments at the top of the emitted file so the user can
//! review them.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use crate::error::CraneError;
use crate::output::{print_error, print_status, print_success, print_warning};

pub mod cmake;
pub mod detect;
pub mod emit;
pub mod makefile;
pub mod meson;

// ── CLI glue ──────────────────────────────────────────────────────────────────

/// Thin wrapper used by the `crane` binary — parses the `--from` string and
/// dispatches to [`run_migrate`], printing errors instead of propagating.
pub fn cmd_migrate(from: Option<&str>, dry_run: bool, force: bool) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };

    let fmt = match from {
        Some(s) => match Format::from_str(s) {
            Ok(f) => Some(f),
            Err(e) => {
                print_error(&e.to_string());
                return;
            }
        },
        None => None,
    };

    if let Err(e) = run_migrate(&cwd, fmt, dry_run, force) {
        print_error(&e.to_string());
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run `crane migrate` against `project_dir`.
///
/// * `format` — when `None`, the importer auto-detects from files present.
/// * `dry_run` — prints the generated `crane.toml` to stdout instead of writing.
/// * `force` — overwrite an existing `crane.toml`.
pub fn run_migrate(
    project_dir: &Path,
    format: Option<Format>,
    dry_run: bool,
    force: bool,
) -> Result<(), CraneError> {
    let fmt = match format {
        Some(f) => f,
        None => detect::detect_format(project_dir)
            .ok_or_else(|| CraneError::ImporterNoFormat(project_dir.display().to_string()))?,
    };

    let manifest_path = project_dir.join("crane.toml");
    if !dry_run && manifest_path.exists() && !force {
        return Err(CraneError::ImporterManifestExists(
            project_dir.display().to_string(),
        ));
    }

    print_status("Importing", &format!("{fmt} project at {}", project_dir.display()));

    let imported = match fmt {
        Format::Cmake => cmake::parse(project_dir)?,
        Format::Makefile => makefile::parse(project_dir)?,
        Format::Meson => meson::parse(project_dir)?,
    };

    let toml = emit::to_toml(&imported);

    if dry_run {
        print!("{toml}");
        return Ok(());
    }

    fs::write(&manifest_path, &toml)?;

    if !imported.notes.is_empty() {
        print_warning(&format!(
            "{} construct(s) could not be imported — see `# CRANE:` comments in crane.toml",
            imported.notes.len()
        ));
    }

    print_success(&format!("wrote {}", manifest_path.display()));
    Ok(())
}

// ── Format ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Cmake,
    Makefile,
    Meson,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Cmake => "cmake",
            Format::Makefile => "makefile",
            Format::Meson => "meson",
        })
    }
}

impl FromStr for Format {
    type Err = CraneError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "cmake" => Ok(Format::Cmake),
            "make" | "makefile" => Ok(Format::Makefile),
            "meson" => Ok(Format::Meson),
            other => Err(CraneError::ImporterUnknownFormat(other.to_string())),
        }
    }
}

// ── Intermediate representation ───────────────────────────────────────────────

/// The common shape the three importers produce. Fields are `Option` /
/// `Vec` / `BTreeMap` so that partial imports are still representable and the
/// emitted TOML only contains sections the source project actually declared.
#[derive(Debug, Default, Clone)]
pub struct ImportedProject {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// Keyed by crane language identifier: `"c"`, `"cpp"`, `"fortran"`, etc.
    pub languages: BTreeMap<String, ImportedLanguage>,
    pub lib: Option<ImportedLib>,
    pub bins: Vec<ImportedBin>,
    /// Keyed by dep name.
    pub dependencies: BTreeMap<String, ImportedDep>,
    pub compiler: ImportedCompiler,
    /// Free-form notes emitted as `# CRANE: …` comments at the top of the
    /// generated manifest so the user can review constructs that didn't map
    /// cleanly.
    pub notes: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ImportedLanguage {
    /// Language standard, e.g. `"c++20"` or `"c17"`.
    pub std: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportedLib {
    /// `"static"`, `"shared"`, or `"header-only"`.
    pub lib_type: String,
    /// Source directory (e.g. `"src/"`).
    pub src: String,
    pub include: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportedBin {
    pub name: String,
    /// Single entry-point source file — the rest of the project's sources are
    /// discovered by crane's build engine at compile time.
    pub src: String,
}

#[derive(Debug, Clone)]
pub enum ImportedDep {
    /// `{ system = "foo" }` — linked from the host OS (e.g. `-lfoo`).
    System(String),
    /// A bare version string — resolved against crane.dev once available.
    Version(String),
    /// `{ path = "../foo" }` — sibling crane project.
    Path(String),
}

#[derive(Debug, Default, Clone)]
pub struct ImportedCompiler {
    pub defines: Vec<String>,
    pub flags: Vec<String>,
    pub include_paths: Vec<String>,
}

impl ImportedProject {
    pub fn push_note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// Ensure a `[language.<key>]` entry exists; return a mutable handle.
    pub fn language_mut(&mut self, key: &str) -> &mut ImportedLanguage {
        self.languages.entry(key.to_string()).or_default()
    }
}
