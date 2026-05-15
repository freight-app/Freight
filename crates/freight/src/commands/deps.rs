use std::path::Path;

use freight_core::dep_cmds::{
    locate_project, manifest_add_dep, manifest_remove_dep, regen_lock,
    fetch_git_deps, fetch_package_deps, fetch_url_deps, update_git_deps, invalidate_url_dep,
    DetailedDep, GitDepAction, PackageDepAction, RegenLockOutcome,
};
use freight_core::manifest::types::{Dependency, Manifest};
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::registry::repos::{repo_by_name, registries_in_order};
use freight_core::toolchain::cache::GlobalConfig;

use crate::output::{print_error, print_status, print_success, print_warning};
use owo_colors::OwoColorize;

// ── freight tree ───────────────────────────────────────────────────────────────

pub fn cmd_tree() {
    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    println!(
        "{} {}",
        manifest.package.name.bold().bright_blue(),
        manifest.package.version.bright_black()
    );
    print_dep_tree(&manifest, &project_dir, "");
}

fn print_dep_tree(manifest: &Manifest, project_dir: &Path, prefix: &str) {
    let deps: Vec<(&String, &Dependency)> = {
        let mut v: Vec<_> = manifest.dependencies.iter().collect();
        v.sort_by_key(|(k, _)| k.as_str());
        v
    };

    for (i, (name, dep)) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
        let branch = format!("{prefix}{connector}").bright_black().to_string();

        match dep {
            Dependency::Simple(ver) => {
                print_package_dep(&branch, name, ver);
            }
            Dependency::Detailed(d) if d.system.is_some() => {
                print_system_dep(&branch, name, d);
            }
            Dependency::Detailed(d) if d.path.is_some() => {
                let rel = d.path.as_deref().unwrap_or("?");
                let dep_dir = project_dir.join(rel);
                if let Ok(m) = load_manifest(&dep_dir) {
                    println!(
                        "{}{} {} {}",
                        branch,
                        name.bold().bright_blue(),
                        m.package.version.bright_black(),
                        format!("(path+{rel})").yellow()
                    );
                    print_dep_tree(&m, &dep_dir, &child_prefix);
                } else {
                    println!(
                        "{}{} {} {}",
                        branch,
                        name.bold().bright_blue(),
                        "???".red().bold(),
                        format!("(path+{rel}) [not found]").yellow()
                    );
                }
            }
            Dependency::Detailed(d) if d.git.is_some() => {
                let url = d.git.as_deref().unwrap_or("?");
                println!(
                    "{}{} {}",
                    branch,
                    name.bold().bright_blue(),
                    format!("(git+{url})").yellow()
                );
            }
            Dependency::Detailed(d) if d.url.is_some() => {
                let url = d.url.as_deref().unwrap_or("?");
                println!(
                    "{}{} {}",
                    branch,
                    name.bold().bright_blue(),
                    format!("(url: {url})").yellow()
                );
            }
            Dependency::Detailed(d) => {
                let ver = d.version.as_deref().unwrap_or("*");
                print_package_dep(&branch, name, ver);
            }
        }
    }
}

fn print_package_dep(branch: &str, name: &str, version: &str) {
    println!(
        "{}{} {} {}",
        branch,
        name.bold().bright_blue(),
        version.bright_black(),
        "(package)".green()
    );
}

fn print_system_dep(branch: &str, name: &str, _dep: &DetailedDep) {
    println!(
        "{}{} {}",
        branch,
        name.bold().bright_blue(),
        "(system)".cyan()
    );
}

// ── freight add ────────────────────────────────────────────────────────────────

pub fn cmd_add(
    package: &str,
    path: Option<&str>,
    git: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
    system: bool,
    repo: Option<&str>,
    dev: bool,
) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };

    // Parse "name@version" or just "name"
    let (dep_name, pinned_version) = if let Some(at) = package.find('@') {
        (&package[..at], Some(&package[at + 1..]))
    } else {
        (package, None)
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
    } else if system {
        Dependency::Detailed(DetailedDep {
            system: Some(dep_name.to_string()),
            ..Default::default()
        })
    } else {
        // Registry-backed dependency.
        let config = {
            let mut cfg = GlobalConfig::load();
            if let Some(local) = GlobalConfig::load_local(&project_dir) {
                cfg.apply_local(local);
            }
            cfg
        };

        let (ver, repo_key) = if let Some(rname) = repo {
            // Explicit --repo: use that registry only.
            let repo_impl = match repo_by_name(rname, &config) {
                Ok(r) => r,
                Err(e) => { print_error(&e.to_string()); return; }
            };
            let key = repo_impl.repo_key().to_string();
            let v = if let Some(pinned) = pinned_version {
                pinned.to_string()
            } else {
                print_status("registry", &format!("looking up `{dep_name}` via {rname}…"));
                match repo_impl.lookup(dep_name) {
                    Ok(Some(info)) => {
                        print_status("resolved", &format!("`{dep_name}` → {}", info.latest));
                        info.latest
                    }
                    Ok(None) => {
                        print_error(&format!("`{dep_name}` not found in the {rname} registry"));
                        return;
                    }
                    Err(e) => {
                        print_warning(&format!("repo unreachable ({e}); adding with version \"*\""));
                        "*".to_string()
                    }
                }
            };
            (v, key)
        } else if let Some(pinned) = pinned_version {
            // Pinned version, no explicit repo: use the default registry.
            (pinned.to_string(), String::new())
        } else {
            // No repo, no pinned version: try each registry in order, first hit wins.
            print_status("registry", &format!("looking up `{dep_name}`…"));
            let all_repos = registries_in_order(&config);
            let mut found: Option<(String, String)> = None;
            for r in &all_repos {
                let display = if r.repo_key().is_empty() { "freight.dev" } else { r.repo_key() };
                match r.lookup(dep_name) {
                    Ok(Some(info)) => {
                        print_status("resolved", &format!("`{dep_name}` → {} (via {display})", info.latest));
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
                    print_error(&format!("`{dep_name}` not found in any configured registry"));
                    return;
                }
            }
        };

        if repo_key.is_empty() {
            Dependency::Simple(ver)
        } else {
            Dependency::Detailed(DetailedDep {
                version: Some(ver),
                repo: Some(repo_key),
                ..Default::default()
            })
        }
    };

    if let Err(e) = manifest_add_dep(&project_dir.join("freight.toml"), dep_name, &dep, dev) {
        print_error(&e.to_string());
        return;
    }

    let section = if dev { "dev-dependencies" } else { "dependencies" };
    print_success(&format!("added `{dep_name}` to [{section}]"));

    // Clone git deps immediately after adding them.
    if matches!(&dep, Dependency::Detailed(d) if d.git.is_some()) {
        print_status("fetch", &format!("cloning `{dep_name}`…"));
        match fetch_git_deps(&project_dir) {
            Ok(outcomes) => {
                for o in outcomes {
                    if o.name == dep_name {
                        match o.action {
                            GitDepAction::Cloned => print_success(&format!("cloned `{dep_name}`")),
                            GitDepAction::AlreadyPresent => print_status("ok", &format!("`{dep_name}` already present")),
                            _ => {}
                        }
                    }
                }
            }
            Err(e) => print_error(&format!("fetch failed: {e}")),
        }
    }

    refresh_lock(&project_dir);
}

/// Interactive `freight add` (no package name given).
/// TODO: freight registry interactive search TUI.
pub fn cmd_add_interactive(_repo: Option<&str>, _dev: bool) {
    print_warning(
        "interactive registry search is not yet available — \
         use `freight add <name>` to add by name or `freight search <query>` to search"
    );
}

// ── freight remove ─────────────────────────────────────────────────────────────

pub fn cmd_remove(package: &str) {
    let project_dir = match locate_project_dir() {
        Some(d) => d,
        None => return,
    };

    match manifest_remove_dep(&project_dir.join("freight.toml"), package) {
        Ok(true) => {
            print_success(&format!("removed `{package}`"));
            refresh_lock(&project_dir);
        }
        Ok(false) => {
            print_error(&format!("`{package}` not found in [dependencies] or [dev-dependencies]"));
        }
        Err(e) => print_error(&e.to_string()),
    }
}

// ── freight update ─────────────────────────────────────────────────────────────

pub fn cmd_update(package: Option<&str>) {
    let project_dir = match locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let target = package.map(|p| p.to_string());

    // Update path dep lockfile checksums.
    let path_count = manifest.dependencies.iter()
        .filter(|(name, dep)| {
            target.as_deref().map_or(true, |t| t == name.as_str())
                && matches!(dep, Dependency::Detailed(d) if d.path.is_some())
        })
        .count();

    // Pull latest commits for git deps.
    match update_git_deps(&project_dir, target.as_deref()) {
        Ok(outcomes) => {
            for o in outcomes {
                match o.action {
                    GitDepAction::Updated => print_success(&format!("updated `{}`", o.name)),
                    GitDepAction::Skipped => print_status("skip", &format!("`{}` (rev-pinned)", o.name)),
                    _ => {}
                }
            }
        }
        Err(e) => { print_error(&e.to_string()); return; }
    }

    // Re-download url deps (invalidate sentinel, then re-fetch).
    let url_count = manifest.dependencies.iter()
        .filter(|(name, dep)| {
            target.as_deref().map_or(true, |t| t == name.as_str())
                && matches!(dep, Dependency::Detailed(d) if d.url.is_some())
        })
        .count();
    if url_count > 0 {
        for (name, dep) in &manifest.dependencies {
            if target.as_deref().map_or(true, |t| t == name.as_str()) {
                if let Dependency::Detailed(d) = dep {
                    if d.url.is_some() {
                        invalidate_url_dep(&project_dir, name);
                    }
                }
            }
        }
        match fetch_url_deps(&project_dir) {
            Ok(outcomes) => {
                for (name, _) in outcomes {
                    print_success(&format!("re-fetched `{name}`"));
                }
            }
            Err(e) => { print_error(&e.to_string()); return; }
        }
    }

    if path_count == 0
        && !manifest.dependencies.values().any(|d| matches!(d, Dependency::Detailed(dd) if dd.git.is_some()))
        && url_count == 0
    {
        if let Some(pkg) = package {
            print_error(&format!("`{pkg}` not found in [dependencies]"));
        } else {
            println!("no dependencies to update");
        }
        return;
    }

    refresh_lock(&project_dir);

    if path_count > 0 {
        print_success(&format!("refreshed lockfile for {path_count} path dep(s)"));
    }
}

// ── freight fetch ──────────────────────────────────────────────────────────────

pub fn cmd_fetch() {
    let project_dir = match locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let mut all_ok = true;
    let mut any_work = false;

    // Verify path deps.
    for (name, dep) in &manifest.dependencies {
        match dep {
            Dependency::Detailed(d) if d.system.is_some() => {
                print_status("skip", &format!("{name} (system)"));
            }
            Dependency::Detailed(d) if d.path.is_some() && d.backend.is_none() => {
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
            Dependency::Detailed(d) if d.backend.is_some() => {
                print_status("skip", &format!("{name} (foreign — built on demand)"));
            }
            _ => {}
        }
    }

    // Clone git deps.
    match fetch_git_deps(&project_dir) {
        Ok(outcomes) => {
            for o in outcomes {
                any_work = true;
                match o.action {
                    GitDepAction::Cloned        => print_success(&format!("cloned `{}`", o.name)),
                    GitDepAction::AlreadyPresent => print_status("ok",   &format!("{} (git, up to date)", o.name)),
                    _ => {}
                }
            }
        }
        Err(e) => { print_error(&e.to_string()); all_ok = false; }
    }

    // Download url deps.
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
        Err(e) => { print_error(&e.to_string()); all_ok = false; }
    }

    // Check version package deps against system (pkg-config) and local cache.
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
        Err(e) => { print_error(&e.to_string()); all_ok = false; }
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

// ── Registry stubs ───────────────────────────────────────────────────────────

pub fn cmd_search(query: &str) {
    print_warning(&format!(
        "`freight search {query}` requires freight.dev, which is not yet available"
    ));
}

pub fn cmd_info(package: Option<&str>) {
    if let Some(package) = package {
        print_warning(&format!(
            "`freight info {package}` requires freight.dev, which is not yet available"
        ));
        return;
    }

    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    print_current_package_info(&project_dir, &manifest);
}

fn print_current_package_info(project_dir: &Path, manifest: &Manifest) {
    println!("{} {}", manifest.package.name, manifest.package.version);

    print_optional_field("description", non_empty(&manifest.package.description));
    print_optional_list("authors", &manifest.package.authors);
    print_optional_field("license", non_empty(&manifest.package.license));
    print_optional_field("repository", manifest.package.repository.as_deref());
    print_optional_field("readme", manifest.package.readme.as_deref());
    print_optional_list("keywords", &manifest.package.keywords);
    print_optional_field("supports", manifest.package.supports.as_deref());
    print_optional_list("provides", &manifest.package.provides);
    print_status("manifest", &project_dir.join("freight.toml").display().to_string());

    if !manifest.language.is_empty() {
        let mut languages: Vec<_> = manifest.language.keys().map(String::as_str).collect();
        languages.sort_unstable();
        print_status("languages", &languages.join(", "));
    }

    if let Some(lib) = &manifest.lib {
        print_status("library", &format!("{:?}", lib.lib_type).to_lowercase());
    }

    if !manifest.bins.is_empty() {
        let mut bins: Vec<_> = manifest.bins.iter().map(|bin| bin.name.as_str()).collect();
        bins.sort_unstable();
        print_status("binaries", &bins.join(", "));
    }

    print_dependency_summary("dependencies", &manifest.dependencies);
    print_dependency_summary("dev-deps", &manifest.dev_dependencies);

    if !manifest.features.is_empty() {
        let mut features: Vec<_> = manifest.features.keys().map(String::as_str).collect();
        features.sort_unstable();
        print_status("features", &features.join(", "));
    }
}

fn non_empty(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}

fn print_optional_field(label: &str, value: Option<&str>) {
    if let Some(value) = value {
        if !value.is_empty() {
            print_status(label, value);
        }
    }
}

fn print_optional_list(label: &str, values: &[String]) {
    if !values.is_empty() {
        print_status(label, &values.join(", "));
    }
}

fn print_dependency_summary(label: &str, deps: &std::collections::HashMap<String, Dependency>) {
    if deps.is_empty() {
        return;
    }

    let mut names: Vec<_> = deps.keys().map(String::as_str).collect();
    names.sort_unstable();
    print_status(label, &names.join(", "));
}

pub fn cmd_login() {
    print_warning("freight.dev registry is not yet available — `freight login` is a no-op");
}

pub fn cmd_publish() {
    print_warning("freight.dev registry is not yet available — `freight publish` is a no-op");
}

pub fn cmd_yank(version: &str) {
    print_warning(&format!(
        "freight.dev registry is not yet available — `freight yank {version}` is a no-op"
    ));
}

// ── Local helpers ────────────────────────────────────────────────────────────

fn locate_project_dir() -> Option<std::path::PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return None; }
    };
    match find_manifest_dir(&cwd) {
        Some(d) => Some(d),
        None => { print_error("no freight.toml found"); None }
    }
}

fn refresh_lock(project_dir: &Path) {
    match regen_lock(project_dir) {
        Ok(RegenLockOutcome::Wrote) => {}
        Ok(RegenLockOutcome::Skipped) => {
            print_warning("freight.lock not updated — run `freight fetch` after downloading dependencies");
        }
        Err(e) => {
            print_error(&format!("cannot write freight.lock: {e}"));
        }
    }
}
