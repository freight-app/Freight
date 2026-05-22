use std::collections::HashSet;
use std::path::{Path, PathBuf};

use freight_core::dep_cmds::{
    locate_project, manifest_add_dep, manifest_remove_dep, regen_lock,
    fetch_git_deps, fetch_package_deps, fetch_registry_deps, fetch_url_deps,
    update_git_deps, invalidate_url_dep,
    DetailedDep, GitDepAction, PackageDepAction, RegistryDepAction, RegenLockOutcome,
};
use freight_core::manifest::types::{Dependency, Manifest};
use freight_core::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use freight_core::registry::freight_registry::FreightRegistry;
use freight_core::registry::repos::{repo_by_name, registries_in_order};
use freight_core::registry::{host_triple, DEFAULT_REGISTRY_URL};
use freight_core::toolchain::cache::{freight_home, GlobalConfig};

use crate::output::{
    print_error, print_status, print_success, print_warning,
    GraphEdge, GraphFormat, render_mermaid_graph, render_dot_graph,
};
use owo_colors::OwoColorize;

// ── freight tree ───────────────────────────────────────────────────────────────

pub fn cmd_tree() {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };

    if let Some(ws) = load_workspace_manifest(&cwd) {
        for (i, member) in ws.members.iter().enumerate() {
            if i > 0 { println!(); }
            let member_dir = cwd.join(member);
            match load_manifest(&member_dir) {
                Ok(manifest) => {
                    println!(
                        "{} {} {}",
                        manifest.package.name.bold().bright_blue(),
                        manifest.package.version.bright_black(),
                        format!("({})", member).bright_black()
                    );
                    print_dep_tree(&manifest, &member_dir, "");
                }
                Err(e) => print_error(&format!("{member}: {e}")),
            }
        }
        return;
    }

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

// ── freight includes ──────────────────────────────────────────────────────────

pub fn cmd_includes(show_system: bool, format: &str) {
    let fmt = GraphFormat::parse(format);

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };

    if let Some(ws) = load_workspace_manifest(&cwd) {
        for (i, member) in ws.members.iter().enumerate() {
            if i > 0 { println!(); }
            let member_dir = cwd.join(member);
            match load_manifest(&member_dir) {
                Ok(manifest) => {
                    use owo_colors::OwoColorize;
                    println!("  {} {}", "member".bright_black(), member.bold());
                    print_includes_for(&member_dir, &manifest, show_system, fmt);
                }
                Err(e) => print_error(&format!("{member}: {e}")),
            }
        }
        return;
    }

    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => { print_error(&e.to_string()); return; }
    };
    print_includes_for(&project_dir, &manifest, show_system, fmt);
}

fn print_includes_for(project_dir: &std::path::Path, manifest: &Manifest, show_system: bool, fmt: GraphFormat) {
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let inc = project_dir.join("inc");
    let src = project_dir.join("src");
    if inc.is_dir() { include_dirs.push(inc); }
    if src.is_dir() { include_dirs.push(src.clone()); }
    for d in &manifest.compiler.includes {
        let p = project_dir.join(d);
        if p.is_dir() { include_dirs.push(p); }
    }

    let source_files = collect_source_files(&src);
    if source_files.is_empty() {
        println!("no source files found in {}", src.display());
        return;
    }

    if fmt != GraphFormat::Text {
        // Collect all edges first, then render.
        let mut all_edges: Vec<GraphEdge> = Vec::new();
        let mut all_nodes: Vec<String> = Vec::new();
        let mut seen_edges: HashSet<(String, String)> = HashSet::new();

        for sf in &source_files {
            let from = sf.strip_prefix(&project_dir).unwrap_or(sf)
                .display().to_string();
            all_nodes.push(from.clone());
            collect_include_edges(
                sf, &project_dir, &include_dirs, show_system,
                &mut all_edges, &mut seen_edges, &mut HashSet::new(),
            );
        }

        let title = format!("{} includes", manifest.package.name);
        match fmt {
            GraphFormat::Mermaid => render_mermaid_graph(&title, &[], &all_edges, &all_nodes),
            GraphFormat::Dot     => render_dot_graph(&title, &[], &all_edges, &all_nodes),
            GraphFormat::Text    => unreachable!(),
        }
        return;
    }

    // Text tree output.
    println!(
        "{} {}",
        manifest.package.name.bold().bright_blue(),
        manifest.package.version.bright_black()
    );

    let mut globally_seen: HashSet<PathBuf> = HashSet::new();

    for (i, sf) in source_files.iter().enumerate() {
        let is_last = i == source_files.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };

        let rel = sf.strip_prefix(&project_dir).unwrap_or(sf);
        println!("{}{}", connector.bright_black(), rel.display().to_string().yellow());

        print_include_tree(
            sf,
            &project_dir,
            &include_dirs,
            child_prefix,
            &mut globally_seen,
            &mut HashSet::new(),
            show_system,
        );
    }
}

/// Walk `src/` and return all C/C++/CUDA source files (not headers).
fn collect_source_files(src_dir: &Path) -> Vec<PathBuf> {
    const SOURCE_EXTS: &[&str] = &["c", "cc", "cpp", "cxx", "c++", "cu", "hip", "m", "mm"];
    let mut files = Vec::new();
    if !src_dir.is_dir() { return files; }
    collect_files_recursive(src_dir, SOURCE_EXTS, &mut files);
    files.sort();
    files
}

fn collect_files_recursive(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, exts, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.contains(&ext) {
                out.push(path);
            }
        }
    }
}

/// Recursively collect all include edges reachable from `file` into `edges`.
fn collect_include_edges(
    file: &Path,
    project_dir: &Path,
    include_dirs: &[PathBuf],
    show_system: bool,
    edges: &mut Vec<GraphEdge>,
    seen_edges: &mut HashSet<(String, String)>,
    in_stack: &mut HashSet<PathBuf>,
) {
    let from = file.strip_prefix(project_dir).unwrap_or(file).display().to_string();
    for (inc, is_system) in parse_includes(file) {
        if is_system && !show_system { continue; }
        let to = if is_system {
            format!("<{inc}>")
        } else {
            match resolve_include(&inc, file, include_dirs) {
                Some(ref resolved) => resolved.strip_prefix(project_dir).unwrap_or(resolved).display().to_string(),
                None => format!("\"{inc}\" (not found)"),
            }
        };
        let key = (from.clone(), to.clone());
        if seen_edges.insert(key) {
            edges.push(GraphEdge { from: from.clone(), to: to.clone() });
            if !is_system {
                if let Some(resolved) = resolve_include(&inc, file, include_dirs) {
                    if !in_stack.contains(&resolved) {
                        in_stack.insert(resolved.clone());
                        collect_include_edges(&resolved, project_dir, include_dirs, show_system, edges, seen_edges, in_stack);
                        in_stack.remove(&resolved);
                    }
                }
            }
        }
    }
}

/// Parse `#include "..."` and optionally `#include <...>` from a file.
fn parse_includes(path: &Path) -> Vec<(String, bool)> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut result = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('#') { continue; }
        let rest = line[1..].trim_start();
        if !rest.starts_with("include") { continue; }
        let rest = rest[7..].trim_start();
        if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.split('"').next()) {
            result.push((inner.to_string(), false));
        } else if let Some(inner) = rest.strip_prefix('<').and_then(|s| s.split('>').next()) {
            result.push((inner.to_string(), true));
        }
    }
    result
}

/// Resolve an include path to an absolute file path, or `None` if not found.
fn resolve_include(include: &str, from_file: &Path, include_dirs: &[PathBuf]) -> Option<PathBuf> {
    // 1. Relative to the file containing the #include.
    if let Some(parent) = from_file.parent() {
        let candidate = parent.join(include);
        if candidate.is_file() { return Some(candidate); }
    }
    // 2. Each include dir in order.
    for dir in include_dirs {
        let candidate = dir.join(include);
        if candidate.is_file() { return Some(candidate); }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn print_include_tree(
    file: &Path,
    project_dir: &Path,
    include_dirs: &[PathBuf],
    prefix: &str,
    globally_seen: &mut HashSet<PathBuf>,
    in_stack: &mut HashSet<PathBuf>,
    show_system: bool,
) {
    let includes = parse_includes(file);
    let visible: Vec<_> = includes.iter()
        .filter(|(_, sys)| show_system || !sys)
        .collect();

    for (i, (inc, is_system)) in visible.iter().enumerate() {
        let is_last = i == visible.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
        let branch = format!("{prefix}{connector}").bright_black().to_string();

        if *is_system {
            println!("{}{}", branch, format!("<{inc}>").cyan());
            continue;
        }

        match resolve_include(inc, file, include_dirs) {
            None => {
                println!("{}{} {}", branch, format!("\"{inc}\"").yellow(), "[not found]".red());
            }
            Some(resolved) => {
                let rel = resolved.strip_prefix(project_dir).unwrap_or(&resolved);

                if in_stack.contains(&resolved) {
                    println!(
                        "{}{} {} {}",
                        branch,
                        format!("\"{inc}\"").yellow(),
                        rel.display().to_string().bright_black(),
                        "(cycle)".red().bold()
                    );
                    continue;
                }

                if globally_seen.contains(&resolved) {
                    println!(
                        "{}{} {} {}",
                        branch,
                        format!("\"{inc}\"").yellow(),
                        rel.display().to_string().bright_black(),
                        "(see above)".bright_black()
                    );
                    continue;
                }

                println!("{}{} {}", branch, format!("\"{inc}\"").yellow(), rel.display().to_string().bright_black());
                globally_seen.insert(resolved.clone());
                in_stack.insert(resolved.clone());
                print_include_tree(
                    &resolved,
                    project_dir,
                    include_dirs,
                    &child_prefix,
                    globally_seen,
                    in_stack,
                    show_system,
                );
                in_stack.remove(&resolved);
            }
        }
    }
}

// ── freight add ────────────────────────────────────────────────────────────────

/// True when `package` looks like a git or archive URL that should be auto-routed.
/// Matches:
///   - https:// / http://          — HTTPS git repos and archives
///   - ssh://                      — SSH git URLs
///   - git@host:…                  — SCP-style SSH git URLs (git@github.com:user/repo.git)
fn looks_like_url(s: &str) -> bool {
    s.starts_with("https://")
        || s.starts_with("http://")
        || s.starts_with("ssh://")
        || s.starts_with("git@")
}

/// True when `url` points to a downloadable archive rather than a git repo.
/// SSH-style URLs are always git; HTTP URLs are archives only when the path ends
/// with a known archive extension.
fn url_is_archive(url: &str) -> bool {
    // SSH URLs are never archives.
    if url.starts_with("ssh://") || url.starts_with("git@") {
        return false;
    }
    // Strip query string / fragment for extension detection.
    let path = url.split('?').next().unwrap_or(url).split('#').next().unwrap_or(url);
    const ARCHIVE_EXTS: &[&str] = &[
        ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst", ".zip", ".7z",
        ".whl", ".gem", ".hpp", ".h", ".c",
    ];
    ARCHIVE_EXTS.iter().any(|ext| path.ends_with(ext))
}

/// Derive a dep name from a URL: take the last path segment, strip `.git` and archive extensions.
/// Handles both slash-separated HTTP URLs and colon-separated SCP paths (`git@host:user/repo.git`).
fn url_dep_name(url: &str) -> String {
    // For SCP-style `git@host:path/to/repo.git`, split on `:` first.
    let path_part = if url.starts_with("git@") {
        url.splitn(2, ':').nth(1).unwrap_or(url)
    } else {
        url.split('?').next().unwrap_or(url).split('#').next().unwrap_or(url)
    };
    let last = path_part.rsplit('/').find(|s| !s.is_empty()).unwrap_or("dep");
    const STRIP_SUFFIXES: &[&str] = &[
        ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst",
        ".zip", ".7z", ".git", ".hpp", ".h", ".c",
    ];
    let mut name = last;
    for suffix in STRIP_SUFFIXES {
        if let Some(s) = name.strip_suffix(suffix) { name = s; break; }
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

    // Auto-detect git / URL archive when the package argument is a raw URL.
    if looks_like_url(package) && path.is_none() && git.is_none() && !system
    {
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
        let section = if dev { "dev-dependencies" } else { "dependencies" };
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
        refresh_lock(&project_dir);
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
                match r.lookup(dep_name, channel_arg) {
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

        if repo_key.is_empty() && channel_arg.is_none() {
            Dependency::Simple(ver)
        } else {
            Dependency::Detailed(DetailedDep {
                version: Some(ver),
                repo: if repo_key.is_empty() { None } else { Some(repo_key) },
                channel: channel_arg.map(str::to_string),
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
pub fn cmd_add_interactive(repo: Option<&str>, dev: bool) {
    match crate::tui::run_package_browser(repo) {
        Ok(Some(selection)) => {
            cmd_add(&format!("{}@{}", selection.name, selection.version),
                None, None, None, None, None, false, repo, dev);
        }
        Ok(None) => {} // user cancelled
        Err(e) => print_warning(&format!("TUI error: {e}")),
    }
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

pub fn cmd_fetch(force_source: bool) {
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

    // Download version deps from configured registries.
    // Try prebuilt for the host triple first (unless --source was passed).
    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };

    if !force_source {
        let triple = host_triple();
        fetch_prebuilt_deps(&manifest, &project_dir, &config, &triple, &mut any_work, &mut all_ok);
    }

    match fetch_registry_deps(&project_dir, &config) {
        Ok(outcomes) => {
            for o in outcomes {
                // Skip deps that were already handled as prebuilts (sentinel exists).
                let sentinel = project_dir.join(".deps").join(&o.name).join(".freight-fetched");
                if sentinel.exists() { continue; }
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

// ── Prebuilt helpers ───────────────────────────────────────────────────────────

/// For every registry version dep, check if a prebuilt for `triple` is available
/// and download it. Deps that are already fetched (sentinel present) are skipped.
fn fetch_prebuilt_deps(
    manifest:    &Manifest,
    project_dir: &std::path::Path,
    config:      &GlobalConfig,
    triple:      &str,
    any_work:    &mut bool,
    all_ok:      &mut bool,
) {
    for (name, dep) in &manifest.dependencies {
        let (version, repo_key, channel) = match dep {
            Dependency::Simple(v) => (v.as_str(), None, None),
            Dependency::Detailed(d)
                if d.version.is_some()
                    && d.path.is_none()
                    && d.system.is_none()
                    && d.git.is_none()
                    && d.url.is_none() =>
            {
                (d.version.as_deref().unwrap(), d.repo.as_deref(), d.channel.as_deref())
            }
            _ => continue,
        };

        if version.is_empty() || version == "*" { continue; }

        // Already fetched (source or prebuilt).
        let sentinel = project_dir.join(".deps").join(name).join(".freight-fetched");
        if sentinel.exists() { continue; }

        let registry = if let Some(rkey) = repo_key {
            match config.registries.iter().find(|r| r.name == rkey) {
                Some(c) => FreightRegistry::from_config(c),
                None    => continue,
            }
        } else {
            match config.registries.first() {
                Some(c) => FreightRegistry::from_config(c),
                None    => FreightRegistry::default_registry(),
            }
        };

        // Check if a prebuilt exists for this triple.
        let triples = match registry.list_prebuilt_triples(name, version, channel) {
            Ok(t)  => t,
            Err(_) => continue, // registry unreachable, fall through to source
        };

        if !triples.contains(&triple.to_string()) { continue; }

        *any_work = true;
        print_status("prebuilt", &format!("downloading `{name}@{version}` ({triple})…"));
        match registry.download_prebuilt(name, version, channel, triple, project_dir) {
            Ok(_) => print_success(&format!("fetched `{name}@{version}` (prebuilt/{triple})")),
            Err(e) => {
                print_warning(&format!("`{name}`: prebuilt download failed ({e}), will fall back to source"));
                *all_ok = false;
            }
        }
    }
}

// ── freight publish --prebuilt ────────────────────────────────────────────────

pub fn cmd_publish_prebuilt(triple: Option<&str>, repo: Option<&str>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let triple = triple.map(str::to_string).unwrap_or_else(host_triple);
    let name    = &manifest.package.name;
    let version = &manifest.package.version;

    print_status("prebuilt", &format!("packaging `{name}@{version}` for {triple}…"));

    // Build the prebuilt tarball in memory.
    let tarball = match build_prebuilt_tarball(&project_dir, &manifest, &triple) {
        Ok(t)  => t,
        Err(e) => { print_error(&format!("packaging failed: {e}")); return; }
    };

    // Resolve the registry.
    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };
    let registry: FreightRegistry = if let Some(rname) = repo {
        match config.registries.iter().find(|c| c.name == rname) {
            Some(c) => FreightRegistry::from_config(c),
            None    => { print_error(&format!("registry `{rname}` not found in config")); return; }
        }
    } else {
        match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None    => FreightRegistry::default_registry(),
        }
    };

    let channel: Option<&str> = None; // channel for the prebuilt upload (defaults to "stable")
    match registry.upload_prebuilt(name, version, channel, &triple, &tarball) {
        Ok(()) => print_success(&format!(
            "published prebuilt `{name}@{version}` for {triple}"
        )),
        Err(e) => print_error(&format!("upload failed: {e}")),
    }
}

/// Pack `include/`, compiled libs from `target/release/`, and a generated `.pc`
/// file into a gzip tarball and return the raw bytes.
fn build_prebuilt_tarball(
    project_dir: &std::path::Path,
    manifest:    &Manifest,
    _triple:     &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let name    = &manifest.package.name;
    let version = &manifest.package.version;
    let desc    = &manifest.package.description;

    let enc  = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut ar = tar::Builder::new(enc);

    // ── headers ───────────────────────────────────────────────────────────────
    let include_dir = project_dir.join("include");
    if include_dir.is_dir() {
        ar.append_dir_all("include", &include_dir)?;
    }

    // ── compiled library ──────────────────────────────────────────────────────
    let release_dir = project_dir.join("target").join("release");
    for ext in &["a", "so", "dll", "dylib", "lib"] {
        // Search for lib<name>.ext or <name>.ext
        for stem in &[format!("lib{name}"), name.clone()] {
            let candidate = release_dir.join(format!("{stem}.{ext}"));
            if candidate.is_file() {
                let dest = format!("lib/{stem}.{ext}");
                ar.append_path_with_name(&candidate, &dest)?;
            }
        }
    }

    // ── pkg-config .pc file ───────────────────────────────────────────────────
    let pc = format!(
        "prefix=/usr/local\n\
         libdir=${{prefix}}/lib\n\
         includedir=${{prefix}}/include\n\
         \n\
         Name: {name}\n\
         Description: {desc}\n\
         Version: {version}\n\
         Cflags: -I${{includedir}}\n\
         Libs: -L${{libdir}} -l{name}\n",
    );
    let pc_bytes = pc.as_bytes();
    let pc_path  = format!("lib/pkgconfig/{name}.pc");
    let mut header = tar::Header::new_gnu();
    header.set_size(pc_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append_data(&mut header, &pc_path, pc_bytes)?;

    let gz = ar.into_inner()?.finish()?;
    Ok(gz)
}

// ── freight outdated ──────────────────────────────────────────────────────────

pub fn cmd_outdated(repo: Option<&str>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => { print_error(&format!("cannot read cwd: {e}")); return; }
    };
    let project_dir = match find_manifest_dir(&cwd) {
        Some(d) => d,
        None => { print_error("no freight.toml found"); return; }
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) {
            cfg.apply_local(local);
        }
        cfg
    };

    // Collect registry deps: (name, current_version, channel, repo_key).
    struct RegistryDep {
        name:     String,
        current:  String,
        channel:  Option<String>,
        repo_key: Option<String>,
    }

    let mut registry_deps: Vec<RegistryDep> = Vec::new();
    for (name, dep) in &manifest.dependencies {
        match dep {
            Dependency::Simple(ver) => {
                registry_deps.push(RegistryDep {
                    name:    name.clone(),
                    current: ver.clone(),
                    channel: None,
                    repo_key: None,
                });
            }
            Dependency::Detailed(d)
                if d.version.is_some()
                    && d.path.is_none()
                    && d.system.is_none()
                    && d.git.is_none()
                    && d.url.is_none() =>
            {
                let ver = d.version.as_deref().unwrap();
                if ver.is_empty() || ver == "*" { continue; }
                registry_deps.push(RegistryDep {
                    name:    name.clone(),
                    current: ver.to_string(),
                    channel: d.channel.clone(),
                    repo_key: d.repo.clone(),
                });
            }
            _ => {}
        }
    }

    if registry_deps.is_empty() {
        println!("no registry dependencies to check");
        return;
    }

    // For each dep, query the registry for the latest version.
    struct OutdatedRow {
        name:    String,
        current: String,
        latest:  String,
        outdated: bool,
    }

    let mut rows: Vec<OutdatedRow> = Vec::new();
    let mut any_error = false;

    for dep in &registry_deps {
        let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = if let Some(rname) = repo {
            match repo_by_name(rname, &config) {
                Ok(r) => vec![r],
                Err(e) => { print_error(&e.to_string()); return; }
            }
        } else if let Some(rkey) = &dep.repo_key {
            match repo_by_name(rkey, &config) {
                Ok(r) => vec![r],
                Err(e) => { print_error(&e.to_string()); return; }
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
                        name:    dep.name.clone(),
                        current: dep.current.clone(),
                        latest:  info.latest,
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
            print_warning(&format!("`{}`: not found in any configured registry", dep.name));
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

    let name_w    = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let current_w = rows.iter().map(|r| r.current.len()).max().unwrap_or(7).max(7);
    let latest_w  = rows.iter().map(|r| r.latest.len()).max().unwrap_or(6).max(6);

    println!(
        "{:<name_w$}  {:<current_w$}  {:<latest_w$}  {}",
        "name".bold(),
        "current".bold(),
        "latest".bold(),
        "status".bold(),
    );
    println!("{}", "─".repeat(name_w + current_w + latest_w + 14).bright_black());

    let mut any_outdated = false;
    for row in &rows {
        let (latest_col, status_col) = if row.outdated {
            any_outdated = true;
            (row.latest.yellow().to_string(), "outdated".yellow().to_string())
        } else {
            (row.latest.green().to_string(), "up to date".green().to_string())
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
        println!("run {} to upgrade outdated dependencies", "`freight add <name>@<version>`".bright_blue());
    } else {
        println!();
        println!("{}", "all dependencies are up to date".green());
    }

    if any_error {
        std::process::exit(1);
    }
}

/// Compare two version strings. Returns `true` if `latest` is newer than `current`.
/// Falls back to string comparison when semver parsing fails.
fn is_outdated(current: &str, latest: &str) -> bool {
    use semver::Version;
    // Strip leading '=' or '^' or '~' from current before comparing.
    let current_clean = current.trim_start_matches(|c: char| matches!(c, '=' | '^' | '~' | ' '));
    match (Version::parse(current_clean), Version::parse(latest)) {
        (Ok(cur), Ok(lat)) => lat > cur,
        _ => latest != current,
    }
}

// ── Registry commands ────────────────────────────────────────────────────────

pub fn cmd_search(query: &str, repo: Option<&str>) {
    let config = GlobalConfig::load();

    let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = if let Some(rname) = repo {
        match repo_by_name(rname, &config) {
            Ok(r) => vec![r],
            Err(e) => { print_error(&e.to_string()); return; }
        }
    } else {
        registries_in_order(&config)
    };

    let mut any = false;
    for r in &repos {
        let label = if r.repo_key().is_empty() { "freight.dev" } else { r.repo_key() };
        match r.search(query) {
            Ok(results) if !results.is_empty() => {
                if !any {
                    println!("{:<32}  {:<12}  {}", "name".bold(), "latest".bold(), "description".bold());
                    println!("{}", "─".repeat(72).bright_black());
                }
                for pkg in &results {
                    println!(
                        "{:<32}  {:<12}  {}",
                        pkg.name.bright_blue(),
                        pkg.latest.bright_black(),
                        pkg.description.as_deref().unwrap_or("").dimmed()
                    );
                }
                any = true;
            }
            Ok(_) => {
                print_status(label, &format!("no results for `{query}`"));
            }
            Err(e) => {
                print_warning(&format!("{label}: {e}"));
            }
        }
    }

    if !any {
        println!("no packages found matching `{query}`");
    }
}

pub fn cmd_info(package: Option<&str>, repo: Option<&str>) {
    if let Some(package) = package {
        let config = GlobalConfig::load();
        let repos: Vec<Box<dyn freight_core::registry::PackageRepo>> = if let Some(rname) = repo {
            match repo_by_name(rname, &config) {
                Ok(r) => vec![r],
                Err(e) => { print_error(&e.to_string()); return; }
            }
        } else {
            registries_in_order(&config)
        };

        for r in &repos {
            let label = if r.repo_key().is_empty() { "freight.dev" } else { r.repo_key() };
            match r.lookup(package, None) {
                Ok(Some(info)) => {
                    println!("{} {}", info.name.bold().bright_blue(), format!("(via {label})").bright_black());
                    if let Some(desc) = &info.description {
                        println!("  {desc}");
                    }
                    if let Some(readme) = r.fetch_readme(&info.name) {
                        println!();
                        print_readme_excerpt(&readme);
                    }
                    println!();
                    println!("  {:<16}  {}", "version".bold(), "status".bold());
                    println!("  {}", "─".repeat(30).bright_black());
                    for v in &info.versions {
                        let yanked = if v.checksum.is_none() { "" } else { "" };
                        println!("  {:<16}  {yanked}", v.version.bright_blue());
                    }

                    // Show dependencies from the latest version.
                    if let Some(latest) = info.versions.first() {
                        if !latest.dependencies.is_empty() {
                            println!();
                            println!("  {}", "dependencies".bold());
                            println!("  {}", "─".repeat(30).bright_black());
                            let mut deps: Vec<_> = latest.dependencies.iter().collect();
                            deps.sort_by_key(|(k, _)| k.as_str());
                            for (name, ver) in deps {
                                println!("  {:<24}  {}", name.bright_blue(), ver.bright_black());
                            }
                        }
                    }
                    return;
                }
                Ok(None) => continue,
                Err(e) => {
                    print_warning(&format!("{label}: {e}"));
                    continue;
                }
            }
        }
        print_error(&format!("`{package}` not found in any configured registry"));
        return;
    }

    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    print_current_package_info(&project_dir, &manifest);
}

/// Print the first prose section of a README, with markdown formatting stripped.
/// Stops at the second `##` heading or after ~500 chars of content.
fn print_readme_excerpt(readme: &str) {
    let mut output = String::new();
    let mut h2_count = 0;

    for line in readme.lines() {
        // Stop before second ## section
        if line.starts_with("## ") || line.starts_with("## ") {
            h2_count += 1;
            if h2_count > 1 { break; }
            // Print first ## as a bold header
            let title = line.trim_start_matches('#').trim();
            output.push_str(&format!("  {}\n", title.bold()));
            continue;
        }
        // Skip top-level # title (already shown as package name)
        if line.starts_with("# ") { continue; }
        // Skip HTML tags, badges, shields
        if line.trim_start().starts_with('<') || line.contains("shields.io") || line.contains("badge") { continue; }
        // Skip pure horizontal rules
        if line.chars().all(|c| c == '-' || c == '=' || c == '*' || c.is_whitespace()) && !line.is_empty() { continue; }

        // Strip inline markdown
        let stripped = strip_inline_md(line);
        output.push_str(&format!("  {stripped}\n"));

        if output.len() > 500 {
            output.push_str("  …\n");
            break;
        }
    }

    let trimmed = output.trim_end();
    if !trimmed.is_empty() {
        println!("{trimmed}");
    }
}

fn strip_inline_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Bold/italic: **, *, __
            '*' | '_' => {
                if chars.peek() == Some(&c) { chars.next(); }
            }
            // Inline code: `...`
            '`' => {
                while chars.peek().map(|&x| x != '`').unwrap_or(false) {
                    out.push(chars.next().unwrap());
                }
                chars.next(); // closing `
            }
            // Links: [text](url) → just text
            '[' => {
                while let Some(&ch) = chars.peek() {
                    if ch == ']' { chars.next(); break; }
                    out.push(chars.next().unwrap());
                }
                // consume (url)
                if chars.peek() == Some(&'(') {
                    chars.next();
                    while chars.peek().map(|&x| x != ')').unwrap_or(false) { chars.next(); }
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    out
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

pub fn cmd_login(registry_url: Option<&str>, token_arg: Option<&str>) {
    let config = GlobalConfig::load();

    // Determine the registry URL: explicit flag → first configured → freight.dev.
    let url = registry_url
        .map(str::to_string)
        .or_else(|| config.registries.first().map(|r| r.url.clone()))
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    // Find the registry name for display and credentials key.
    let name = config
        .registries
        .iter()
        .find(|r| r.url == url)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "freight".to_string());

    let token = match token_arg {
        Some(t) => t.to_string(),
        None => {
            use std::io::{self, Write};
            print!("Token for {url}: ");
            io::stdout().flush().ok();
            let mut t = String::new();
            io::stdin().read_line(&mut t).ok();
            t.trim().to_string()
        }
    };

    if token.is_empty() {
        print_error("token cannot be empty");
        return;
    }

    match GlobalConfig::save_credential(&url, &name, &token) {
        Ok(()) => {
            let creds_path = freight_home()
                .map(|h| h.join("credentials.toml").to_string_lossy().into_owned())
                .unwrap_or_else(|| "~/.freight/credentials.toml".into());
            print_success(&format!("token saved to {creds_path}"));
        }
        Err(e) => print_error(&e.to_string()),
    }
}

pub fn cmd_publish(dry_run: bool, repo: Option<&str>) {
    let project_dir = match locate_project_dir() {
        Some(d) => d,
        None => return,
    };
    let manifest = match load_manifest(&project_dir) {
        Ok(m) => m,
        Err(e) => { print_error(&e.to_string()); return; }
    };

    let name    = &manifest.package.name;
    let version = &manifest.package.version;
    let description = if manifest.package.description.is_empty() { None } else { Some(manifest.package.description.as_str()) };
    let license     = if manifest.package.license.is_empty()     { None } else { Some(manifest.package.license.as_str())     };

    if dry_run {
        print_status("dry-run", &format!("would publish {name}@{version}"));
        if let Some(d) = description { print_status("description", d); }
        if let Some(l) = license     { print_status("license", l); }
        return;
    }

    // Bundle the project into a tarball, excluding target/ .deps/ .freight-build/
    let archive = project_dir.join("target").join(format!("{name}-{version}.tar.gz"));
    if let Some(p) = archive.parent() { std::fs::create_dir_all(p).ok(); }

    print_status("packaging", &format!("{name}@{version}"));

    let ok = std::process::Command::new("tar")
        .current_dir(&project_dir)
        .args([
            "--exclude=./target",
            "--exclude=./.deps",
            "--exclude=./.freight-build",
            "-czf",
            &archive.to_string_lossy(),
            ".",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        print_error("failed to create tarball — is `tar` installed?");
        return;
    }

    let tarball = match std::fs::read(&archive) {
        Ok(b) => b,
        Err(e) => { print_error(&format!("cannot read tarball: {e}")); return; }
    };
    let _ = std::fs::remove_file(&archive);

    let config = {
        let mut cfg = GlobalConfig::load();
        if let Some(local) = GlobalConfig::load_local(&project_dir) { cfg.apply_local(local); }
        cfg
    };

    let registry: FreightRegistry = if let Some(rname) = repo {
        match config.registries.iter().find(|r| r.name == rname) {
            Some(c) => FreightRegistry::from_config(c),
            None    => { print_error(&format!("unknown registry `{rname}`")); return; }
        }
    } else {
        match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None    => FreightRegistry::default_registry(),
        }
    };

    print_status("publishing", &format!("{name}@{version} ({} bytes)", tarball.len()));

    match registry.publish_package(name, version, None, description, license, &tarball) {
        Ok(()) => print_success(&format!("published {name}@{version}")),
        Err(e) => print_error(&e.to_string()),
    }
}

pub fn cmd_yank(version_arg: &str, undo: bool, repo: Option<&str>) {
    // Accept "name@version" or just "version" (infer name from current manifest).
    let (pkg_name, version) = if let Some(at) = version_arg.find('@') {
        (version_arg[..at].to_string(), &version_arg[at + 1..])
    } else {
        let project_dir = match locate_project_dir() {
            Some(d) => d,
            None => return,
        };
        let manifest = match load_manifest(&project_dir) {
            Ok(m) => m,
            Err(e) => { print_error(&e.to_string()); return; }
        };
        (manifest.package.name.clone(), version_arg)
    };

    let config = GlobalConfig::load();
    let registry: FreightRegistry = if let Some(rname) = repo {
        match config.registries.iter().find(|r| r.name == rname) {
            Some(c) => FreightRegistry::from_config(c),
            None    => { print_error(&format!("unknown registry `{rname}`")); return; }
        }
    } else {
        match config.registries.first() {
            Some(c) => FreightRegistry::from_config(c),
            None    => FreightRegistry::default_registry(),
        }
    };

    let action = if undo { "unyank" } else { "yank" };
    print_status(action, &format!("{pkg_name}@{version}"));

    match registry.yank_version(&pkg_name, version, !undo) {
        Ok(()) => print_success(&format!("{action}ed {pkg_name}@{version}")),
        Err(e) => print_error(&e.to_string()),
    }
}

pub fn cmd_register(
    registry_url: Option<&str>,
    username_arg: Option<&str>,
    email_arg: Option<&str>,
    token_name_arg: Option<&str>,
) {
    let config = GlobalConfig::load();

    let url = registry_url
        .map(str::to_string)
        .or_else(|| config.registries.first().map(|r| r.url.clone()))
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    let reg_name = config
        .registries
        .iter()
        .find(|r| r.url == url)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "freight".to_string());

    let username = match username_arg {
        Some(u) => u.to_string(),
        None => {
            use std::io::{self, Write};
            print!("Username: ");
            io::stdout().flush().ok();
            let mut u = String::new();
            io::stdin().read_line(&mut u).ok();
            u.trim().to_string()
        }
    };

    if username.is_empty() {
        print_error("username cannot be empty");
        return;
    }

    let password = {
        use std::io::{self, Write};
        print!("Password: ");
        io::stdout().flush().ok();
        let mut p1 = String::new();
        io::stdin().read_line(&mut p1).ok();
        let p1 = p1.trim().to_string();

        print!("Confirm password: ");
        io::stdout().flush().ok();
        let mut p2 = String::new();
        io::stdin().read_line(&mut p2).ok();
        let p2 = p2.trim().to_string();

        if p1 != p2 {
            print_error("passwords do not match");
            return;
        }
        if p1.len() < 8 {
            print_error("password must be at least 8 characters");
            return;
        }
        p1
    };

    let cfg = freight_core::toolchain::cache::RegistryConfig {
        name: reg_name.clone(),
        url:  url.clone(),
        token: None,
    };
    let registry = FreightRegistry::from_config(&cfg);

    print_status("register", &format!("creating account `{username}` on {url}…"));

    match registry.register_user(&username, &password, email_arg, token_name_arg) {
        Ok((_, token)) => {
            match GlobalConfig::save_credential(&url, &reg_name, &token) {
                Ok(()) => {
                    let creds_path = freight_home()
                        .map(|h| h.join("credentials.toml").to_string_lossy().into_owned())
                        .unwrap_or_else(|| "~/.freight/credentials.toml".into());
                    print_success(&format!(
                        "registered as `{username}` — token saved to {creds_path}"
                    ));
                }
                Err(e) => {
                    print_success(&format!("registered as `{username}`"));
                    print_warning(&format!("could not save token automatically: {e}"));
                }
            }
        }
        Err(e) => print_error(&e.to_string()),
    }
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
