use std::path::Path;

use crane_core::dep_cmds::{
    locate_project, manifest_add_dep, manifest_remove_dep, regen_lock,
    fetch_git_deps, fetch_url_deps, update_git_deps, invalidate_url_dep,
    DetailedDep, GitDepAction, RegenLockOutcome,
};
use crane_core::manifest::types::{Dependency, Manifest};
use crane_core::manifest::{find_manifest_dir, load_manifest};

use crate::output::{print_error, print_status, print_success, print_warning};

// ── crane tree ───────────────────────────────────────────────────────────────

pub fn cmd_tree() {
    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    println!("{} {}", manifest.package.name, manifest.package.version);
    print_dep_tree(&manifest, &project_dir, "", true);
}

fn print_dep_tree(manifest: &Manifest, project_dir: &Path, prefix: &str, _is_root: bool) {
    let deps: Vec<(&String, &Dependency)> = {
        let mut v: Vec<_> = manifest.dependencies.iter().collect();
        v.sort_by_key(|(k, _)| k.as_str());
        v
    };

    for (i, (name, dep)) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

        match dep {
            Dependency::Simple(ver) => {
                println!("{}{}{} {} (registry)", prefix, connector, name, ver);
            }
            Dependency::Detailed(d) if d.system.is_some() => {
                if let Some(query) = &d.pkg_config {
                    println!("{}{}{} (system, pkg-config: {})", prefix, connector, name, query);
                } else {
                    println!("{}{}{} (system)", prefix, connector, name);
                }
            }
            Dependency::Detailed(d) if d.path.is_some() => {
                let rel = d.path.as_deref().unwrap_or("?");
                let dep_dir = project_dir.join(rel);
                if let Ok(m) = load_manifest(&dep_dir) {
                    println!("{}{}{} {} (path+{})", prefix, connector, name, m.package.version, rel);
                    print_dep_tree(&m, &dep_dir, &child_prefix, false);
                } else {
                    println!("{}{}{} ??? (path+{}) [not found]", prefix, connector, name, rel);
                }
            }
            Dependency::Detailed(d) if d.git.is_some() => {
                let url = d.git.as_deref().unwrap_or("?");
                println!("{}{}{} (git+{})", prefix, connector, name, url);
            }
            Dependency::Detailed(d) if d.url.is_some() => {
                let url = d.url.as_deref().unwrap_or("?");
                println!("{}{}{} (url: {})", prefix, connector, name, url);
            }
            Dependency::Detailed(d) => {
                let ver = d.version.as_deref().unwrap_or("*");
                println!("{}{}{} {} (registry)", prefix, connector, name, ver);
            }
        }
    }
}

// ── crane add ────────────────────────────────────────────────────────────────

pub fn cmd_add(
    package: &str,
    path: Option<&str>,
    git: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
    system: bool,
    dev: bool,
) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no crane.toml found"); return; }
    };

    // Parse "name@version" or just "name"
    let (dep_name, version) = if let Some(at) = package.find('@') {
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
        if !dep_dir.join("crane.toml").exists() {
            print_error(&format!("no crane.toml in {}", dep_dir.display()));
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
        if version.is_none() {
            print_warning("crane.dev registry is not yet available — this dependency cannot be fetched");
        }
        let ver = version.unwrap_or("*").to_string();
        Dependency::Simple(ver)
    };

    if let Err(e) = manifest_add_dep(&project_dir.join("crane.toml"), dep_name, &dep, dev) {
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

// ── crane remove ─────────────────────────────────────────────────────────────

pub fn cmd_remove(package: &str) {
    let project_dir = match locate_project_dir() {
        Some(d) => d,
        None => return,
    };

    match manifest_remove_dep(&project_dir.join("crane.toml"), package) {
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

// ── crane update ─────────────────────────────────────────────────────────────

pub fn cmd_update(package: Option<&str>) {
    let project_dir = match locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    if manifest.dependencies.values().any(|d| matches!(d, Dependency::Simple(_))) {
        print_warning("crane.dev registry is not yet available — version dependencies cannot be updated");
    }

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

// ── crane fetch ──────────────────────────────────────────────────────────────

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
            Dependency::Detailed(d) if d.path.is_some() && d.build_system.is_none() => {
                any_work = true;
                let rel = d.path.as_deref().unwrap();
                let dep_dir = project_dir.join(rel);
                if dep_dir.join("crane.toml").exists() {
                    print_status("ok", &format!("{name} (path+{rel})"));
                } else {
                    print_error(&format!("{name}: not found at {rel}"));
                    all_ok = false;
                }
            }
            Dependency::Detailed(d) if d.build_system.is_some() => {
                print_status("skip", &format!("{name} (foreign — built on demand)"));
            }
            Dependency::Simple(_) => {
                any_work = true;
                print_warning(&format!("{name}: crane.dev registry not yet available — skipping"));
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
        "`crane search {query}` requires crane.dev, which is not yet available"
    ));
}

pub fn cmd_info(package: &str) {
    print_warning(&format!(
        "`crane info {package}` requires crane.dev, which is not yet available"
    ));
}

pub fn cmd_login() {
    print_warning("crane.dev registry is not yet available — `crane login` is a no-op");
}

pub fn cmd_publish() {
    print_warning("crane.dev registry is not yet available — `crane publish` is a no-op");
}

pub fn cmd_yank(version: &str) {
    print_warning(&format!(
        "crane.dev registry is not yet available — `crane yank {version}` is a no-op"
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
        None => { print_error("no crane.toml found"); None }
    }
}

fn refresh_lock(project_dir: &Path) {
    match regen_lock(project_dir) {
        Ok(RegenLockOutcome::Wrote) => {}
        Ok(RegenLockOutcome::Skipped) => {
            print_warning("crane.lock not updated — run `crane fetch` after downloading dependencies");
        }
        Err(e) => {
            print_error(&format!("cannot write crane.lock: {e}"));
        }
    }
}
