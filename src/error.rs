use thiserror::Error;

#[derive(Debug, Error)]
pub enum FreightError {
    #[error("project directory '{0}' already exists")]
    ProjectExists(String),

    #[error("unsupported language '{0}'")]
    UnsupportedLanguage(String),

    #[error("no freight.toml found in '{0}' or any parent directory")]
    ManifestNotFound(String),

    #[error("freight.toml parse error: {0}")]
    ManifestParse(String),

    #[error("cycle detected in module dependency graph: {0}")]
    DependencyCycle(String),

    #[error("slot conflict — '{0}' and '{1}' both provide '{2}'\n       only one provider per slot may be active")]
    SlotConflict(String, String, String),

    #[error("no compiler found for language '{0}' — is the toolchain installed?")]
    NoCompilerForLang(String),

    #[error("compiler not found: {0}")]
    CompilerNotFound(String),

    #[error("{0}")]
    OptionError(String),

    #[error("undeclared include(s) — blocked by [lints].undeclared-include = \"deny\":\n{0}")]
    UndeclaredInclude(String),

    #[error("compilation failed: {0}\n{1}")]
    CompileFailed(String, String),

    #[error("compiler template error: {0}")]
    TemplateError(String),

    #[error("no build system detected in '{0}' — pass --from cmake|makefile|meson")]
    ImporterNoFormat(String),

    #[error("unknown migration format '{0}' — expected cmake, makefile, or meson")]
    ImporterUnknownFormat(String),

    #[error("freight.toml already exists in '{0}' — use --force to overwrite")]
    ImporterManifestExists(String),

    #[error("importer error: {0}")]
    ImporterParse(String),

    #[error("build script '{0}' failed:\n{1}")]
    BuildScriptFailed(String, String),

    #[error("git error: {0}")]
    GitError(String),

    #[error("install failed: {0}")]
    InstallFailed(String),

    #[error("registry error: {0}")]
    RegistryError(String),

    #[error("package not found in registry: {0}")]
    RegistryNotFound(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
