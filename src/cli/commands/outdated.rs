use freight::manifest::types::Dependency;
use freight::manifest::{find_manifest_dir, load_manifest};
use freight::registry::repos::{registries_in_order, repo_by_name};
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_warning};
use owo_colors::OwoColorize;

#[derive(clap::Args)]
pub struct Args {
    /// Registry to query (default: all configured registries in order)
    #[arg(long, short = 'r', value_name = "REGISTRY")]
    pub registry: Option<String>,
}

impl Args {
    pub fn run(self) {
        cmd_outdated(self.registry.as_deref());
    }
}

fn cmd_outdated(repo: Option<&str>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => {
            print_error("no freight.toml found");
            return;
        }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };

    struct RegistryDep {
        name: String,
        current: String,
        channel: Option<String>,
        repo_key: Option<String>,
    }

    let mut registry_deps: Vec<RegistryDep> = Vec::new();
    for (name, dep) in &manifest.dependencies {
        match dep {
            Dependency::Simple(ver) => {
                registry_deps.push(RegistryDep {
                    name: name.clone(),
                    current: ver.clone(),
                    channel: None,
                    repo_key: None,
                });
            }
            Dependency::Detailed(d)
                if d.version.is_some()
                    && d.path.is_none()
                    && !d.is_git()
                    && d.url.is_none()
                    && !freight::manifest::types::is_platform_dep(name) =>
            {
                let ver = d.version.as_deref().unwrap();
                if freight::manifest::types::is_unpinned_version(ver) {
                    continue;
                }
                registry_deps.push(RegistryDep {
                    name: name.clone(),
                    current: ver.to_string(),
                    channel: d.channel.clone(),
                    repo_key: d.registry.clone(),
                });
            }
            _ => {}
        }
    }

    if registry_deps.is_empty() {
        println!("no registry dependencies to check");
        return;
    }

    struct OutdatedRow {
        name: String,
        current: String,
        latest: String,
        outdated: bool,
    }

    let mut rows: Vec<OutdatedRow> = Vec::new();
    let mut any_error = false;

    for dep in &registry_deps {
        let repos: Vec<Box<dyn freight::registry::PackageRepo>> = if let Some(rname) = repo {
            match repo_by_name(rname, &config) {
                Ok(r) => vec![r],
                Err(e) => {
                    print_error(&e.to_string());
                    return;
                }
            }
        } else if let Some(rkey) = &dep.repo_key {
            match repo_by_name(rkey, &config) {
                Ok(r) => vec![r],
                Err(e) => {
                    print_error(&e.to_string());
                    return;
                }
            }
        } else {
            registries_in_order(&config)
        };

        let channel = dep.channel.as_deref();
        let mut found = false;
        for r in &repos {
            match r.lookup(&dep.name, channel) {
                Ok(Some(info)) => {
                    let outdated = is_outdated(&dep.current, &info.latest);
                    rows.push(OutdatedRow {
                        name: dep.name.clone(),
                        current: dep.current.clone(),
                        latest: info.latest,
                        outdated,
                    });
                    found = true;
                    break;
                }
                Ok(None) => continue,
                Err(e) => {
                    print_warning(&format!("`{}`: registry unreachable ({})", dep.name, e));
                    any_error = true;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            print_warning(&format!(
                "`{}`: not found in any configured registry",
                dep.name
            ));
            any_error = true;
        }
    }

    if rows.is_empty() {
        if !any_error {
            println!("no registry dependencies found");
        }
        return;
    }

    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let current_w = rows
        .iter()
        .map(|r| r.current.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let latest_w = rows
        .iter()
        .map(|r| r.latest.len())
        .max()
        .unwrap_or(6)
        .max(6);

    println!(
        "{:<name_w$}  {:<current_w$}  {:<latest_w$}  {}",
        "name".bold(),
        "current".bold(),
        "latest".bold(),
        "status".bold(),
    );
    println!(
        "{}",
        "─"
            .repeat(name_w + current_w + latest_w + 14)
            .bright_black()
    );

    let mut any_outdated = false;
    for row in &rows {
        let (latest_col, status_col) = if row.outdated {
            any_outdated = true;
            (
                row.latest.yellow().to_string(),
                "outdated".yellow().to_string(),
            )
        } else {
            (
                row.latest.green().to_string(),
                "up to date".green().to_string(),
            )
        };
        println!(
            "{:<name_w$}  {:<current_w$}  {:<latest_w$}  {}",
            row.name.bright_blue(),
            row.current.bright_black(),
            latest_col,
            status_col,
        );
    }

    if any_outdated {
        println!();
        println!(
            "run {} to upgrade outdated dependencies",
            "`freight add <name>@<version>`".bright_blue()
        );
    } else {
        println!();
        println!("{}", "all dependencies are up to date".green());
    }

    if any_error {
        std::process::exit(1);
    }
}

fn is_outdated(current: &str, latest: &str) -> bool {
    use semver::Version;
    let current_clean = current.trim_start_matches(|c: char| matches!(c, '=' | '^' | '~' | ' '));
    match (Version::parse(current_clean), Version::parse(latest)) {
        (Ok(cur), Ok(lat)) => lat > cur,
        _ => latest != current,
    }
}
