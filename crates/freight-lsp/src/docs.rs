//! Hover documentation for Freight manifests, build scripts, and compiler templates.

use crate::completion::DocumentKind;

/// Return Markdown documentation for the symbol/path under the cursor.
pub fn lookup(kind: DocumentKind, path: &str) -> Option<&'static str> {
    match kind {
        DocumentKind::Manifest => lookup_manifest(path),
        DocumentKind::BuildScript => lookup_build_script(path),
        DocumentKind::CompilerTemplate => lookup_compiler_template(path),
        DocumentKind::FortranSource => None,
    }
}

fn lookup_manifest(path: &str) -> Option<&'static str> {
    lookup_manifest_exact(path).or_else(|| lookup_manifest_exact(&normalize_manifest_path(path)))
}

fn lookup_manifest_exact(path: &str) -> Option<&'static str> {
    match path {
        // Package
        "package" => Some("Top-level metadata. `name` and `version` are required."),
        "package.name" => Some("Package name. Letters, digits, `-`, `_`. Used as the default binary / lib name."),
        "package.version" => Some("Semver version string. `freight publish` refuses non-semver."),
        "package.authors" => Some("List of authors. Free-form; `Name <email>` is conventional."),
        "package.description" => Some("One-line summary shown on `freight.dev`."),
        "package.license" => Some("SPDX license identifier, e.g. `MIT`, `Apache-2.0`."),
        "package.readme" => Some("Path to a README file for package documentation."),
        "package.repository" => Some("URL of the source repository."),
        "package.keywords" => Some("Search keywords for this package."),
        "package.provides" => Some("Virtual slots this package fills, used to detect conflicts such as multiple BLAS providers."),

        // Language
        "language" => Some(
            "Per-language settings, keyed by the language identifier from a compiler template \
             (e.g. `cpp`, `c`, `fortran`, `ada`, `d`, `cuda`, `asm`).",
        ),
        "language.std" => Some("Language standard, e.g. `c++20`, `c17`, `c23`. Must match the template's `standards` table."),
        "language.stdlib" => Some("C++ standard library selection: `libstdc++`, `libc++`, or `none`."),

        // Lib / bin
        "lib" => Some("Library target. `type = \"static\" | \"shared\" | \"header-only\"`."),
        "lib.type" => Some("`static` → `.a`, `shared` → `.so`, `header-only` → no compile output."),
        "lib.src" => Some("Root source directory for the library."),
        "lib.inc" | "lib.include" => Some("Public header directory exposed to dependents."),
        "bin" => Some("Executable target. Repeat `[[bin]]` for multiple binaries."),
        "bin.name" => Some("Output binary name."),
        "bin.src" => Some("Path to the entry-point source file containing `main`."),

        // Dependencies
        "dependencies" => Some(
            "Runtime dependencies. Entries can be version strings, local `path` deps, `system` libs, `git` deps, archives via `url`/`sha256`, foreign build-system deps, or `pkg_config` queries.",
        ),
        "dev-dependencies" => Some("Dependencies used only by `freight test`."),
        "dependencies.path" | "dev-dependencies.path" => Some("Relative path to another Freight project."),
        "dependencies.system" | "dev-dependencies.system" => Some("System library name passed through the active compiler template's system-library flag."),
        "dependencies.git" | "dev-dependencies.git" => Some("Git repository URL for this dependency."),
        "dependencies.branch" | "dependencies.tag" | "dependencies.rev" => Some("Git ref selector. `rev` pins an exact commit/ref; `branch` and `tag` are named refs."),
        "dependencies.pkg_config" => Some("pkg-config query string. Freight injects returned cflags/libs into compilation and linking."),
        "dependencies.url" => Some("Source archive URL fetched into `.deps/{name}/` and built by an auto-detected foreign build system."),
        "dependencies.sha256" => Some("Expected SHA-256 checksum for an archive dependency."),
        "dependencies.features" => Some("Dependency features to activate."),
        "dependencies.default-features" => Some("Whether to enable the dependency's default features."),
        "dependencies.optional" => Some("Marks the dependency as feature-gated/optional."),
        "dependencies.targets" => Some("Target triple allowlist for this dependency."),
        "dependencies.os" => Some("Host OS or family alias allowlist, e.g. `linux`, `macos`, `unix`, `bsd`."),
        "dependencies.arch" => Some("Host CPU architecture allowlist, e.g. `x86_64`, `aarch64`."),
        "dependencies.build_system" => Some("External build system: `cmake`, `make`, `meson`, or `auto`."),
        "dependencies.include" => Some("Include directories exposed by a foreign dependency."),
        "dependencies.cmake_args" => Some("Extra arguments forwarded to CMake configure."),

        // Compiler
        "compiler" => Some("Toolchain-independent default compiler settings."),
        "compiler.opt-level" => Some("Optimization level `0`–`3`. Mapped through the template's `flags[\"opt\"]` table."),
        "compiler.debug" => Some("Emit debug info. Mapped through the template's debug flags."),
        "compiler.warnings" => Some("One of `none`, `default`, `all`, `error`. Mapped through the template's warnings flags."),
        "compiler.defines" => Some("Preprocessor defines. `[\"FOO\", \"BAR=1\"]` → `-DFOO -DBAR=1` using template syntax."),
        "compiler.flags" => Some("Extra flags passed through verbatim."),
        "compiler.overrides" => Some("Language-to-template overrides, e.g. `cpp = \"clang\"`, when auto selection is not enough."),
        "compiler.pch" => Some("Header path to precompile and inject into matching language sources."),
        "compiler.includes" => Some("Extra include directories beyond target defaults."),
        "compiler.includes.paths" => Some("List of include directories. Relative to the project root."),

        // Features
        "features" => Some("Named feature sets. Values are lists of feature/dependency names activated together."),

        // Profiles
        "profile" => Some("Build profile overrides. `freight build` uses `dev`, `--release` uses `release`; custom profiles may inherit from either."),
        "profile.dev" => Some("Default profile (fast compile, debug info)."),
        "profile.release" => Some("Release profile (`--release`; optimized, no debug by default)."),
        "profile.inherits" => Some("Parent profile to merge before this profile's overrides."),
        "profile.opt-level" => Some("Overrides `[compiler] opt-level` for this profile."),
        "profile.debug" => Some("Overrides `[compiler] debug` for this profile."),
        "profile.lto" => Some("Enable link-time optimization."),
        "profile.strip" => Some("Strip symbols at link time."),
        "profile.sanitize" => Some("Sanitizers: `[\"address\", \"undefined\", \"thread\", \"memory\"]`, subject to template support."),
        "profile.features" => Some("Features enabled only in this profile."),

        // Target
        "target" => Some("Architecture + CPU extension selection."),
        "target.arch" => Some("Target architecture name, resolved through the template's `arch_flags` table."),
        "target.cpu_extensions" => Some("CPU feature names (e.g. `[\"avx2\", \"fma\"]`) rendered by the template's `flags[\"cpu_ext\"]` pattern."),

        // Platform overlays
        "platform" => Some("Per-host overlays keyed by OS name or family alias such as `linux`, `windows`, `unix`, or `bsd`."),
        "platform.dependencies" => Some("Dependencies added or overridden only when this platform overlay matches."),
        "platform.compiler" => Some("Compiler `defines`, `flags`, and `includes` added only when this platform overlay matches."),
        "platform.language" => Some("Per-language settings, such as `stdlib`, overridden only when this platform overlay matches."),

        _ => None,
    }
}

fn normalize_manifest_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('.').collect();
    match parts.as_slice() {
        ["language", _lang, field] => format!("language.{field}"),
        ["profile", _name, field] => format!("profile.{field}"),
        ["platform", _os] => "platform".to_string(),
        ["platform", _os, "dependencies"] => "platform.dependencies".to_string(),
        ["platform", _os, "compiler"] => "platform.compiler".to_string(),
        ["platform", _os, "compiler", field] => format!("compiler.{field}"),
        ["platform", _os, "language", _lang] => "platform.language".to_string(),
        ["platform", _os, "language", _lang, field] => format!("language.{field}"),
        ["dependencies", _name] => "dependencies".to_string(),
        ["dependencies", _name, field] => format!("dependencies.{field}"),
        ["dev-dependencies", _name] => "dev-dependencies".to_string(),
        ["dev-dependencies", _name, field] => format!("dev-dependencies.{field}"),
        _ => path.to_string(),
    }
}

fn lookup_build_script(symbol: &str) -> Option<&'static str> {
    match symbol {
        "package_name" => Some("Returns the current package name from `[package]`."),
        "package_version" => Some("Returns the current package version from `[package]`."),
        "profile" => Some("Returns the active build profile name."),
        "out_dir" => Some("Returns `target/{profile}/build`, which is automatically added to include paths."),
        "src_dir" => Some("Returns the project root directory."),
        "define" => Some("Adds a preprocessor define without a value."),
        "define_value" => Some("Adds a preprocessor define with an explicit value."),
        "add_include" => Some("Adds an include directory for subsequent compilation."),
        "add_flag" => Some("Adds a raw compiler flag."),
        "add_link_lib" => Some("Adds a system library to link, rendered through the active compiler template."),
        "add_link_flag" => Some("Adds a raw linker flag."),
        "link_path" => Some("Adds a library search directory as a link flag."),
        "add_source" => Some("Adds a generated source file to compile with the project."),
        "warning" => Some("Emits a non-fatal build-script warning."),
        "rerun_if" => Some("Adds a file dependency for build-script caching."),
        "write_file" => Some("Writes a file, creating parent directories and avoiding rewrites when content is unchanged."),
        "read_file" => Some("Reads a file into a string, or returns an empty string if it cannot be read."),
        "path_exists" => Some("Returns whether a path exists."),
        "mkdir" => Some("Creates a directory and its parents."),
        "pkg_config_cflags" => Some("Runs `pkg-config --cflags <name>` and returns the output."),
        "pkg_config_libs" => Some("Runs `pkg-config --libs <name>` and returns the output."),
        "pkg_config_apply" => Some("Queries pkg-config and applies include, compile, library, and link flags."),
        "find_tool" => Some("Finds a tool in `PATH`; returns unit when not found."),
        "pkg_config_exists" => Some("Returns whether `pkg-config --exists <name>` succeeds."),
        "run" => Some("Runs a command in the project directory and returns a map with `ok`, `status`, `stdout`, and `stderr`."),
        "fail" => Some("Aborts the build script with an error message."),
        "glob" => Some("Returns sorted paths matching a glob pattern relative to the project root."),
        "changed_files" => Some("Returns matching files newer than the previous build-script stamp; first build returns all matches."),
        "file_stem" => Some("Returns the file stem for a path."),
        "file_name" => Some("Returns the file name for a path."),
        "file_dir" => Some("Returns the parent directory for a path."),
        "env" => Some("Map-like environment access. Reading `env[\"VAR\"]` reads the host env; assigning sets overrides for commands and compiler invocations."),
        "toolchain" => Some("Read-only map with `backend`, `version`, `target`, `arch`, and `os`."),
        "packages" => Some("Map of resolved pkg-config dependencies. Each entry has `.found` and `.version`."),
        _ => None,
    }
}

fn lookup_compiler_template(symbol: &str) -> Option<&'static str> {
    match symbol {
        "name" => Some("Template identifier. Used by toolchain selection and overrides."),
        "family" => Some("Compiler family group such as `gnu`, `llvm`, `intel`, or `nvidia`."),
        "sanitizers" => Some("Sanitizer names supported by this compiler."),
        "homepage" => Some("Informational homepage URL."),
        "binary" => Some("Primary compiler binary probed for version detection."),
        "version_arg" => Some("Argument passed to the binary when detecting compiler version."),
        "version_regex" => Some("Regex with a capture group extracting the version from compiler output."),
        "extensions" => Some("File extensions this template claims during source discovery."),
        "always_flags" => Some("Flags appended to every compile/link invocation for this template."),
        "passthrough" => Some("Whether this template wraps a host compiler and needs passthrough handling."),
        "passthrough_prefix" => Some("Prefix used to pass host compiler flags through a wrapper."),
        "supported_archs" => Some("Host CPU architectures where this toolchain is available."),
        "supported_os" => Some("Host operating systems where this toolchain is available."),
        "required_tools" => Some("Additional PATH tools required for this template to be usable."),
        "required_env" => Some("Environment variables that must be set for this template to be usable."),
        "requires_toolchain" => Some("Language ABI providers required from another detected toolchain."),
        "min_version" => Some("Minimum acceptable compiler version."),
        "include_dir" => Some("Flag template for include directories. Placeholder: `{path}`."),
        "define" => Some("Flag template for defines without values. Placeholder: `{name}`."),
        "define_value" => Some("Flag template for defines with values. Placeholders: `{name}`, `{value}`."),
        "output" => Some("Default output flag template. Placeholder: `{path}`."),
        "output_obj" => Some("Compile-step output flag template when it differs from `output`."),
        "output_bin" => Some("Link-step output flag template when it differs from `output`."),
        "compile_only" => Some("Flag that asks the compiler to compile without linking."),
        "dep_file" => Some("Dependency-file flag template. Placeholder: `{path}`."),
        "dep_file_mode" => Some("Dependency tracking mode: `file`, `stdout`, or `none`."),
        "system_lib" => Some("System library link template. Placeholder: `{name}`."),
        "target" => Some("Cross target triple flag template. Placeholder: `{triple}`."),
        "sysroot" => Some("Sysroot flag template. Placeholder: `{path}`."),
        "flags" => Some("Two-level map for optimization, debug, warning, LTO, sanitizer, CPU extension, stdlib, and runtime flags."),
        "standards" => Some("Map from manifest language standards like `c++20` to compiler flags."),
        "modules" => Some("Module support settings such as `style`, `enable_flag`, `compile_miu`, `import_module`, and `header_unit`."),
        "linking" => Some("Language ABI/linking metadata keyed by language id, e.g. `cpp`, `c`, `fortran`, or `asm`."),
        "toolset" => Some("Tool role to binary map, e.g. `cc`, `cxx`, `ld`, `ar`, `strip`, or `as`."),
        "load_flags" => Some("Role-specific flags appended dynamically by `fn load()`."),
        "arch_flags" => Some("Per-architecture or per-architecture+OS flags. More specific keys win."),
        "pch" => Some("Precompiled header support settings: `compile`, `use`, and `extension`."),
        "env" => Some("Read-only environment map available while evaluating the template."),
        "arch" => Some("Host architecture string available while evaluating the template."),
        "os" => Some("Host operating system string available while evaluating the template."),
        "find_tool" => Some("Finds a binary on PATH; returns unit when not found."),
        "check" => Some("Optional template hook returning whether the toolchain is available."),
        "load" => Some("Optional template hook for dynamic setup such as appending `load_flags`."),
        _ => None,
    }
}
