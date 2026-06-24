//! `freight-system-registry` — generate a system-side registry.
//!
//! For every pkg-config package installed on the host, write a `[package]` stub
//! `.toml` into the output directory:
//!   * if the package exists in a freight registry, use that registry's metadata
//!     (version + description);
//!   * otherwise synthesize a stub — version from pkg-config, description from the
//!     system package manager (apt/dnf/pacman/…) that owns the `.pc` file.
//!
//! The result lets freight resolve locally-installed libraries against real
//! package metadata without hitting the network.
//!
//! ```text
//! freight-system-registry [--out DIR] [--force] [--no-registry] [--limit N]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use freight::registry::{repos::registries_in_order, PackageInfo, PackageRepo};
use freight::resolve::pkg_config::{pkg_config_list_all, pkg_config_version};
use freight::resolve::system_pm::{self, pc_file_path, SystemPm};
use freight::resolve::system_registry::{render_package_stub, system_registry_dir, StubSource};
use freight::toolchain::cache::GlobalConfig;

fn main() -> ExitCode {
    let mut out: Option<PathBuf> = None;
    let mut force = false;
    let mut use_registry = true;
    let mut limit = 0usize;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" | "-o" => match args.next() {
                Some(v) => out = Some(PathBuf::from(v)),
                None => return die("--out needs a directory"),
            },
            "--force" | "-f" => force = true,
            "--no-registry" => use_registry = false,
            "--limit" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => limit = n,
                None => return die("--limit needs a number"),
            },
            "-h" | "--help" => {
                eprintln!(
                    "freight-system-registry [--out DIR] [--force] [--no-registry] [--limit N]"
                );
                return ExitCode::SUCCESS;
            }
            other => return die(&format!("unknown argument: {other}")),
        }
    }

    let out_dir = match out.or_else(system_registry_dir) {
        Some(d) => d,
        None => return die("cannot determine FREIGHT_HOME; pass --out DIR"),
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        return die(&format!("cannot create {}: {e}", out_dir.display()));
    }

    let mut packages = pkg_config_list_all();
    if packages.is_empty() {
        return die("no pkg-config packages found (is pkg-config installed?)");
    }
    if limit > 0 {
        packages.truncate(limit);
    }

    let registry = use_registry.then(Registries::new);
    let pm = system_pm::detect();
    eprintln!(
        "freight-system-registry: {} pkg-config packages → {} (pm: {}, registry: {})",
        packages.len(),
        out_dir.display(),
        pm.map(SystemPm::name).unwrap_or("none"),
        if registry.is_some() { "on" } else { "off" },
    );

    let (mut from_registry, mut from_system, mut written, mut skipped) = (0, 0, 0, 0);
    for (name, pc_desc) in &packages {
        let dest = out_dir.join(format!("{name}.toml"));
        if dest.exists() && !force {
            skipped += 1;
            continue;
        }

        let stub = match registry.as_ref().and_then(|r| r.lookup(name)) {
            Some(info) => {
                from_registry += 1;
                let desc = info.description.unwrap_or_else(|| pc_desc.clone());
                render_package_stub(name, &info.latest, &desc, StubSource::Registry)
            }
            None => {
                from_system += 1;
                let version = pkg_config_version(name);
                // Prefer the OS package manager's description; fall back to the
                // one pkg-config itself reports in `--list-all`.
                let desc = pm
                    .and_then(|pm| pc_file_path(name).and_then(|pc| pm.describe(&pc)))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| pc_desc.clone());
                render_package_stub(name, &version, &desc, StubSource::System)
            }
        };

        if let Err(e) = std::fs::write(&dest, stub) {
            eprintln!("  warn: failed to write {}: {e}", dest.display());
            continue;
        }
        written += 1;
    }

    eprintln!(
        "done: {written} written ({from_registry} from registry, {from_system} from system), {skipped} skipped (exist; use --force)"
    );
    ExitCode::SUCCESS
}

/// Best-effort lookup across configured registries. A 404 is a normal miss; the
/// first transport error flips the whole thing off so an offline run doesn't
/// retry an unreachable registry once per pkg-config package.
struct Registries {
    repos: Vec<Box<dyn PackageRepo>>,
    reachable: std::cell::Cell<bool>,
}

impl Registries {
    fn new() -> Self {
        Self {
            repos: registries_in_order(&GlobalConfig::load()),
            reachable: std::cell::Cell::new(true),
        }
    }

    fn lookup(&self, name: &str) -> Option<PackageInfo> {
        if !self.reachable.get() {
            return None;
        }
        for repo in &self.repos {
            match repo.lookup(name, None) {
                Ok(Some(info)) => return Some(info),
                Ok(None) => continue,
                Err(_) => {
                    self.reachable.set(false);
                    return None;
                }
            }
        }
        None
    }
}

fn die(msg: &str) -> ExitCode {
    eprintln!("freight-system-registry: {msg}");
    ExitCode::FAILURE
}
