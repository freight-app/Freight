use freight::dep_cmds::{fetch_git_deps, manifest_add_dep, DetailedDep, GitDepAction};
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
    /// Add as a git dependency (URL)
    #[arg(long, value_name = "URL")]
    pub git: Option<String>,
    /// Git branch to track (requires --git)
    #[arg(long)]
    pub branch: Option<String>,
    /// Git tag to check out (requires --git)
    #[arg(long)]
    pub tag: Option<String>,
    /// Exact commit SHA to pin (requires --git)
    #[arg(long)]
    pub rev: Option<String>,
    /// Package repository to use (default: freight registry).
    #[arg(long, value_name = "REPO")]
    pub repo: Option<String>,
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
                self.git.as_deref(),
                self.branch.as_deref(),
                self.tag.as_deref(),
                self.rev.as_deref(),
                self.repo.as_deref(),
                self.dev,
            );
        } else {
            cmd_add_interactive(self.repo.as_deref(), self.dev);
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
    git: Option<&str>,
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
    if looks_like_url(package) && path.is_none() && git.is_none() {
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
                git: Some(package.to_string()),
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
        if matches!(&dep, Dependency::Detailed(d) if d.git.is_some()) {
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
    } else if let Some(url) = git {
        Dependency::Detailed(DetailedDep {
            git: Some(url.to_string()),
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
                        print_warning(&format!(
                            "repo unreachable ({e}); adding with version \"*\""
                        ));
                        "*".to_string()
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
                repo: if repo_key.is_empty() {
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

    if matches!(&dep, Dependency::Detailed(d) if d.git.is_some()) {
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

/// Interactive `freight add` (no package name given) — opens the package browser.
/// Packages are added to freight.toml directly from within the browser; the
/// browser stays open until the user presses Esc.
pub fn cmd_add_interactive(repo: Option<&str>, dev: bool) {
    if let Err(e) = crate::tui::run_package_browser(repo, dev) {
        print_warning(&format!("TUI error: {e}"));
    }
}
