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
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crane_core::error::CraneError;

pub mod cmake;
pub mod detect;
pub mod emit;
pub mod makefile;
pub mod meson;

// ── Public entry point ────────────────────────────────────────────────────────

/// Result of a successful migrate. Caller (CLI) decides what to display.
pub struct MigrateOutcome {
    pub format: Format,
    pub project_dir: PathBuf,
    /// Where the manifest was written, or `None` for `dry_run`.
    pub written_to: Option<PathBuf>,
    /// Generated `crane.toml` contents — useful for `--dry-run` output.
    pub toml: String,
    /// Number of `# CRANE:` notes the user should review.
    pub note_count: usize,
}

/// Run `crane migrate` against `project_dir`.
///
/// * `format` — when `None`, the importer auto-detects from files present.
/// * `dry_run` — generates the manifest but does not write to disk.
/// * `force` — overwrite an existing `crane.toml`.
///
/// Pure: returns the outcome instead of printing. The CLI shell formats a
/// human-readable summary; library users can inspect [`MigrateOutcome`].
pub fn run_migrate(
    project_dir: &Path,
    format: Option<Format>,
    dry_run: bool,
    force: bool,
) -> Result<MigrateOutcome, CraneError> {
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

    let imported = match fmt {
        Format::Cmake => cmake::parse(project_dir)?,
        Format::Makefile => makefile::parse(project_dir)?,
        Format::Meson => meson::parse(project_dir)?,
    };

    let toml = emit::to_toml(&imported);
    let note_count = imported.notes.len();

    let written_to = if dry_run {
        None
    } else {
        fs::write(&manifest_path, &toml)?;
        Some(manifest_path)
    };

    Ok(MigrateOutcome {
        format: fmt,
        project_dir: project_dir.to_path_buf(),
        written_to,
        toml,
        note_count,
    })
}

/// Parse a `--from` CLI flag into a [`Format`]. Convenience for the binary.
pub fn parse_format(s: &str) -> Result<Format, CraneError> {
    Format::from_str(s)
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
    /// All libraries declared in the source build file, in declaration order.
    /// The first is emitted as `[lib]`; extras generate a workspace note.
    pub libs: Vec<ImportedLib>,
    pub bins: Vec<ImportedBin>,
    /// Keyed by dep name.
    pub dependencies: BTreeMap<String, ImportedDep>,
    pub compiler: ImportedCompiler,
    /// Per-platform overlays keyed by crane platform name (`linux`, `windows`,
    /// `macos`, `unix`, …). Populated when the source build system gates calls
    /// behind a platform check (e.g. CMake's `if(WIN32) … endif()`); emitted as
    /// `[platform.X]` sections in the generated `crane.toml`.
    pub platforms: BTreeMap<String, ImportedPlatformOverlay>,
    /// Free-form notes emitted as `# CRANE: …` comments at the top of the
    /// generated manifest so the user can review constructs that didn't map
    /// cleanly.
    pub notes: Vec<String>,
}

/// Per-platform fragment of an [`ImportedProject`]. Only carries the fields a
/// platform overlay can actually override — mirrors `manifest::PlatformOverlay`.
#[derive(Debug, Default, Clone)]
pub struct ImportedPlatformOverlay {
    pub dependencies: BTreeMap<String, ImportedDep>,
    pub defines: Vec<String>,
    pub flags: Vec<String>,
    pub include_paths: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ImportedLanguage {
    /// Language standard, e.g. `"c++20"` or `"c17"`.
    pub std: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportedLib {
    /// Library target name from the source build system (e.g. `"mylib"`).
    pub name: String,
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

    /// Ensure a `[platform.<key>]` overlay exists; return a mutable handle.
    pub fn platform_mut(&mut self, key: &str) -> &mut ImportedPlatformOverlay {
        self.platforms.entry(key.to_string()).or_default()
    }

    /// Insert a dependency, routing it to a platform overlay when `platform`
    /// is set (e.g. inside `if(WIN32)`), otherwise to the base manifest.
    /// Existing entries are preserved (importer never silently overrides).
    pub fn add_dep(&mut self, platform: Option<&str>, name: String, dep: ImportedDep) {
        let bucket = match platform {
            Some(p) => &mut self.platform_mut(p).dependencies,
            None => &mut self.dependencies,
        };
        bucket.entry(name).or_insert(dep);
    }

    pub fn add_define(&mut self, platform: Option<&str>, define: String) {
        let bucket = match platform {
            Some(p) => &mut self.platform_mut(p).defines,
            None => &mut self.compiler.defines,
        };
        if !bucket.iter().any(|d| d == &define) {
            bucket.push(define);
        }
    }

    pub fn add_flag(&mut self, platform: Option<&str>, flag: String) {
        let bucket = match platform {
            Some(p) => &mut self.platform_mut(p).flags,
            None => &mut self.compiler.flags,
        };
        if !bucket.iter().any(|f| f == &flag) {
            bucket.push(flag);
        }
    }

    pub fn add_include_path(&mut self, platform: Option<&str>, path: String) {
        let bucket = match platform {
            Some(p) => &mut self.platform_mut(p).include_paths,
            None => &mut self.compiler.include_paths,
        };
        if !bucket.iter().any(|p| p == &path) {
            bucket.push(path);
        }
    }
}
