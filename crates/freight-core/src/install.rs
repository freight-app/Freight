//! `freight install` and `freight package` — copy build outputs to the system.

use std::path::{Path, PathBuf};
use std::fs;
use std::io::{Seek, Write};

use crate::build::build_project_at;
use crate::error::FreightError;
use crate::event::silent;
use crate::manifest::load_manifest;
use crate::manifest::types::LibType;
use crate::toolchain::GlobalConfig;
use crate::vendor::parse_triple;

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
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            prefix:   default_prefix(),
            destdir:  None,
            release:  true,
            no_build: false,
            target:   None,
        }
    }
}

pub enum InstalledKind {
    Binary,
    StaticLib,
    SharedLib,
    Header,
    Symlink,
}

impl InstalledKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Binary    => "binary",
            Self::StaticLib => "static-lib",
            Self::SharedLib => "shared-lib",
            Self::Header    => "header",
            Self::Symlink   => "symlink",
        }
    }
}

pub struct InstalledItem {
    pub dst:  PathBuf,
    pub kind: InstalledKind,
}

pub struct InstallResult {
    pub items: Vec<InstalledItem>,
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Build (unless `opts.no_build`) and install all outputs to `opts.prefix`.
pub fn install_project(project_dir: &Path, opts: &InstallOptions) -> Result<InstallResult, FreightError> {
    let manifest = load_manifest(project_dir)?;
    let profile  = if opts.release { "release" } else { "dev" };

    if !opts.no_build {
        build_project_at(project_dir, profile, &[], true, opts.target.as_deref(), &[], &silent())?;
    }

    // Derive target OS/arch: prefer the explicit override, then ~/.freight/config.toml, then host.
    let global_target = GlobalConfig::load().target;
    let target_str = opts.target.as_deref()
        .or_else(|| global_target.as_deref());
    let (target_arch, target_os) = target_str
        .map(parse_triple)
        .unwrap_or_else(|| (std::env::consts::ARCH.to_string(), std::env::consts::OS.to_string()));

    let root    = install_root(&opts.prefix, opts.destdir.as_deref());
    let bin_dir = root.join("bin");
    let lib_dir = root.join("lib");
    let mut items: Vec<InstalledItem> = Vec::new();

    // ── Binaries ──────────────────────────────────────────────────────────────
    for bin in &manifest.bins {
        let bin_file = executable_name(&bin.name, &target_os);
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
        items.push(InstalledItem { dst, kind: InstalledKind::Binary });
    }

    // ── Library ───────────────────────────────────────────────────────────────
    if let Some(lib) = &manifest.lib {
        fs::create_dir_all(&lib_dir)?;

        match lib.lib_type {
            LibType::Static => {
                let fname = format!("lib{}.a", manifest.package.name);
                let src   = project_dir.join("target").join(profile).join(&fname);
                if src.exists() {
                    let dst = lib_dir.join(&fname);
                    copy_file(&src, &dst)?;
                    set_mode(&dst, 0o644)?;
                    items.push(InstalledItem { dst, kind: InstalledKind::StaticLib });
                }
            }
            LibType::Shared => {
                install_shared_lib(
                    project_dir, profile,
                    &manifest.package.name,
                    &manifest.package.version,
                    &lib_dir, &opts.prefix,
                    &target_os,
                    &mut items,
                )?;
            }
            LibType::HeaderOnly => {}
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
                    items.push(InstalledItem { dst, kind: InstalledKind::Header });
                }
            }
        }
    }

    // On Linux targets: refresh the dynamic linker cache when installing shared
    // libs to a real system path (not a destdir-staged install).
    if target_os == "linux"
        && items.iter().any(|i| matches!(i.kind, InstalledKind::SharedLib))
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
pub fn package_project(project_dir: &Path, release: bool, target: Option<&str>) -> Result<PathBuf, FreightError> {
    let manifest = load_manifest(project_dir)?;
    let profile  = if release { "release" } else { "dev" };

    build_project_at(project_dir, profile, &[], true, target, &[], &silent())?;

    let global_target = GlobalConfig::load().target;
    let (pkg_arch, pkg_os) = target
        .or_else(|| global_target.as_deref())
        .map(parse_triple)
        .unwrap_or_else(|| (std::env::consts::ARCH.to_string(), std::env::consts::OS.to_string()));

    let stem = format!(
        "{}-{}-{}-{}",
        manifest.package.name,
        manifest.package.version,
        pkg_arch,
        pkg_os,
    );

    let pkg_dir = project_dir.join("target").join("package");
    fs::create_dir_all(&pkg_dir)?;

    let staging = pkg_dir.join(&stem);
    if staging.exists() { fs::remove_dir_all(&staging)?; }

    // Install directly into the staging dir (prefix = staging, no destdir).
    install_project(project_dir, &InstallOptions {
        prefix:   staging.clone(),
        destdir:  None,
        release,
        no_build: true,
        target:   target.map(str::to_string),
    })?;

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
            let src = project_dir.join("target").join(profile).join(format!("lib{name}.so"));
            if !src.exists() { return Ok(()); }

            let major      = version.split('.').next().unwrap_or("0");
            let versioned  = format!("lib{name}.so.{version}");
            let soname     = format!("lib{name}.so.{major}");
            let unversioned = format!("lib{name}.so");

            // Install the full versioned file.
            let dst = lib_dir.join(&versioned);
            copy_file(&src, &dst)?;
            set_mode(&dst, 0o755)?;
            items.push(InstalledItem { dst, kind: InstalledKind::SharedLib });

            // libfoo.so.1   → libfoo.so.1.2.3   (SONAME link)
            make_symlink(lib_dir, &soname, &versioned)?;
            items.push(InstalledItem { dst: lib_dir.join(&soname), kind: InstalledKind::Symlink });

            // libfoo.so     → libfoo.so.1         (linker-time link)
            make_symlink(lib_dir, &unversioned, &soname)?;
            items.push(InstalledItem { dst: lib_dir.join(&unversioned), kind: InstalledKind::Symlink });
        }

        "macos" => {
            let src = project_dir.join("target").join(profile).join(format!("lib{name}.dylib"));
            if !src.exists() { return Ok(()); }

            let fname = format!("lib{name}.dylib");
            let dst   = lib_dir.join(&fname);
            copy_file(&src, &dst)?;
            set_mode(&dst, 0o755)?;

            // Update the embedded install name so consumers can find the lib
            // at its installed location without extra DYLD_LIBRARY_PATH magic.
            let install_name = prefix.join("lib").join(&fname);
            let _ = std::process::Command::new("install_name_tool")
                .args(["-id", &install_name.to_string_lossy(), &dst.to_string_lossy().into_owned()])
                .status();

            items.push(InstalledItem { dst, kind: InstalledKind::SharedLib });
        }

        _ => {
            // Windows — DLLs live in bin/, not lib/.
            let src = project_dir.join("target").join(profile).join(format!("{name}.dll"));
            if !src.exists() { return Ok(()); }

            let bin_dir = lib_dir.parent().unwrap_or(lib_dir).join("bin");
            fs::create_dir_all(&bin_dir)?;

            let dst = bin_dir.join(format!("{name}.dll"));
            copy_file(&src, &dst)?;
            items.push(InstalledItem { dst, kind: InstalledKind::SharedLib });

            // Import lib alongside the static libs if present.
            let imp_src = project_dir.join("target").join(profile).join(format!("{name}.lib"));
            if imp_src.exists() {
                let imp_dst = lib_dir.join(format!("{name}.lib"));
                copy_file(&imp_src, &imp_dst)?;
                items.push(InstalledItem { dst: imp_dst, kind: InstalledKind::StaticLib });
            }
        }
    }
    Ok(())
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
    if let Some(p) = dst.parent() { fs::create_dir_all(p)?; }
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
    if link.symlink_metadata().is_ok() { fs::remove_file(&link)?; }
    std::os::unix::fs::symlink(target, &link)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_symlink(_dir: &Path, _link: &str, _target: &str) -> Result<(), FreightError> {
    Ok(()) // Symlinks on Windows require elevated rights; skip silently.
}


fn executable_name(name: &str, target_os: &str) -> String {
    if target_os == "windows" && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
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

fn collect_zip_files(root: &Path, dir: &Path, stem: &str, files: &mut Vec<(String, PathBuf)>) -> Result<(), FreightError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_zip_files(root, &path, stem, files)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel = rel.components()
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
        .args(["-czf", &archive.to_string_lossy(), "-C", &parent.to_string_lossy(), stem])
        .status()
        .map_err(|e| FreightError::InstallFailed(format!("tar not found: {e}")))?;

    if !status.success() {
        return Err(FreightError::InstallFailed("tar exited with non-zero status".into()));
    }
    Ok(())
}

fn run_ldconfig(lib_dir: &Path) {
    // Only meaningful on a Linux host; no-op when cross-compiling from another OS.
    if cfg!(target_os = "linux") {
        // Non-fatal — fails silently when not running as root.
        let _ = std::process::Command::new("ldconfig")
            .arg(lib_dir)
            .status();
    }
}

fn default_prefix() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files")
    } else {
        PathBuf::from("/usr/local")
    }
}

