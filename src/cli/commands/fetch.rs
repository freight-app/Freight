use freight::dep_cmds::{
    fetch_git_deps, fetch_package_deps, fetch_registry_deps, fetch_url_deps, GitDepAction,
    PackageDepAction, RegistryDepAction,
};
use freight::manifest::load_manifest;
use freight::manifest::types::{Dependency, Manifest};
use freight::registry::freight_registry::FreightRegistry;
use freight::registry::host_triple;
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success, print_warning};

#[derive(clap::ValueEnum, Clone, Default, PartialEq)]
pub enum PrebuiltKind {
    /// Download a release (optimised) prebuilt for the target triple
    #[default]
    Release,
    /// Download a debug prebuilt for the target triple
    Debug,
    /// Always fetch source — skip prebuilt lookup entirely
    Source,
}

#[derive(clap::Args)]
pub struct Args {
    /// Prebuilt variant to prefer: release (default), debug, or source
    #[arg(long, short = 'p', value_name = "KIND", default_value = "release")]
    pub prebuilt: PrebuiltKind,
    /// Target triple for cross-compile prebuilt selection (e.g. aarch64-linux-gnu).
    /// Defaults to the host triple.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
    /// Parallel jobs for building foreign dependencies (cmake/make/meson/autotools)
    #[arg(long, short = 'j', value_name = "N")]
    pub jobs: Option<usize>,
}

impl Args {
    pub fn run(self) {
        super::common::apply_jobs(self.jobs);
        let triple = self.target.unwrap_or_else(host_triple);
        cmd_fetch(self.prebuilt, &triple);
    }
}

fn cmd_fetch(prebuilt: PrebuiltKind, triple: &str) {
    let project_dir = match super::common::locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let mut all_ok = true;
    let mut any_work = false;

    for (name, dep) in &manifest.dependencies {
        match dep {
            Dependency::Detailed(_) if freight::manifest::types::is_platform_dep(name) => {
                print_status("skip", &format!("{name} (platform)"));
            }
            Dependency::Detailed(d) if d.path.is_some() && d.dep_type.is_none() => {
                any_work = true;
                let rel = d.path.as_deref().unwrap();
                let dep_dir = project_dir.join(rel);
                if dep_dir.join("freight.toml").exists() {
                    print_status("ok", &format!("{name} (path+{rel})"));
                } else {
                    print_error(&format!("{name}: not found at {rel}"));
                    all_ok = false;
                }
            }
            Dependency::Detailed(d) if d.dep_type.is_some() => {
                print_status("skip", &format!("{name} (foreign — built on demand)"));
            }
            _ => {}
        }
    }

    match fetch_git_deps(&project_dir) {
        Ok(outcomes) => {
            for o in outcomes {
                any_work = true;
                match o.action {
                    GitDepAction::Cloned => print_success(&format!("cloned `{}`", o.name)),
                    GitDepAction::AlreadyPresent => {
                        print_status("ok", &format!("{} (git, up to date)", o.name))
                    }
                    _ => {}
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            all_ok = false;
        }
    }

    match fetch_url_deps(&project_dir) {
        Ok(outcomes) => {
            for (name, already_present) in outcomes {
                any_work = true;
                if already_present {
                    print_status("ok", &format!("{name} (http, up to date)"));
                } else {
                    print_success(&format!("fetched `{name}`"));
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            all_ok = false;
        }
    }

    match fetch_package_deps(&project_dir) {
        Ok(outcomes) => {
            for outcome in outcomes {
                any_work = true;
                match outcome.action {
                    PackageDepAction::SystemPresent => {
                        print_status("ok", &format!("{} (system)", outcome.name));
                    }
                    PackageDepAction::AlreadyPresent => {
                        print_status("ok", &format!("{} (cached)", outcome.name));
                    }
                    PackageDepAction::Fetched => {
                        print_success(&format!("fetched `{}`", outcome.name));
                    }
                    PackageDepAction::Missing => {
                        print_warning(&format!(
                            "`{}` not found locally or via pkg-config — \
                             run `freight build` to trigger registry fetch",
                            outcome.name
                        ));
                    }
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            all_ok = false;
        }
    }

    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };

    if prebuilt != PrebuiltKind::Source {
        let effective_triple = match prebuilt {
            PrebuiltKind::Debug => format!("{triple}-debug"),
            _ => triple.to_string(),
        };
        fetch_prebuilt_deps(
            &manifest,
            &project_dir,
            &config,
            &effective_triple,
            &mut any_work,
            &mut all_ok,
        );
    }

    match fetch_registry_deps(&project_dir, &config) {
        Ok(outcomes) => {
            for o in outcomes {
                let sentinel = project_dir
                    .join(".pkgs")
                    .join(&o.name)
                    .join(".freight-fetched");
                if sentinel.exists() {
                    continue;
                }
                any_work = true;
                match o.action {
                    RegistryDepAction::AlreadyPresent => {
                        print_status("ok", &format!("{} (source, up to date)", o.name));
                    }
                    RegistryDepAction::Downloaded => {
                        print_success(&format!("fetched `{}@{}` (source)", o.name, o.version));
                    }
                    RegistryDepAction::Unavailable => {
                        print_warning(&format!(
                            "`{}@{}` not found in any registry — run `freight login` or check your config",
                            o.name, o.version
                        ));
                        all_ok = false;
                    }
                }
            }
        }
        Err(e) => {
            print_error(&e.to_string());
            all_ok = false;
        }
    }

    if !any_work {
        println!("no dependencies to fetch");
        return;
    }

    if all_ok {
        println!();
        print_success("all dependencies ready");
    }
}

fn fetch_prebuilt_deps(
    manifest: &Manifest,
    project_dir: &std::path::Path,
    config: &GlobalConfig,
    triple: &str,
    any_work: &mut bool,
    all_ok: &mut bool,
) {
    for (name, dep) in &manifest.dependencies {
        let (version, repo_key, channel) = match dep {
            Dependency::Simple(v) => (v.as_str(), None, None),
            Dependency::Detailed(d)
                if d.version.is_some()
                    && d.path.is_none()
                    && d.git.is_none()
                    && d.url.is_none()
                    && !freight::manifest::types::is_platform_dep(name) =>
            {
                (
                    d.version.as_deref().unwrap(),
                    d.registry.as_deref(),
                    d.channel.as_deref(),
                )
            }
            _ => continue,
        };

        if version.is_empty() || version == "*" {
            continue;
        }

        let sentinel = project_dir
            .join(".pkgs")
            .join(name)
            .join(".freight-fetched");
        if sentinel.exists() {
            continue;
        }

        let registry = if let Some(rkey) = repo_key {
            match config.registries.iter().find(|r| r.name == rkey) {
                Some(c) => FreightRegistry::from_config(c),
                None => continue,
            }
        } else {
            match config.registries.first() {
                Some(c) => FreightRegistry::from_config(c),
                None => FreightRegistry::default_registry(),
            }
        };

        let triples = match registry.list_prebuilt_triples(name, version, channel) {
            Ok(t) => t,
            Err(_) => continue,
        };

        if !triples.contains(&triple.to_string()) {
            continue;
        }

        *any_work = true;
        print_status(
            "prebuilt",
            &format!("downloading `{name}@{version}` ({triple})…"),
        );
        match registry.download_prebuilt(name, version, channel, triple, project_dir) {
            Ok(_) => print_success(&format!("fetched `{name}@{version}` (prebuilt/{triple})")),
            Err(e) => {
                print_warning(&format!(
                    "`{name}`: prebuilt download failed ({e}), will fall back to source"
                ));
                *all_ok = false;
            }
        }
    }
}
