//! `freight install` and `freight package` — copy build outputs to the system.

use std::fs;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use crate::build::build_project_at;
use crate::environment::Environment;
use crate::error::FreightError;
use crate::event::silent;
use crate::manifest::load_manifest;
use crate::manifest::types::LibType;
use crate::toolchain::GlobalConfig;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct InstallOptions {
    /// Installation prefix, e.g. `/usr/local`. The subdirectories `bin/`,
    /// `lib/`, `include/` are created beneath it.
    pub prefix: PathBuf,
    /// Optional staging root prepended before `prefix` (for package tools).
    /// Actual on-disk path = `destdir / prefix.strip_leading_slash()`.
    pub destdir: Option<PathBuf>,
    /// Build in release mode before installing.
    pub release: bool,
    /// Skip the build step — install whatever is already in `target/`.
    pub no_build: bool,
    /// Cross-compilation target triple (e.g. `aarch64-linux-gnu`).
    /// When set, overrides `[compiler] target` in the manifest and drives
    /// platform-specific install decisions (shared lib naming, DLL placement).
    pub target: Option<String>,
    /// Features to activate for the build (in addition to defaults, unless
    /// `default_features` is false). Affects which sources/deps are compiled and
    /// what ends up in the installed artifact.
    pub features: Vec<String>,
    /// Whether to activate the package's `default` feature set.
    pub default_features: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            destdir: None,
            release: true,
            no_build: false,
            target: None,
            features: Vec::new(),
            default_features: true,
        }
    }
}

pub enum InstalledKind {
    Binary,
    StaticLib,
    SharedLib,
    Header,
    Symlink,
    PkgConfig,
}

impl InstalledKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::StaticLib => "static-lib",
            Self::SharedLib => "shared-lib",
            Self::Header => "header",
            Self::Symlink => "symlink",
            Self::PkgConfig => "pkg-config",
        }
    }
}

pub struct InstalledItem {
    pub dst: PathBuf,
    pub kind: InstalledKind,
}

pub struct InstallResult {
    pub items: Vec<InstalledItem>,
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Build (unless `opts.no_build`) and install all outputs to `opts.prefix`.
///
/// Thin wrapper over [`crate::project::Project::install`], the central path.
pub fn install_project(
    project_dir: &Path,
    opts: &InstallOptions,
) -> Result<InstallResult, FreightError> {
    crate::project::Project::open(project_dir)?.install(opts, &silent())
}

/// Install build outputs that are already compiled (no build step).
pub fn install_project_built(
    project_dir: &Path,
    manifest: &crate::manifest::types::Manifest,
    opts: &InstallOptions,
) -> Result<InstallResult, FreightError> {
    let profile = if opts.release { "release" } else { "debug" };

    // Target OS/arch: the explicit override, then ~/.freight/config.toml, then host.
    let env = Environment::from_config(GlobalConfig::load(), opts.target.clone(), None);
    let (target_arch, target_os) = (env.target_arch.clone(), env.target_os.clone());

    let root = install_root(&opts.prefix, opts.destdir.as_deref());
    let bin_dir = root.join("bin");
    let lib_dir = root.join("lib");
    let mut items: Vec<InstalledItem> = Vec::new();

    // ── Binaries ──────────────────────────────────────────────────────────────
    for bin in &manifest.bins {
        let bin_file = crate::build::link::executable_name(&bin.name, &target_os);
        let src = project_dir.join("target").join(profile).join(&bin_file);
        if !src.exists() {
            return Err(FreightError::InstallFailed(format!(
                "binary '{}' not found in target/{profile}/ — run `freight build` first",
                bin.name
            )));
        }
        fs::create_dir_all(&bin_dir)?;
        let dst = bin_dir.join(&bin_file);
        copy_file(&src, &dst)?;
        set_mode(&dst, 0o755)?;
        items.push(InstalledItem {
            dst,
            kind: InstalledKind::Binary,
        });
    }

    // ── Library ───────────────────────────────────────────────────────────────
    if let Some(lib) = &manifest.lib {
        fs::create_dir_all(&lib_dir)?;

        // Prebuilt libs (link is set) have no built artifact to install.
        if lib.link.is_none() {
            match lib.lib_type {
                LibType::Static => {
                    let fname = format!("lib{}.a", manifest.package.name);
                    let src = project_dir.join("target").join(profile).join(&fname);
                    if src.exists() {
                        let dst = lib_dir.join(&fname);
                        copy_file(&src, &dst)?;
                        set_mode(&dst, 0o644)?;
                        items.push(InstalledItem {
                            dst,
                            kind: InstalledKind::StaticLib,
                        });
                    }
                }
                LibType::Shared => {
                    install_shared_lib(
                        project_dir,
                        profile,
                        &manifest.package.name,
                        &manifest.package.version,
                        &lib_dir,
                        &opts.prefix,
                        &target_os,
                        &mut items,
                    )?;
                }
                LibType::Header => {}
            }
        }

        // ── Public headers ────────────────────────────────────────────────────
        if !lib.hdrs.is_empty() {
            let inc_dst = root.join("include").join(&manifest.package.name);
            std::fs::create_dir_all(&inc_dst)?;
            for hdr in &lib.hdrs {
                let src = project_dir.join(hdr);
                if src.is_file() {
                    let dst = inc_dst.join(src.file_name().unwrap());
                    std::fs::copy(&src, &dst)?;
                    items.push(InstalledItem {
                        dst,
                        kind: InstalledKind::Header,
                    });
                }
            }
        }

        // ── pkg-config descriptor ───────────────────────────────────────────────
        // A `<name>.pc` makes the installed library consumable by every build
        // system that speaks pkg-config (CMake's pkg_check_modules, Meson's
        // dependency(), autotools' PKG_CHECK_MODULES, plain Makefiles, …) — the
        // mirror of `freight migrate` for downstream interop.
        let pc = render_pkg_config(manifest, lib, &opts.prefix);
        let pc_dir = lib_dir.join("pkgconfig");
        fs::create_dir_all(&pc_dir)?;
        let pc_dst = pc_dir.join(format!("{}.pc", manifest.package.name));
        fs::write(&pc_dst, pc)?;
        items.push(InstalledItem {
            dst: pc_dst,
            kind: InstalledKind::PkgConfig,
        });
    }

    // On Linux targets: refresh the dynamic linker cache when installing shared
    // libs to a real system path (not a destdir-staged install).
    if target_os == "linux"
        && items
            .iter()
            .any(|i| matches!(i.kind, InstalledKind::SharedLib))
        && opts.destdir.is_none()
    {
        run_ldconfig(&lib_dir);
    }

    // suppress unused-variable warning when compiled on non-Linux hosts
    let _ = target_arch;

    Ok(InstallResult { items })
}

/// Build in release mode, install to a staging dir, and produce a
/// `{name}-{version}-{arch}-{os}.tar.gz` (or `.zip` for Windows targets) in `target/package/`.
///
/// `target` is an optional cross-compilation triple (e.g. `aarch64-linux-gnu`).
/// When provided it overrides the manifest's `[compiler] target` and is used to
/// derive the arch/os components of the archive filename.
pub fn package_project(
    project_dir: &Path,
    release: bool,
    target: Option<&str>,
) -> Result<PathBuf, FreightError> {
    crate::project::Project::open(project_dir)?.package(release, target, &silent())
}

/// Package build outputs that are already compiled (no build step).
pub fn package_project_built(
    project_dir: &Path,
    manifest: &crate::manifest::types::Manifest,
    release: bool,
    target: Option<&str>,
) -> Result<PathBuf, FreightError> {
    let env = Environment::from_config(GlobalConfig::load(), target.map(str::to_string), None);
    let (pkg_arch, pkg_os) = (env.target_arch.clone(), env.target_os.clone());

    let stem = format!(
        "{}-{}-{}-{}",
        manifest.package.name, manifest.package.version, pkg_arch, pkg_os
    );
    let pkg_dir = project_dir.join("target").join("package");
    fs::create_dir_all(&pkg_dir)?;
    let staging = pkg_dir.join(&stem);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }

    install_project_built(
        project_dir,
        manifest,
        &InstallOptions {
            prefix: staging.clone(),
            destdir: None,
            release,
            no_build: true,
            target: target.map(str::to_string),
            features: Vec::new(),
            default_features: true,
        },
    )?;

    let archive = if pkg_os == "windows" {
        let archive = pkg_dir.join(format!("{stem}.zip"));
        create_zip_archive(&pkg_dir, &stem, &archive)?;
        archive
    } else {
        let archive = pkg_dir.join(format!("{stem}.tar.gz"));
        create_tarball(&pkg_dir, &stem, &archive)?;
        archive
    };
    fs::remove_dir_all(&staging)?;
    Ok(archive)
}

// ── Platform-specific shared lib install ─────────────────────────────────────

fn install_shared_lib(
    project_dir: &Path,
    profile: &str,
    name: &str,
    version: &str,
    lib_dir: &Path,
    prefix: &Path,
    target_os: &str,
    items: &mut Vec<InstalledItem>,
) -> Result<(), FreightError> {
    match target_os {
        "linux" => {
            let src = project_dir
                .join("target")
                .join(profile)
                .join(format!("lib{name}.so"));
            if !src.exists() {
                return Ok(());
            }

            let major = version.split('.').next().unwrap_or("0");
            let versioned = format!("lib{name}.so.{version}");
            let soname = format!("lib{name}.so.{major}");
            let unversioned = format!("lib{name}.so");

            // Install the full versioned file.
            let dst = lib_dir.join(&versioned);
            copy_file(&src, &dst)?;
            set_mode(&dst, 0o755)?;
            items.push(InstalledItem {
                dst,
                kind: InstalledKind::SharedLib,
            });

            // libfoo.so.1   → libfoo.so.1.2.3   (SONAME link)
            make_symlink(lib_dir, &soname, &versioned)?;
            items.push(InstalledItem {
                dst: lib_dir.join(&soname),
                kind: InstalledKind::Symlink,
            });

            // libfoo.so     → libfoo.so.1         (linker-time link)
            make_symlink(lib_dir, &unversioned, &soname)?;
            items.push(InstalledItem {
                dst: lib_dir.join(&unversioned),
                kind: InstalledKind::Symlink,
            });
        }

        "macos" => {
            let src = project_dir
                .join("target")
                .join(profile)
                .join(format!("lib{name}.dylib"));
            if !src.exists() {
                return Ok(());
            }

            let fname = format!("lib{name}.dylib");
            let dst = lib_dir.join(&fname);
            copy_file(&src, &dst)?;
            set_mode(&dst, 0o755)?;

            // Update the embedded install name so consumers can find the lib
            // at its installed location without extra DYLD_LIBRARY_PATH magic.
            let install_name = prefix.join("lib").join(&fname);
            let _ = std::process::Command::new("install_name_tool")
                .args([
                    "-id",
                    &install_name.to_string_lossy(),
                    &dst.to_string_lossy(),
                ])
                .status();

            items.push(InstalledItem {
                dst,
                kind: InstalledKind::SharedLib,
            });
        }

        _ => {
            // Windows — DLLs live in bin/, not lib/.
            let src = project_dir
                .join("target")
                .join(profile)
                .join(format!("{name}.dll"));
            if !src.exists() {
                return Ok(());
            }

            let bin_dir = lib_dir.parent().unwrap_or(lib_dir).join("bin");
            fs::create_dir_all(&bin_dir)?;

            let dst = bin_dir.join(format!("{name}.dll"));
            copy_file(&src, &dst)?;
            items.push(InstalledItem {
                dst,
                kind: InstalledKind::SharedLib,
            });

            // Import lib alongside the static libs if present.
            let imp_src = project_dir
                .join("target")
                .join(profile)
                .join(format!("{name}.lib"));
            if imp_src.exists() {
                let imp_dst = lib_dir.join(format!("{name}.lib"));
                copy_file(&imp_src, &imp_dst)?;
                items.push(InstalledItem {
                    dst: imp_dst,
                    kind: InstalledKind::StaticLib,
                });
            }
        }
    }
    Ok(())
}

// ── pkg-config descriptor ─────────────────────────────────────────────────────

/// Render a `<name>.pc` for an installed library. `prefix` is the *logical*
/// install prefix (e.g. `/usr/local`) — not the destdir-staged path — so the
/// emitted file resolves correctly at the package's final location.
fn render_pkg_config(
    manifest: &crate::manifest::types::Manifest,
    lib: &crate::manifest::types::LibTarget,
    prefix: &Path,
) -> String {
    let name = &manifest.package.name;
    let description = if manifest.package.description.is_empty() {
        name.clone()
    } else {
        manifest.package.description.clone()
    };
    // pkg-config wants forward slashes even on Windows.
    let prefix_str = prefix.to_string_lossy().replace('\\', "/");

    let mut out = String::new();
    out.push_str(&format!("prefix={prefix_str}\n"));
    out.push_str("exec_prefix=${prefix}\n");
    out.push_str("libdir=${prefix}/lib\n");
    out.push_str("includedir=${prefix}/include\n\n");
    out.push_str(&format!("Name: {name}\n"));
    out.push_str(&format!("Description: {description}\n"));
    out.push_str(&format!("Version: {}\n", manifest.package.version));

    // Transitive freight deps that are resolvable by pkg-config name go in
    // Requires.private (only consulted for `--static`), so a missing module
    // never breaks plain dynamic-link consumers.
    let requires = pkg_config_requires(&manifest.dependencies);
    if !requires.is_empty() {
        out.push_str(&format!("Requires.private: {}\n", requires.join(" ")));
    }

    // Headers install under include/<name>/. Offer both roots so consumers can
    // use `<name/foo.h>` or a bare `<foo.h>`.
    out.push_str(&format!(
        "Cflags: -I${{includedir}} -I${{includedir}}/{name}\n"
    ));

    // Header-only libraries have no link line.
    if !matches!(lib.lib_type, crate::manifest::types::LibType::Header) {
        let link = lib.link.clone().unwrap_or_else(|| name.clone());
        out.push_str(&format!("Libs: -L${{libdir}} -l{link}\n"));
    }
    out
}

/// Dependency keys that are plain, system-resolvable version deps (the kind
/// freight itself resolves via pkg-config), suitable for a `.pc` Requires line.
/// Path / git / url / foreign / optional deps are excluded.
fn pkg_config_requires(
    deps: &std::collections::HashMap<String, crate::manifest::types::Dependency>,
) -> Vec<String> {
    use crate::manifest::types::Dependency;
    let mut names: Vec<String> = deps
        .iter()
        .filter_map(|(name, dep)| match dep {
            Dependency::Simple(_) => Some(name.clone()),
            Dependency::Detailed(d) => {
                let plain = d.version.is_some()
                    && d.path.is_none()
                    && d.url.is_none()
                    && d.branch.is_none()
                    && d.tag.is_none()
                    && d.rev.is_none()
                    && !d.optional;
                plain.then(|| name.clone())
            }
        })
        .collect();
    names.sort();
    names
}

// ── File / dir utilities ──────────────────────────────────────────────────────

/// Compute the effective on-disk install root from `prefix` and `destdir`.
fn install_root(prefix: &Path, destdir: Option<&Path>) -> PathBuf {
    match destdir {
        None => prefix.to_path_buf(),
        Some(dd) => {
            // Strip the leading `/` so joining doesn't discard destdir.
            let rel = prefix.strip_prefix("/").unwrap_or(prefix);
            dd.join(rel)
        }
    }
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), FreightError> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(src, dst).map(|_| ())?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), FreightError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), FreightError> {
    Ok(())
}

#[cfg(unix)]
fn make_symlink(dir: &Path, link_name: &str, target: &str) -> Result<(), FreightError> {
    let link = dir.join(link_name);
    // Remove stale link so we can re-link cleanly.
    if link.symlink_metadata().is_ok() {
        fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(target, &link)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_symlink(_dir: &Path, _link: &str, _target: &str) -> Result<(), FreightError> {
    Ok(()) // Symlinks on Windows require elevated rights; skip silently.
}

fn create_zip_archive(parent: &Path, stem: &str, archive: &Path) -> Result<(), FreightError> {
    let root = parent.join(stem);
    let mut files = Vec::new();
    collect_zip_files(&root, &root, stem, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = fs::File::create(archive)?;
    let mut central = Vec::new();

    for (name, path) in files {
        let data = fs::read(&path)?;
        let crc = crc32(&data);
        let offset = out.stream_position()? as u32;
        let name_bytes = name.as_bytes();

        write_u32(&mut out, 0x0403_4b50)?;
        write_u16(&mut out, 20)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u32(&mut out, crc)?;
        write_u32(&mut out, data.len() as u32)?;
        write_u32(&mut out, data.len() as u32)?;
        write_u16(&mut out, name_bytes.len() as u16)?;
        write_u16(&mut out, 0)?;
        out.write_all(name_bytes)?;
        out.write_all(&data)?;

        central.push((name, crc, data.len() as u32, offset));
    }

    let central_start = out.stream_position()? as u32;
    for (name, crc, len, offset) in &central {
        let name_bytes = name.as_bytes();
        write_u32(&mut out, 0x0201_4b50)?;
        write_u16(&mut out, 20)?;
        write_u16(&mut out, 20)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u32(&mut out, *crc)?;
        write_u32(&mut out, *len)?;
        write_u32(&mut out, *len)?;
        write_u16(&mut out, name_bytes.len() as u16)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u16(&mut out, 0)?;
        write_u32(&mut out, 0)?;
        write_u32(&mut out, *offset)?;
        out.write_all(name_bytes)?;
    }
    let central_end = out.stream_position()? as u32;
    let central_size = central_end - central_start;

    write_u32(&mut out, 0x0605_4b50)?;
    write_u16(&mut out, 0)?;
    write_u16(&mut out, 0)?;
    write_u16(&mut out, central.len() as u16)?;
    write_u16(&mut out, central.len() as u16)?;
    write_u32(&mut out, central_size)?;
    write_u32(&mut out, central_start)?;
    write_u16(&mut out, 0)?;

    Ok(())
}

fn collect_zip_files(
    root: &Path,
    dir: &Path,
    stem: &str,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), FreightError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_zip_files(root, &path, stem, files)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((format!("{stem}/{rel}"), path));
        }
    }
    Ok(())
}

fn write_u16<W: Write>(w: &mut W, n: u16) -> Result<(), FreightError> {
    w.write_all(&n.to_le_bytes()).map_err(FreightError::Io)
}

fn write_u32<W: Write>(w: &mut W, n: u32) -> Result<(), FreightError> {
    w.write_all(&n.to_le_bytes()).map_err(FreightError::Io)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn create_tarball(parent: &Path, stem: &str, archive: &Path) -> Result<(), FreightError> {
    // `tar` is available on Linux, macOS, and Windows 10+.
    let status = std::process::Command::new("tar")
        .args([
            "-czf",
            &archive.to_string_lossy(),
            "-C",
            &parent.to_string_lossy(),
            stem,
        ])
        .status()
        .map_err(|e| FreightError::InstallFailed(format!("tar not found: {e}")))?;

    if !status.success() {
        return Err(FreightError::InstallFailed(
            "tar exited with non-zero status".into(),
        ));
    }
    Ok(())
}

// ── Native installer (.deb / .dmg / NSIS .exe) ───────────────────────────────

/// Build a native installer for the target platform:
///
/// - **Linux** → `.deb` package (pure Rust, no external tools required).
///   Installs to `/usr/local/bin` and `/usr/local/lib` by default.
///   Bundles transitive shared-lib dependencies that are not part of glibc.
///
/// - **macOS** → `.dmg` disk image via `hdiutil` (always available on macOS).
///   Bundles dylib dependencies; rewrites install names to `@executable_path/../lib/`.
///
/// - **Windows** → NSIS-based `.exe` installer via `makensis`.
///   Installs to `Program Files`; full Win32 trust. Requires `makensis` on PATH.
///
/// Note: MSIX (Windows sandbox / Store) is a UWP deployment model, not just a
/// packaging format — the app must be built against the UWP API surface. It is
/// not exposed here; see `build_msix` if you need it for a UWP-targeted binary.
pub fn installer_project(
    project_dir: &Path,
    release: bool,
    target: Option<&str>,
) -> Result<PathBuf, FreightError> {
    let manifest = load_manifest(project_dir)?;
    let profile = if release { "release" } else { "debug" };

    build_project_at(
        project_dir,
        profile,
        &[],
        true,
        target,
        &[],
        &silent(),
        None,
    )?;

    let env = Environment::from_config(GlobalConfig::load(), target.map(str::to_string), None);
    let (pkg_arch, pkg_os) = (env.target_arch.clone(), env.target_os.clone());

    let pkg_dir = project_dir.join("target").join("package");
    fs::create_dir_all(&pkg_dir)?;

    // Install into a staging dir with a /usr/local-style layout.
    let stage_name = format!(
        "{}-{}-{}-{}-stage",
        manifest.package.name, manifest.package.version, pkg_arch, pkg_os,
    );
    let staging = pkg_dir.join(&stage_name);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    install_project(
        project_dir,
        &InstallOptions {
            prefix: staging.clone(),
            destdir: None,
            release,
            no_build: true,
            target: target.map(str::to_string),
            features: Vec::new(),
            default_features: true,
        },
    )?;

    // Bundle transitive shared-lib deps.
    bundle_shared_deps(&staging, &pkg_os)?;
    // macOS: rewrite dylib install names so they resolve relative to the bundle.
    if pkg_os == "macos" {
        let bin_dir = staging.join("bin");
        let lib_dir = staging.join("lib");
        if bin_dir.is_dir() {
            for entry in fs::read_dir(&bin_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() {
                    rewrite_macos_rpaths(&path, &lib_dir)?;
                }
            }
        }
    }

    let output = match pkg_os.as_str() {
        "linux" => build_deb(&manifest, &staging, &pkg_dir, &pkg_arch)?,
        "macos" => build_dmg(&manifest, &staging, &pkg_dir)?,
        "windows" => build_nsis(&manifest, &staging, &pkg_dir)?,
        other => {
            return Err(FreightError::InstallFailed(format!(
                "native installer not supported for target OS '{other}'"
            )));
        }
    };

    fs::remove_dir_all(&staging)?;
    Ok(output)
}

// ── Shared-lib bundling ───────────────────────────────────────────────────────

/// Copy transitive shared-lib dependencies for every binary in `staging/bin/`
/// into `staging/lib/` (Linux/macOS) or `staging/bin/` (Windows).
fn bundle_shared_deps(staging: &Path, pkg_os: &str) -> Result<(), FreightError> {
    let bin_dir = staging.join("bin");
    if !bin_dir.is_dir() {
        return Ok(());
    }
    let lib_dir = staging.join("lib");
    fs::create_dir_all(&lib_dir)?;

    for entry in fs::read_dir(&bin_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let deps = collect_shared_deps(&path, pkg_os)?;
        for dep in deps {
            let fname = dep.file_name().unwrap_or_default();
            let dst = if pkg_os == "windows" {
                bin_dir.join(fname)
            } else {
                lib_dir.join(fname)
            };
            if !dst.exists() {
                fs::copy(&dep, &dst).map_err(|e| {
                    FreightError::InstallFailed(format!("bundling {}: {e}", dep.display()))
                })?;
            }
        }
    }
    Ok(())
}

/// Paths to system-provided libraries that should never be bundled.
fn is_system_lib(path: &Path, target_os: &str) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();

    match target_os {
        "linux" => {
            let skip = [
                "libc.so",
                "libm.so",
                "libdl.so",
                "libpthread.so",
                "librt.so",
                "libresolv.so",
                "libutil.so",
                "libnss_",
                "libnsl.so",
                "libgcc_s.so",
                "ld-linux",
                "linux-vdso",
                "linux-gate",
            ];
            skip.iter().any(|s| name.starts_with(s))
        }
        "macos" => {
            path.starts_with("/usr/lib")
                || path.starts_with("/System/")
                || path.starts_with("/Library/Apple/")
        }
        "windows" => {
            let skip = [
                "kernel32.dll",
                "user32.dll",
                "gdi32.dll",
                "ole32.dll",
                "oleaut32.dll",
                "ntdll.dll",
                "advapi32.dll",
                "shell32.dll",
                "shlwapi.dll",
                "ws2_32.dll",
                "msvcp",
                "vcruntime",
                "ucrtbase",
                "api-ms-win",
                "ext-ms-win",
            ];
            skip.iter().any(|s| name.starts_with(s))
        }
        _ => false,
    }
}

fn collect_shared_deps(binary: &Path, target_os: &str) -> Result<Vec<PathBuf>, FreightError> {
    match target_os {
        "linux" => collect_deps_ldd(binary, target_os),
        "macos" => collect_deps_otool(binary, target_os),
        "windows" => collect_deps_dumpbin(binary, target_os),
        other => {
            eprintln!("warning: shared-lib collection not supported on {other}");
            Ok(vec![])
        }
    }
}

fn collect_deps_ldd(binary: &Path, target_os: &str) -> Result<Vec<PathBuf>, FreightError> {
    let out = std::process::Command::new("ldd")
        .arg(binary)
        .output()
        .map_err(|e| FreightError::InstallFailed(format!("ldd not found: {e}")))?;
    if !out.status.success() {
        return Ok(vec![]);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut deps = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        let path_str = if let Some(idx) = line.find("=>") {
            let after = line[idx + 2..].trim();
            after.split_whitespace().next().filter(|&p| p != "not")
        } else {
            let p = match line.split_whitespace().next() {
                Some(p) => p,
                None => continue,
            };
            if p.starts_with('/') {
                Some(p)
            } else {
                None
            }
        };
        if let Some(p) = path_str {
            let pb = PathBuf::from(p);
            if pb.exists() && !is_system_lib(&pb, target_os) {
                deps.push(pb);
            }
        }
    }
    Ok(deps)
}

fn collect_deps_otool(binary: &Path, target_os: &str) -> Result<Vec<PathBuf>, FreightError> {
    let out = std::process::Command::new("otool")
        .args(["-L", &binary.to_string_lossy()])
        .output()
        .map_err(|e| FreightError::InstallFailed(format!("otool not found: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut deps = Vec::new();
    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if let Some(path_str) = line.split(' ').next() {
            let pb = PathBuf::from(path_str);
            if pb.is_absolute() && pb.exists() && !is_system_lib(&pb, target_os) {
                deps.push(pb);
            }
        }
    }
    Ok(deps)
}

fn collect_deps_dumpbin(binary: &Path, target_os: &str) -> Result<Vec<PathBuf>, FreightError> {
    let out = match std::process::Command::new("dumpbin")
        .args(["/DEPENDENTS", &binary.to_string_lossy()])
        .output()
    {
        Ok(o) => o,
        Err(_) => return collect_deps_ldd(binary, target_os),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut deps = Vec::new();
    let mut in_section = false;
    for line in stdout.lines() {
        let line = line.trim();
        if line.contains("has the following dependencies") {
            in_section = true;
            continue;
        }
        if in_section {
            if line.is_empty() {
                break;
            }
            if line.to_ascii_lowercase().ends_with(".dll") {
                let path_var = std::env::var("PATH").unwrap_or_default();
                for dir in std::env::split_paths(&path_var) {
                    let candidate = dir.join(line);
                    if candidate.exists() && !is_system_lib(&candidate, target_os) {
                        deps.push(candidate);
                        break;
                    }
                }
            }
        }
    }
    Ok(deps)
}

// ── macOS rpath rewriting ─────────────────────────────────────────────────────

fn rewrite_macos_rpaths(binary: &Path, bundled_lib_dir: &Path) -> Result<(), FreightError> {
    if !bundled_lib_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(bundled_lib_dir)? {
        let entry = entry?;
        let lib = entry.path();
        if lib.extension().is_some_and(|e| e == "dylib") {
            let old = lib.to_string_lossy().into_owned();
            let new = format!(
                "@executable_path/../lib/{}",
                lib.file_name().unwrap_or_default().to_string_lossy()
            );
            let _ = std::process::Command::new("install_name_tool")
                .args(["-change", &old, &new, &binary.to_string_lossy()])
                .status();
        }
    }
    Ok(())
}

// ── Linux .deb builder ────────────────────────────────────────────────────────

/// Build a `.deb` package from the staging directory.
///
/// The `.deb` format is an `ar` archive containing three members:
/// `debian-binary`, `control.tar.gz`, and `data.tar.gz`.
fn build_deb(
    manifest: &crate::manifest::types::Manifest,
    staging: &Path,
    pkg_dir: &Path,
    pkg_arch: &str,
) -> Result<PathBuf, FreightError> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let deb_arch = deb_arch(pkg_arch);

    // Compute installed size in KiB.
    let installed_kb = dir_size_kb(staging);

    let maintainer = manifest
        .package
        .authors
        .first()
        .cloned()
        .unwrap_or_else(|| "Unknown".to_string());
    let description = if manifest.package.description.is_empty() {
        name.clone()
    } else {
        manifest.package.description.clone()
    };

    // ── control.tar.gz ────────────────────────────────────────────────────────
    let control = format!(
        "Package: {name}\n\
         Version: {version}\n\
         Architecture: {deb_arch}\n\
         Maintainer: {maintainer}\n\
         Installed-Size: {installed_kb}\n\
         Description: {description}\n"
    );

    let control_tar_gz = {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut ar = tar::Builder::new(enc);
            let data = control.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_path("./control")?;
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            ar.append(&header, data)?;
            ar.into_inner()?.finish()?;
        }
        buf
    };

    // ── data.tar.gz ───────────────────────────────────────────────────────────
    // Map staging layout (bin/, lib/, include/) into /usr/local/{bin,lib,include}.
    let data_tar_gz = {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut ar = tar::Builder::new(enc);
            append_dir_to_tar(&mut ar, staging, staging, "/usr/local")?;
            ar.into_inner()?.finish()?;
        }
        buf
    };

    // ── ar archive ────────────────────────────────────────────────────────────
    let deb_path = pkg_dir.join(format!("{name}_{version}_{deb_arch}.deb"));
    let mut out = fs::File::create(&deb_path)?;

    // ar global header
    out.write_all(b"!<arch>\n")?;
    write_ar_member(&mut out, "debian-binary", b"2.0\n")?;
    write_ar_member(&mut out, "control.tar.gz", &control_tar_gz)?;
    write_ar_member(&mut out, "data.tar.gz", &data_tar_gz)?;

    Ok(deb_path)
}

/// Map Freight arch names to Debian arch names.
fn deb_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "i686" | "i386" => "i386",
        "arm" => "armhf",
        "riscv64" => "riscv64",
        other => other,
    }
}

/// Recursively walk `dir` and append every file to `ar` under `dest_prefix`.
fn append_dir_to_tar<W: Write>(
    ar: &mut tar::Builder<W>,
    root: &Path,
    dir: &Path,
    dest_prefix: &str,
) -> Result<(), FreightError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel_str = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let ar_path = format!("{dest_prefix}/{rel_str}");

        if path.is_dir() {
            append_dir_to_tar(ar, root, &path, dest_prefix)?;
        } else if path.is_file() {
            let mut f = fs::File::open(&path)?;
            let meta = f.metadata()?;
            let mut header = tar::Header::new_gnu();
            header.set_path(&ar_path)?;
            header.set_size(meta.len());
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                header.set_mode(meta.permissions().mode());
            }
            #[cfg(not(unix))]
            header.set_mode(0o644);
            header.set_cksum();
            ar.append(&header, &mut f)?;
        }
    }
    Ok(())
}

/// Write one member into an `ar` archive.
fn write_ar_member<W: Write>(w: &mut W, name: &str, data: &[u8]) -> Result<(), FreightError> {
    // ar member header: 60 bytes, all ASCII, space-padded.
    let mut header = [b' '; 60];
    // Name field (16 bytes)
    let name_bytes = name.as_bytes();
    let len = name_bytes.len().min(16);
    header[..len].copy_from_slice(&name_bytes[..len]);
    // Timestamp (12 bytes) at offset 16 — leave as spaces (= 0).
    // UID / GID (6 bytes each) at 28/34 — spaces = 0.
    // File mode (8 bytes) at 40.
    let mode = b"100644  ";
    header[40..48].copy_from_slice(mode);
    // File size (10 bytes) at 48.
    let size_str = format!("{:<10}", data.len());
    header[48..58].copy_from_slice(size_str.as_bytes());
    // End-of-header magic at 58–59.
    header[58] = b'`';
    header[59] = b'\n';

    w.write_all(&header)?;
    w.write_all(data)?;
    // ar requires each member to start on a 2-byte boundary.
    if !data.len().is_multiple_of(2) {
        w.write_all(b"\n")?;
    }
    Ok(())
}

fn dir_size_kb(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                total += fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            } else if path.is_dir() {
                total += dir_size_kb(&path) * 1024;
            }
        }
    }
    total.div_ceil(1024)
}

// ── macOS .dmg builder ────────────────────────────────────────────────────────

/// Create a `.dmg` disk image from the staging directory using `hdiutil`.
fn build_dmg(
    manifest: &crate::manifest::types::Manifest,
    staging: &Path,
    pkg_dir: &Path,
) -> Result<PathBuf, FreightError> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let dmg_path = pkg_dir.join(format!("{name}-{version}.dmg"));

    let vol_name = format!("{name} {version}");
    let status = std::process::Command::new("hdiutil")
        .args([
            "create",
            "-volname",
            &vol_name,
            "-srcfolder",
            &staging.to_string_lossy(),
            "-ov",
            "-format",
            "UDZO",
            &dmg_path.to_string_lossy(),
        ])
        .status()
        .map_err(|e| {
            FreightError::InstallFailed(format!("hdiutil not found — is this a macOS host? ({e})"))
        })?;

    if !status.success() {
        return Err(FreightError::InstallFailed(
            "hdiutil exited with non-zero status".into(),
        ));
    }
    Ok(dmg_path)
}

// ── Windows NSIS .exe builder ─────────────────────────────────────────────────

/// Generate an NSIS installer script and run `makensis` to produce a `.exe`.
///
/// Requires NSIS to be installed (`makensis` on PATH).
/// On Windows: `winget install NSIS.NSIS` or `choco install nsis`.
/// On Linux (cross): `apt install nsis`.
fn build_nsis(
    manifest: &crate::manifest::types::Manifest,
    staging: &Path,
    pkg_dir: &Path,
) -> Result<PathBuf, FreightError> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;
    let exe_path = pkg_dir.join(format!("{name}-{version}-setup.exe"));

    // Find the first binary to use as the main shortcut target.
    let first_bin = manifest
        .bins
        .first()
        .map(|b| format!("bin\\{}.exe", b.name))
        .unwrap_or_else(|| format!("bin\\{name}.exe"));

    let nsi = format!(
        r#"!include "MUI2.nsh"
Unicode true

Name "{name} {version}"
OutFile "{out}"
InstallDir "$PROGRAMFILES64\{name}"
InstallDirRegKey HKLM "Software\{name}" "Install_Dir"
RequestExecutionLevel admin

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "{staging}\*"
  CreateShortcut "$DESKTOP\{name}.lnk" "$INSTDIR\{first_bin}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{name}" \
    "DisplayName" "{name} {version}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{name}" \
    "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  RMDir /r "$INSTDIR"
  Delete "$DESKTOP\{name}.lnk"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\{name}"
SectionEnd
"#,
        name = name,
        version = version,
        out = exe_path.to_string_lossy().replace('/', "\\"),
        staging = staging.to_string_lossy().replace('/', "\\"),
        first_bin = first_bin,
    );

    let nsi_path = pkg_dir.join(format!("{name}-{version}.nsi"));
    fs::write(&nsi_path, &nsi)?;

    let status = std::process::Command::new("makensis")
        .arg(&nsi_path)
        .status()
        .map_err(|_| {
            FreightError::InstallFailed(
                "makensis not found — install NSIS first:\n  \
             Windows: winget install NSIS.NSIS\n  \
             Linux:   apt install nsis"
                    .into(),
            )
        })?;

    let _ = fs::remove_file(&nsi_path);

    if !status.success() {
        return Err(FreightError::InstallFailed(
            "makensis exited with non-zero status".into(),
        ));
    }
    Ok(exe_path)
}

// ── Windows MSIX builder ──────────────────────────────────────────────────────
// Not exposed through the CLI. MSIX is a UWP deployment model — the app must
// be built against UWP APIs. These helpers are reserved for when UWP targeting
// is implemented as a proper first-class build target.

/// Build an MSIX package via `makeappx.exe` (part of the Windows SDK).
///
/// The package runs in Windows' virtualised app container, making it suitable
/// for the Microsoft Store, Windows App Installer, and Windows Sandbox.
///
/// # Signing
/// The produced `.msix` is **unsigned**. To sideload it without the Microsoft
/// Store, either:
/// - Enable **Developer Mode** in Windows Settings → System → For developers, or
/// - Sign with `signtool.exe` using a trusted certificate:
///   ```text
///   signtool sign /fd SHA256 /a myapp-1.0.msix
///   ```
///
/// # Requires
/// `makeappx.exe` on PATH (part of the Windows SDK / Visual Studio).
/// On Windows: installed automatically with Visual Studio or the Windows SDK.
/// In CI: available in `windows-latest` GitHub Actions runners.
#[allow(dead_code)]
fn build_msix(
    manifest: &crate::manifest::types::Manifest,
    staging: &Path,
    pkg_dir: &Path,
    pkg_arch: &str,
) -> Result<PathBuf, FreightError> {
    let name = &manifest.package.name;
    let version = &manifest.package.version;

    // MSIX Identity/@Version must be a four-part dotted number.
    let msix_version = pad_version_to_four(version);
    let msix_arch = msix_arch(pkg_arch);
    let publisher = manifest
        .package
        .authors
        .first()
        .map(|a| format!("CN={a}"))
        .unwrap_or_else(|| format!("CN={name}"));
    let description = if manifest.package.description.is_empty() {
        name.clone()
    } else {
        manifest.package.description.clone()
    };

    let first_bin = manifest
        .bins
        .first()
        .map(|b| format!("bin\\{}.exe", b.name))
        .unwrap_or_else(|| format!("bin\\{name}.exe"));

    // Create a temporary staging dir that holds all MSIX contents.
    let msix_stage = pkg_dir.join(format!("{name}-{version}-msix-stage"));
    if msix_stage.exists() {
        fs::remove_dir_all(&msix_stage)?;
    }

    // Copy the installed layout (bin/, lib/) into the MSIX staging root.
    copy_dir_all(staging, &msix_stage)?;

    // Write placeholder logo assets.
    let assets_dir = msix_stage.join("assets");
    fs::create_dir_all(&assets_dir)?;
    fs::write(
        assets_dir.join("logo44.png"),
        solid_png(44, 44, [0x00, 0x78, 0xd7]),
    )?;
    fs::write(
        assets_dir.join("logo150.png"),
        solid_png(150, 150, [0x00, 0x78, 0xd7]),
    )?;

    // AppxManifest.xml
    let manifest_xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  IgnorableNamespaces="uap">

  <Identity
    Name="{name}"
    Version="{msix_version}"
    Publisher="{publisher}"
    ProcessorArchitecture="{msix_arch}" />

  <Properties>
    <DisplayName>{name}</DisplayName>
    <PublisherDisplayName>{publisher}</PublisherDisplayName>
    <Logo>assets\logo150.png</Logo>
  </Properties>

  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop"
      MinVersion="10.0.17763.0" MaxVersionTested="10.0.22621.0" />
  </Dependencies>

  <Resources>
    <Resource Language="en-us" />
  </Resources>

  <Applications>
    <Application Id="App"
      Executable="{first_bin}"
      EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements
        DisplayName="{name}"
        Description="{description}"
        BackgroundColor="transparent"
        Square44x44Logo="assets\logo44.png"
        Square150x150Logo="assets\logo150.png" />
    </Application>
  </Applications>

</Package>
"#,
        name = name,
        msix_version = msix_version,
        publisher = publisher,
        msix_arch = msix_arch,
        first_bin = first_bin,
        description = description,
    );
    fs::write(msix_stage.join("AppxManifest.xml"), manifest_xml.as_bytes())?;

    // [Content_Types].xml — makeappx generates this automatically, but
    // providing it lets us pre-declare every extension in the package.
    let content_types = r#"<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="exe"  ContentType="application/octet-stream" />
  <Default Extension="dll"  ContentType="application/octet-stream" />
  <Default Extension="png"  ContentType="image/png" />
  <Override PartName="/AppxManifest.xml"
    ContentType="application/vnd.ms-appx.manifest+xml" />
</Types>
"#;
    fs::write(
        msix_stage.join("[Content_Types].xml"),
        content_types.as_bytes(),
    )?;

    let msix_path = pkg_dir.join(format!("{name}-{version}.msix"));

    let status = std::process::Command::new("makeappx")
        .args([
            "pack",
            "/d",
            &msix_stage.to_string_lossy(),
            "/p",
            &msix_path.to_string_lossy(),
            "/nv", // skip validation so unsigned builds work
            "/o",  // overwrite if exists
        ])
        .status()
        .map_err(|_| {
            FreightError::InstallFailed(
                "makeappx.exe not found — install the Windows SDK or Visual Studio.\n  \
             In GitHub Actions use windows-latest; it ships with the SDK."
                    .into(),
            )
        })?;

    fs::remove_dir_all(&msix_stage)?;

    if !status.success() {
        return Err(FreightError::InstallFailed(
            "makeappx exited with non-zero status".into(),
        ));
    }

    eprintln!(
        "note: {name}-{version}.msix is unsigned.\n      \
         To sideload without the Store, enable Developer Mode or sign with:\n      \
         signtool sign /fd SHA256 /a {name}-{version}.msix"
    );

    Ok(msix_path)
}

/// Convert a semver string to the four-part `Major.Minor.Patch.0` required by MSIX Identity.
#[allow(dead_code)]
fn pad_version_to_four(v: &str) -> String {
    let parts: Vec<&str> = v.splitn(4, '.').collect();
    match parts.len() {
        1 => format!("{}.0.0.0", parts[0]),
        2 => format!("{}.{}.0.0", parts[0], parts[1]),
        3 => format!("{}.{}.{}.0", parts[0], parts[1], parts[2]),
        _ => v.to_string(),
    }
}

/// Map Freight arch names to MSIX `ProcessorArchitecture` values.
#[allow(dead_code)]
fn msix_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "i686" | "i386" | "x86" => "x86",
        "arm" => "arm",
        _ => "neutral",
    }
}

/// Recursively copy a directory tree.
#[allow(dead_code)]
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), FreightError> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Generate a solid-colour RGB PNG of the given dimensions.
///
/// Uses only `flate2` (already a crate dependency) for zlib compression.
/// Each scanline uses PNG filter type 0 (None) so the raw pixel data is
/// directly compressible.
#[allow(dead_code)]
fn solid_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    use flate2::{write::ZlibEncoder, Compression};

    // Raw image data: filter_byte(0) + width * RGB per scanline.
    let row_len = 1 + (width as usize) * 3;
    let mut raw = vec![0u8; (height as usize) * row_len];
    for y in 0..height as usize {
        let base = y * row_len;
        // raw[base] = 0  (filter type: None — already zero)
        for x in 0..width as usize {
            let p = base + 1 + x * 3;
            raw[p] = rgb[0];
            raw[p + 1] = rgb[1];
            raw[p + 2] = rgb[2];
        }
    }

    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut zlib, &raw).expect("in-memory write");
    let idat_data = zlib.finish().expect("zlib finish");

    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    // IHDR: width(4) + height(4) + bit_depth(1) + colour_type(2=RGB) + compress/filter/interlace
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // colour type: RGB
                 // bytes 10–12 remain 0 (compression=0, filter=0, interlace=0)
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &idat_data);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

#[allow(dead_code)]
fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    // CRC32 over tag + data using the standard IEEE 802.3 polynomial.
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn run_ldconfig(lib_dir: &Path) {
    // Only meaningful on a Linux host; no-op when cross-compiling from another OS.
    if cfg!(target_os = "linux") {
        // Non-fatal — fails silently when not running as root.
        let _ = std::process::Command::new("ldconfig").arg(lib_dir).status();
    }
}

fn default_prefix() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files")
    } else {
        PathBuf::from("/usr/local")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::types::Manifest;

    fn manifest(toml: &str) -> Manifest {
        toml::from_str(toml).expect("parse test manifest")
    }

    #[test]
    fn pkg_config_static_lib_with_requires() {
        let m = manifest(
            r#"
            [package]
            name = "mylib"
            version = "1.2.3"
            description = "My test library"

            [lib]
            type = "static"

            [dependencies]
            zlib = "1.3"
            bar = { version = "2.0" }
            foo = { path = "../foo" }
            opt = { version = "1.0", optional = true }
            "#,
        );
        let pc = render_pkg_config(&m, m.lib.as_ref().unwrap(), Path::new("/usr/local"));

        assert!(pc.contains("prefix=/usr/local\n"), "{pc}");
        assert!(pc.contains("Name: mylib\n"), "{pc}");
        assert!(pc.contains("Description: My test library\n"), "{pc}");
        assert!(pc.contains("Version: 1.2.3\n"), "{pc}");
        // Sorted; path dep and optional dep excluded.
        assert!(pc.contains("Requires.private: bar zlib\n"), "{pc}");
        assert!(
            pc.contains("Cflags: -I${includedir} -I${includedir}/mylib\n"),
            "{pc}"
        );
        assert!(pc.contains("Libs: -L${libdir} -lmylib\n"), "{pc}");
    }

    #[test]
    fn pkg_config_header_only_has_no_libs() {
        let m = manifest(
            r#"
            [package]
            name = "headeronly"
            version = "0.1.0"

            [lib]
            type = "header"
            "#,
        );
        let pc = render_pkg_config(&m, m.lib.as_ref().unwrap(), Path::new("/opt/x"));
        assert!(pc.contains("Description: headeronly\n"), "{pc}"); // falls back to name
        assert!(
            !pc.contains("\nLibs:"),
            "header-only must have no Libs line: {pc}"
        );
        assert!(!pc.contains("Requires.private"), "{pc}");
    }

    #[test]
    fn pkg_config_respects_custom_link_name() {
        let m = manifest(
            r#"
            [package]
            name = "mypkg"
            version = "3.0.0"

            [lib]
            type = "shared"
            link = "customname"
            "#,
        );
        let pc = render_pkg_config(&m, m.lib.as_ref().unwrap(), Path::new("/usr/local"));
        assert!(pc.contains("Libs: -L${libdir} -lcustomname\n"), "{pc}");
    }
}
