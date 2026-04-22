use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value, value};

use crate::build::deps::resolve_dep_graph;
use crate::error::CraneError;
use crate::lock::LockFile;
use crate::manifest::types::{Dependency, DetailedDep, Manifest};
use crate::manifest::{find_manifest_dir, load_manifest};
use crate::output::{print_error, print_status, print_success, print_warning};
use crate::toolchain::{detect_all_cached, load_templates, templates_dir};

// ── crane tree ────────────────────────────────────────────────────────────────

pub fn cmd_tree() {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no crane.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
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
                println!("{}{}{}  (system)", prefix, connector, name);
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
            Dependency::Detailed(d) => {
                let ver = d.version.as_deref().unwrap_or("*");
                println!("{}{}{} {} (registry)", prefix, connector, name, ver);
            }
        }
    }
}

// ── crane add ─────────────────────────────────────────────────────────────────

pub fn cmd_add(
    package: &str,
    path: Option<&str>,
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

    // Determine what kind of dep to add
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
    } else if system {
        Dependency::Detailed(DetailedDep {
            system: Some(dep_name.to_string()),
            ..Default::default()
        })
    } else {
        // Version dep (registry — not yet fetchable)
        let ver = version.unwrap_or("*").to_string();
        Dependency::Simple(ver)
    };

    // Warn early for registry deps
    if matches!(&dep, Dependency::Simple(_)) || matches!(&dep, Dependency::Detailed(d) if d.git.is_some()) {
        print_warning("crane.dev registry is not yet available — this dependency cannot be fetched");
    }

    if let Err(e) = manifest_add_dep(&project_dir.join("crane.toml"), dep_name, &dep, dev) {
        print_error(&e.to_string());
        return;
    }

    let section = if dev { "dev-dependencies" } else { "dependencies" };
    print_success(&format!("added `{dep_name}` to [{section}]"));

    // Regenerate lockfile
    regen_lock(&project_dir);
}

// ── crane remove ──────────────────────────────────────────────────────────────

pub fn cmd_remove(package: &str) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no crane.toml found"); return; }
    };

    match manifest_remove_dep(&project_dir.join("crane.toml"), package) {
        Ok(true) => {
            print_success(&format!("removed `{package}`"));
            regen_lock(&project_dir);
        }
        Ok(false) => {
            print_error(&format!("`{package}` not found in [dependencies] or [dev-dependencies]"));
        }
        Err(e) => print_error(&e.to_string()),
    }
}

// ── crane update ──────────────────────────────────────────────────────────────

pub fn cmd_update(package: Option<&str>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no crane.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    // Check whether any version/git deps exist — those need the registry
    let has_registry = manifest.dependencies.values().any(|d| matches!(d, Dependency::Simple(_)));
    let has_git = manifest.dependencies.values()
        .any(|d| matches!(d, Dependency::Detailed(dd) if dd.git.is_some()));

    if has_registry || has_git {
        print_warning("crane.dev registry is not yet available — version/git dependencies cannot be updated");
    }

    // For path deps: just regenerate the lockfile to refresh checksums
    let target = package.map(|p| p.to_string());
    let path_deps: Vec<&str> = manifest.dependencies.iter()
        .filter(|(name, dep)| {
            target.as_deref().map_or(true, |t| t == name.as_str())
                && matches!(dep, Dependency::Detailed(d) if d.path.is_some())
        })
        .map(|(name, _)| name.as_str())
        .collect();

    if path_deps.is_empty() && !has_registry && !has_git {
        if let Some(pkg) = package {
            print_error(&format!("`{pkg}` not found in [dependencies]"));
        } else {
            println!("no dependencies to update");
        }
        return;
    }

    regen_lock(&project_dir);

    if !path_deps.is_empty() {
        print_success(&format!("updated lockfile for {} path dep(s)", path_deps.len()));
    }
}

// ── crane fetch ───────────────────────────────────────────────────────────────

pub fn cmd_fetch() {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no crane.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let mut all_ok = true;
    let mut any_path = false;
    let mut any_registry = false;

    for (name, dep) in &manifest.dependencies {
        match dep {
            Dependency::Detailed(d) if d.system.is_some() => {
                // System deps are managed by the OS package manager; nothing to fetch
                print_status("skip", &format!("{name} (system)"));
            }
            Dependency::Detailed(d) if d.path.is_some() => {
                any_path = true;
                let rel = d.path.as_deref().unwrap();
                let dep_dir = project_dir.join(rel);
                if dep_dir.join("crane.toml").exists() {
                    print_status("ok", &format!("{name} (path+{rel})"));
                } else {
                    print_error(&format!("{name}: not found at {rel}"));
                    all_ok = false;
                }
            }
            Dependency::Detailed(d) if d.git.is_some() => {
                any_registry = true;
                print_warning(&format!("{name}: git dependencies are not yet supported — skipping"));
            }
            _ => {
                any_registry = true;
                print_warning(&format!("{name}: crane.dev registry not yet available — skipping"));
            }
        }
    }

    if any_registry {
        println!();
        print_warning("version and git dependencies require crane.dev, which is not yet available");
    }

    if !any_path && !any_registry {
        println!("no dependencies to fetch");
        return;
    }

    if all_ok && any_path {
        println!();
        print_success("all path dependencies present");
    }
}

// ── crane search ──────────────────────────────────────────────────────────────

pub fn cmd_search(query: &str) {
    print_warning(&format!(
        "`crane search {query}` requires crane.dev, which is not yet available"
    ));
}

// ── crane info ────────────────────────────────────────────────────────────────

pub fn cmd_info(package: &str) {
    print_warning(&format!(
        "`crane info {package}` requires crane.dev, which is not yet available"
    ));
}

// ── crane login ───────────────────────────────────────────────────────────────

pub fn cmd_login() {
    print_warning("crane.dev registry is not yet available — `crane login` is a no-op");
}

// ── crane publish ─────────────────────────────────────────────────────────────

pub fn cmd_publish() {
    print_warning("crane.dev registry is not yet available — `crane publish` is a no-op");
}

// ── crane yank ────────────────────────────────────────────────────────────────

pub fn cmd_yank(version: &str) {
    print_warning(&format!(
        "crane.dev registry is not yet available — `crane yank {version}` is a no-op"
    ));
}

// ── Manifest mutation helpers ─────────────────────────────────────────────────

fn manifest_add_dep(
    manifest_path: &Path,
    name: &str,
    dep: &Dependency,
    dev: bool,
) -> Result<(), CraneError> {
    let src = std::fs::read_to_string(manifest_path)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| CraneError::ManifestParse(e.to_string()))?;

    let section = if dev { "dev-dependencies" } else { "dependencies" };

    // Ensure the section exists
    if !doc.contains_key(section) {
        doc[section] = Item::Table(Table::new());
    }

    let table = doc[section]
        .as_table_mut()
        .ok_or_else(|| CraneError::ManifestParse(format!("[{section}] is not a table")))?;

    match dep {
        Dependency::Simple(ver) => {
            table[name] = value(ver.as_str());
        }
        Dependency::Detailed(d) => {
            let mut inline = toml_edit::InlineTable::new();
            if let Some(p) = &d.path {
                inline.insert("path", Value::from(p.as_str()));
            }
            if let Some(s) = &d.system {
                inline.insert("system", Value::from(s.as_str()));
            }
            if let Some(g) = &d.git {
                inline.insert("git", Value::from(g.as_str()));
            }
            if let Some(v) = &d.version {
                inline.insert("version", Value::from(v.as_str()));
            }
            table[name] = Item::Value(Value::InlineTable(inline));
        }
    }

    std::fs::write(manifest_path, doc.to_string())?;
    Ok(())
}

fn manifest_remove_dep(manifest_path: &Path, name: &str) -> Result<bool, CraneError> {
    let src = std::fs::read_to_string(manifest_path)?;
    let mut doc: DocumentMut = src
        .parse()
        .map_err(|e: toml_edit::TomlError| CraneError::ManifestParse(e.to_string()))?;

    let mut removed = false;
    for &section in &["dependencies", "dev-dependencies"] {
        let is_empty = {
            let table = match doc.get_mut(section).and_then(|i| i.as_table_mut()) {
                Some(t) => t,
                None => continue,
            };
            if table.remove(name).is_some() {
                removed = true;
            }
            table.is_empty()
        };
        // Drop the section header entirely when it becomes empty
        if is_empty && removed {
            doc.remove(section);
        }
    }

    if removed {
        std::fs::write(manifest_path, doc.to_string())?;
    }
    Ok(removed)
}

// ── Lock regeneration ─────────────────────────────────────────────────────────

fn regen_lock(project_dir: &Path) {
    let manifest = match load_manifest(project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&format!("cannot reload manifest: {e}")); return; }
    };

    let tdir = match templates_dir() {
        Some(d) => d,
        None => { print_error("compiler-templates directory not found"); return; }
    };
    let templates = load_templates(&tdir);
    let _ = detect_all_cached(&templates); // warm the version cache as a side effect

    let resolved = match resolve_dep_graph(project_dir, &manifest, false) {
        Ok(r) => r,
        Err(_) => {
            // If deps are not yet fetched (registry / git), the lock cannot be
            // fully resolved. Leave any existing crane.lock in place.
            print_warning("crane.lock not updated — run `crane fetch` after downloading dependencies");
            return;
        }
    };

    let lock = LockFile::generate(project_dir, &manifest, &resolved);
    if let Err(e) = lock.save(project_dir) {
        print_error(&format!("cannot write crane.lock: {e}"));
    }
}
