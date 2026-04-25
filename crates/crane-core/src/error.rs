use thiserror::Error;

#[derive(Debug, Error)]
pub enum CraneError {
    #[error("project directory '{0}' already exists")]
    ProjectExists(String),

    #[error("unsupported language '{0}'")]
    UnsupportedLanguage(String),

    #[error("no crane.toml found in '{0}' or any parent directory")]
    ManifestNotFound(String),

    #[error("crane.toml parse error: {0}")]
    ManifestParse(String),

    #[error("cycle detected in module dependency graph: {0}")]
    DependencyCycle(String),

    #[error("no compiler found for language '{0}' — is the toolchain installed?")]
    NoCompilerForLang(String),

    #[error("compiler not found: {0}")]
    CompilerNotFound(String),

    #[error("compilation failed: {0}\n{1}")]
    CompileFailed(String, String),

    #[error("compiler template error: {0}")]
    TemplateError(String),

    #[error("no build system detected in '{0}' — pass --from cmake|makefile|meson")]
    ImporterNoFormat(String),

    #[error("unknown migration format '{0}' — expected cmake, makefile, or meson")]
    ImporterUnknownFormat(String),

    #[error("crane.toml already exists in '{0}' — use --force to overwrite")]
    ImporterManifestExists(String),

    #[error("importer error: {0}")]
    ImporterParse(String),

    #[error("git error: {0}")]
    GitError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
