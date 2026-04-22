//! Hover documentation for `crane.toml` fields.

/// Return Markdown documentation for a dotted path into `crane.toml`,
/// e.g. `"package.name"` or `"compiler.backend"`. Unknown paths return `None`.
pub fn lookup(path: &str) -> Option<&'static str> {
    match path {
        // Package
        "package" => Some("Top-level metadata. `name` and `version` are required."),
        "package.name" => Some("Package name. Letters, digits, `-`, `_`. Used as the default binary / lib name."),
        "package.version" => Some("Semver version string. `crane publish` refuses non-semver."),
        "package.authors" => Some("List of authors. Free-form; `Name <email>` is conventional."),
        "package.description" => Some("One-line summary shown on `crane.dev`."),
        "package.license" => Some("SPDX license identifier, e.g. `MIT`, `Apache-2.0`."),
        "package.repository" => Some("URL of the source repository."),

        // Language
        "language" => Some(
            "Per-language settings, keyed by the language identifier from a compiler template \
             (e.g. `cpp`, `c`, `fortran`, `ada`, `d`, `cuda`).",
        ),
        "language.std" => Some("Language standard, e.g. `c++20`, `c17`, `c23`. Must match the template's `[standards]` table."),

        // Lib / bin
        "lib" => Some("Library target. `type = \"static\" | \"shared\" | \"header-only\"`."),
        "lib.type" => Some("`static` → `.a`, `shared` → `.so`, `header-only` → no compile output."),
        "lib.src" => Some("Root source directory for the library."),
        "lib.include" => Some("Public header directory exposed to dependents."),
        "bin" => Some("Executable target. Repeat `[[bin]]` for multiple binaries."),
        "bin.name" => Some("Output binary name."),
        "bin.src" => Some("Path to the entry-point source file containing `main`."),

        // Dependencies
        "dependencies" => Some(
            "Runtime dependencies. Each entry is one of:\n\n\
             - **Version**: `foo = \"0.3\"` — fetched from crane.dev (not yet implemented)\n\
             - **Path**: `foo = { path = \"../foo\" }` — sibling crane project, built + linked\n\
             - **System**: `foo = { system = \"openssl\" }` — `-l{name}` at link time\n\
             - **Git**: `foo = { git = \"...\" }` — not yet implemented",
        ),
        "dev-dependencies" => Some("Dependencies used only by `crane test`."),

        // Compiler
        "compiler" => Some("Toolchain selection + default flags."),
        "compiler.backend" => Some(
            "Compiler backend: `auto` (default) picks the first available template per language. \
             Otherwise a template name like `gcc`, `clang`, `gfortran`, `nvcc`.",
        ),
        "compiler.opt-level" => Some("Optimization level `0`–`3`. Mapped via the template's `flags.opt.N`."),
        "compiler.debug" => Some("Emit debug info. Mapped via the template's `flags.debug.true`."),
        "compiler.warnings" => Some("One of `none`, `default`, `all`, `error`. Mapped via `flags.warnings.<value>`."),
        "compiler.defines" => Some("Preprocessor defines. `[\"FOO\", \"BAR=1\"]` → `-DFOO -DBAR=1`."),
        "compiler.flags" => Some("Extra flags passed through verbatim."),
        "compiler.includes" => Some("Extra include directories beyond `src/` and `include/`."),
        "compiler.includes.paths" => Some("List of include directories. Relative to the project root."),
        "compiler.target" => Some("Cross-compilation target triple (e.g. `aarch64-linux-gnu`). Reserved for the cross-compile phase."),
        "compiler.sysroot" => Some("Path to target sysroot. Reserved for the cross-compile phase."),

        // Profiles
        "profile" => Some("Build profile overrides. `crane build` uses `dev`, `--release` uses `release`."),
        "profile.dev" => Some("Default profile (fast compile, debug info)."),
        "profile.release" => Some("Release profile (`--release`; optimized, no debug by default)."),
        "profile.opt-level" => Some("Overrides `[compiler] opt-level` for this profile."),
        "profile.debug" => Some("Overrides `[compiler] debug` for this profile."),
        "profile.lto" => Some("Enable link-time optimization."),
        "profile.strip" => Some("Strip symbols at link time."),
        "profile.sanitize" => Some("Sanitizers: `[\"address\", \"undefined\", \"thread\", \"memory\"]`."),

        // Target (phase 6)
        "target" => Some("Architecture + CPU extension selection."),
        "target.arch" => Some("Target architecture name, resolved through the template's `[arch_flags]` table."),
        "target.cpu_extensions" => Some("CPU feature names (e.g. `[\"avx2\", \"fma\"]`) rendered by the template's `cpu_extension` pattern."),

        _ => None,
    }
}
