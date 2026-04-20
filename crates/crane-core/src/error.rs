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

    #[error("compiler not found: {0}")]
    CompilerNotFound(String),

    #[error("compiler template error: {0}")]
    TemplateError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
