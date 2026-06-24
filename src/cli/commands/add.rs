use freight::adaptors::detect_build_system;
use freight::dep_cmds::{
    fetch_git_deps, manifest_add_dep, manifest_add_foreign_build, DetailedDep, GitDepAction,
};
use freight::manifest::find_manifest_dir;
use freight::manifest::types::Dependency;
use freight::registry::repos::{registries_in_order, repo_by_name};
use freight::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success, print_warning};

#[derive(clap::Args)]
pub struct Args {
    /// Package name, optionally with version: `name` or `name@1.0`.
    /// Pass a git URL (https://…) or archive URL (https://….tar.gz) to add
    /// without `--git`/`--url` flags. Omit entirely for an interactive prompt.
    #[arg(value_name = "NAME[@VERSION]|URL")]
    pub package: Option<String>,
    /// Add as a path dependency pointing to a local freight project
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,
    /// Add as a URL dependency (git repo or HTTP archive)
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// Git branch to track (makes --url a git dep)
    #[arg(long)]
    pub branch: Option<String>,
    /// Git tag to check out (makes --url a git dep)
    #[arg(long)]
    pub tag: Option<String>,
    /// Exact commit SHA to pin (makes --url a git dep)
    #[arg(long)]
    pub rev: Option<String>,
    /// Registry to search for the package (default: first configured registry).
    #[arg(long, short = 'r', value_name = "REGISTRY")]
    pub registry: Option<String>,
    /// Add to [dev-dependencies] instead of [dependencies]
    #[arg(long)]
    pub dev: bool,
}

impl Args {
    pub fn run(self) {
        if let Some(package) = self.package {
            cmd_add(
                &package,
                self.path.as_deref(),
                self.url.as_deref(),
                self.branch.as_deref(),
                self.tag.as_deref(),
                self.rev.as_deref(),
                self.registry.as_deref(),
                self.dev,
            );
        } else {
            cmd_add_interactive(self.registry.as_deref(), self.dev);
        }
    }
}

fn looks_like_url(s: &str) -> bool {
    s.starts_with("https://")
        || s.starts_with("http://")
        || s.starts_with("ssh://")
        || s.starts_with("git@")
}

fn url_is_archive(url: &str) -> bool {
    if url.starts_with("ssh://") || url.starts_with("git@") {
        return false;
    }
    let path = url
        .split('?')
        .next()
        .unwrap_or(url)
        .split('#')
        .next()
        .unwrap_or(url);
    const ARCHIVE_EXTS: &[&str] = &[
        ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst", ".zip", ".7z", ".whl", ".gem",
        ".hpp", ".h", ".c",
    ];
    ARCHIVE_EXTS.iter().any(|ext| path.ends_with(ext))
}

fn url_dep_name(url: &str) -> String {
    let path_part = if url.starts_with("git@") {
        url.splitn(2, ':').nth(1).unwrap_or(url)
    } else {
        url.split('?')
            .next()
            .unwrap_or(url)
            .split('#')
            .next()
            .unwrap_or(url)
    };
    let last = path_part
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("dep");
    const STRIP_SUFFIXES: &[&str] = &[
        ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst", ".zip", ".7z", ".git", ".hpp", ".h",
        ".c",
    ];
    let mut name = last;
    for suffix in STRIP_SUFFIXES {
        if let Some(s) = name.strip_suffix(suffix) {
            name = s;
            break;
        }
    }
    name.to_string()
}

pub fn cmd_add(
    package: &str,
    path: Option<&str>,
    url: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
    repo: Option<&str>,
    dev: bool,
) {
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

    // Auto-detect git / URL archive when the package argument is a raw URL.
    if looks_like_url(package) && path.is_none() && url.is_none() {
        let dep_name = url_dep_name(package);
        let dep = if url_is_archive(package) {
            print_status("detected", &format!("URL archive dep → `{dep_name}`"));
            Dependency::Detailed(DetailedDep {
                url: Some(package.to_string()),
                ..Default::default()
            })
        } else {
            print_status("detected", &format!("git dep → `{dep_name}`"));
            Dependency::Detailed(DetailedDep {
                url: Some(package.to_string()),
                branch: branch.map(str::to_string),
                tag: tag.map(str::to_string),
                rev: rev.map(str::to_string),
                ..Default::default()
            })
        };
        if let Err(e) = manifest_add_dep(&project_dir.join("freight.toml"), &dep_name, &dep, dev) {
            print_error(&e.to_string());
            return;
        }
        let section = if dev {
            "dev-dependencies"
        } else {
            "dependencies"
        };
        print_success(&format!("added `{dep_name}` to [{section}]"));
        if matches!(&dep, Dependency::Detailed(d) if d.is_git()) {
            print_status("fetch", &format!("cloning `{dep_name}`…"));
            match fetch_git_deps(&project_dir) {
                Ok(outcomes) => {
                    for o in outcomes {
                        if o.name == dep_name {
                            if matches!(o.action, GitDepAction::Cloned) {
                                print_success(&format!("cloned `{dep_name}`"));
                            }
                        }
                    }
                }
                Err(e) => print_error(&format!("fetch failed: {e}")),
            }
            // A cloned dep without a freight.toml is a foreign project: mark it
            // `external` (freight won't build it directly) and, if a build system
            // is recognised, wire it to the matching build plugin.
            adopt_foreign_dep(&project_dir, &dep_name, package, branch, tag, rev, dev);
        }
        super::common::refresh_lock(&project_dir);
        return;
    }

    // Parse "channel/name@version", "channel/name", "name@version", or just "name".
    let (channel_arg, name_and_ver) = if let Some(slash) = package.find('/') {
        (Some(&package[..slash]), &package[slash + 1..])
    } else {
        (None, package)
    };
    let (dep_name, pinned_version) = if let Some(at) = name_and_ver.find('@') {
        (&name_and_ver[..at], Some(&name_and_ver[at + 1..]))
    } else {
        (name_and_ver, None)
    };

    if dep_name.is_empty() {
        print_error("dependency name cannot be empty");
        return;
    }

    let dep = if let Some(rel_path) = path {
        let dep_dir = project_dir.join(rel_path);
        if !dep_dir.exists() {
            print_error(&format!("path dependency not found: {}", dep_dir.display()));
            return;
        }
        if !dep_dir.join("freight.toml").exists() {
            print_error(&format!("no freight.toml in {}", dep_dir.display()));
            return;
        }
        Dependency::Detailed(DetailedDep {
            path: Some(rel_path.to_string()),
            ..Default::default()
        })
    } else if let Some(u) = url {
        Dependency::Detailed(DetailedDep {
            url: Some(u.to_string()),
            branch: branch.map(str::to_string),
            tag: tag.map(str::to_string),
            rev: rev.map(str::to_string),
            ..Default::default()
        })
    } else {
        let config = {
            let mut cfg = GlobalConfig::load();
            if let Some(local) = GlobalConfig::load_local(&project_dir) {
                cfg.apply_local(local);
            }
            cfg
        };

        let (ver, repo_key) = if let Some(rname) = repo {
            let repo_impl = match repo_by_name(rname, &config) {
                Ok(r) => r,
                Err(e) => {
                    print_error(&e.to_string());
                    return;
                }
            };
            let key = repo_impl.repo_key().to_string();
            let v = if let Some(pinned) = pinned_version {
                pinned.to_string()
            } else {
                print_status("registry", &format!("looking up `{dep_name}` via {rname}…"));
                match repo_impl.lookup(dep_name, channel_arg) {
                    Ok(Some(info)) => {
                        print_status("resolved", &format!("`{dep_name}` → {}", info.latest));
                        info.latest
                    }
                    Ok(None) => {
                        print_error(&format!("`{dep_name}` not found in the {rname} registry"));
                        return;
                    }
                    Err(e) => {
                        // Registry unreachable — fall back to the installed version
                        // from pkg-config (a bare `*` is not allowed).
                        let pc = freight::adaptors::pkg_config_version(dep_name);
                        if pc.is_empty() {
                            print_error(&format!(
                                "repo unreachable ({e}) and pkg-config doesn't know `{dep_name}`; \
                                 specify a version: `freight add {dep_name}@<version>`"
                            ));
                            return;
                        }
                        print_warning(&format!(
                            "repo unreachable ({e}); pinning pkg-config's installed version {pc}"
                        ));
                        pc
                    }
                }
            };
            (v, key)
        } else if let Some(pinned) = pinned_version {
            (pinned.to_string(), String::new())
        } else {
            print_status("registry", &format!("looking up `{dep_name}`…"));
            let all_repos = registries_in_order(&config);
            let mut found: Option<(String, String)> = None;
            for r in &all_repos {
                let display = if r.repo_key().is_empty() {
                    "freight.dev"
                } else {
                    r.repo_key()
                };
                match r.lookup(dep_name, channel_arg) {
                    Ok(Some(info)) => {
                        print_status(
                            "resolved",
                            &format!("`{dep_name}` → {} (via {display})", info.latest),
                        );
                        found = Some((info.latest, r.repo_key().to_string()));
                        break;
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        print_warning(&format!("{display} unreachable ({e}), trying next…"));
                        continue;
                    }
                }
            }
            match found {
                Some(pair) => pair,
                None => {
                    print_error(&format!(
                        "`{dep_name}` not found in any configured registry"
                    ));
                    return;
                }
            }
        };

        if repo_key.is_empty() && channel_arg.is_none() {
            Dependency::Simple(ver)
        } else {
            Dependency::Detailed(DetailedDep {
                version: Some(ver),
                registry: if repo_key.is_empty() {
                    None
                } else {
                    Some(repo_key)
                },
                channel: channel_arg.map(str::to_string),
                ..Default::default()
            })
        }
    };

    if let Err(e) = manifest_add_dep(&project_dir.join("freight.toml"), dep_name, &dep, dev) {
        print_error(&e.to_string());
        return;
    }

    let section = if dev {
        "dev-dependencies"
    } else {
        "dependencies"
    };
    print_success(&format!("added `{dep_name}` to [{section}]"));

    if matches!(&dep, Dependency::Detailed(d) if d.is_git()) {
        print_status("fetch", &format!("cloning `{dep_name}`…"));
        match fetch_git_deps(&project_dir) {
            Ok(outcomes) => {
                for o in outcomes {
                    if o.name == dep_name {
                        match o.action {
                            GitDepAction::Cloned => print_success(&format!("cloned `{dep_name}`")),
                            GitDepAction::AlreadyPresent => {
                                print_status("ok", &format!("`{dep_name}` already present"))
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(e) => print_error(&format!("fetch failed: {e}")),
        }
    }

    super::common::refresh_lock(&project_dir);
}

/// Inspect a just-cloned git dependency. If it ships a `freight.toml` it is a
/// native freight package and nothing changes. Otherwise it is foreign: re-write
/// the dep with `external = true` and, when a build system is recognised, wire it
/// to the matching build plugin (`[cmake] build = ["dep"]`, etc.).
fn adopt_foreign_dep(
    project_dir: &std::path::Path,
    dep_name: &str,
    url: &str,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
    dev: bool,
) {
    let dep_dir = project_dir.join(".pkgs").join(dep_name);
    if !dep_dir.is_dir() || dep_dir.join("freight.toml").is_file() {
        return; // not fetched, or a native freight package
    }

    let manifest = project_dir.join("freight.toml");
    let dep = Dependency::Detailed(DetailedDep {
        url: Some(url.to_string()),
        branch: branch.map(str::to_string),
        tag: tag.map(str::to_string),
        rev: rev.map(str::to_string),
        external: true,
        ..Default::default()
    });
    if let Err(e) = manifest_add_dep(&manifest, dep_name, &dep, dev) {
        print_error(&e.to_string());
        return;
    }
    print_status("foreign", &format!("`{dep_name}` is not a freight package → external = true"));

    match detect_build_system(&dep_dir) {
        Some(backend) => {
            if let Err(e) = manifest_add_foreign_build(&manifest, dep_name, &backend) {
                print_error(&e.to_string());
                return;
            }
            print_success(&format!(
                "wired `{dep_name}` to the `{backend}` plugin ([{backend}] build)"
            ));
        }
        None => print_warning(&format!(
            "no build system detected for `{dep_name}`; add headers via `include = [..]` if it is header-only"
        )),
    }
}

/// Interactive `freight add` (no package name given) — opens the package browser.
/// Packages are added to freight.toml directly from within the browser; the
/// browser stays open until the user presses Esc.
pub fn cmd_add_interactive(repo: Option<&str>, dev: bool) {
    if let Err(e) = crate::tui::run_package_browser(repo, dev) {
        print_warning(&format!("TUI error: {e}"));
    }
}
