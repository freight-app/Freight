use std::collections::HashSet;
use std::path::{Path, PathBuf};

use freight::dep_cmds::{locate_project, DetailedDep};
use freight::manifest::types::{Dependency, Manifest};
use freight::manifest::{load_manifest, load_workspace_manifest};

use crate::output::{print_error, render_dot_graph, render_mermaid_graph, GraphEdge, GraphFormat};
use owo_colors::OwoColorize;

#[derive(clap::Args)]
pub struct Args {
    /// Show the include graph for source and header files instead of the dependency tree
    #[arg(long, short = 's')]
    pub sources: bool,
    /// Also show system headers (#include <...>) when using --sources
    #[arg(long, short = 'a', requires = "sources")]
    pub all: bool,
    /// Output format: text (default), mermaid, dot
    #[arg(long, short = 'f', default_value = "text", value_name = "FORMAT")]
    pub format: String,
    /// Maximum dependency depth to display (omit for unlimited)
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,
}

impl Args {
    pub fn run(self) {
        if self.sources {
            cmd_includes(self.all, &self.format);
        } else {
            cmd_tree(self.depth);
        }
    }
}

// ── freight tree ───────────────────────────────────────────────────────────────

pub fn cmd_tree(depth: Option<usize>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };

    if let Some(ws) = load_workspace_manifest(&cwd) {
        for (i, member) in ws.members.iter().enumerate() {
            if i > 0 {
                println!();
            }
            let member_dir = cwd.join(member);
            match load_manifest(&member_dir) {
                Ok(manifest) => {
                    println!(
                        "{} {} {}",
                        manifest.package.name.bold().bright_blue(),
                        manifest.package.version.bright_black(),
                        format!("({})", member).bright_black()
                    );
                    print_dep_groups(&manifest, &member_dir, depth);
                }
                Err(e) => print_error(&format!("{member}: {e}")),
            }
        }
        return;
    }

    let (project_dir, manifest) = match locate_project() {
        Ok(p) => p,
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };

    println!(
        "{} {}",
        manifest.package.name.bold().bright_blue(),
        manifest.package.version.bright_black()
    );
    print_dep_groups(&manifest, &project_dir, depth);
}

/// Print a project's full dependency tree: `[dependencies]` (recursed into path
/// deps), then flat `[build-dependencies]` and `[dev-dependencies]` groups —
/// matching `cargo tree`, which surfaces every dependency kind.
fn print_dep_groups(manifest: &Manifest, project_dir: &Path, depth: Option<usize>) {
    print_dep_tree(manifest, project_dir, "", depth);
    print_dep_group(
        "build-dependencies",
        &manifest.build_dependencies,
        project_dir,
        depth,
    );
    print_dep_group(
        "dev-dependencies",
        &manifest.dev_dependencies,
        project_dir,
        depth,
    );
}

fn print_dep_group(
    label: &str,
    deps: &std::collections::HashMap<String, Dependency>,
    project_dir: &Path,
    depth: Option<usize>,
) {
    if deps.is_empty() || depth == Some(0) {
        return;
    }
    println!("{}", format!("[{label}]").bright_black());
    let mut v: Vec<(&String, &Dependency)> = deps.iter().collect();
    v.sort_by_key(|(k, _)| k.as_str());
    print_named_deps(&v, project_dir, "", depth);
}

fn print_dep_tree(manifest: &Manifest, project_dir: &Path, prefix: &str, depth: Option<usize>) {
    if depth == Some(0) {
        return;
    }
    let mut deps: Vec<(&String, &Dependency)> = manifest.dependencies.iter().collect();
    deps.sort_by_key(|(k, _)| k.as_str());
    print_named_deps(&deps, project_dir, prefix, depth);
}

fn print_named_deps(
    deps: &[(&String, &Dependency)],
    project_dir: &Path,
    prefix: &str,
    depth: Option<usize>,
) {
    for (i, (name, dep)) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
        let branch = format!("{prefix}{connector}").bright_black().to_string();

        match dep {
            Dependency::Simple(ver) => {
                print_package_dep(&branch, name, ver);
            }
            Dependency::Detailed(d) if freight::manifest::types::is_platform_dep(name) => {
                print_platform_dep(&branch, name, d);
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
                    print_dep_tree(&m, &dep_dir, &child_prefix, depth.map(|d| d - 1));
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
            Dependency::Detailed(d) if d.is_git() => {
                let url = d.url.as_deref().unwrap_or("?");
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

fn print_platform_dep(branch: &str, name: &str, dep: &DetailedDep) {
    let features = if dep.features.is_empty() {
        String::new()
    } else {
        format!(" [{}]", dep.features.join(", "))
    };
    println!(
        "{}{} {}{}",
        branch,
        name.bold().bright_blue(),
        "(platform)".cyan(),
        features
    );
}

// ── freight tree --sources (includes) ─────────────────────────────────────────

pub fn cmd_includes(show_system: bool, format: &str) {
    let fmt = GraphFormat::parse(format);

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            print_error(&format!("cannot read cwd: {e}"));
            return;
        }
    };

    if let Some(ws) = load_workspace_manifest(&cwd) {
        for (i, member) in ws.members.iter().enumerate() {
            if i > 0 {
                println!();
            }
            let member_dir = cwd.join(member);
            match load_manifest(&member_dir) {
                Ok(manifest) => {
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
        Err(e) => {
            print_error(&e.to_string());
            return;
        }
    };
    print_includes_for(&project_dir, &manifest, show_system, fmt);
}

fn print_includes_for(
    project_dir: &std::path::Path,
    manifest: &Manifest,
    show_system: bool,
    fmt: GraphFormat,
) {
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let inc = project_dir.join("inc");
    let src = project_dir.join("src");
    if inc.is_dir() {
        include_dirs.push(inc);
    }
    if src.is_dir() {
        include_dirs.push(src.clone());
    }
    for d in &manifest.compiler.includes {
        let p = project_dir.join(d);
        if p.is_dir() {
            include_dirs.push(p);
        }
    }

    let source_files = collect_source_files(&src);
    if source_files.is_empty() {
        println!("no source files found in {}", src.display());
        return;
    }

    if fmt != GraphFormat::Text {
        let mut all_edges: Vec<GraphEdge> = Vec::new();
        let mut all_nodes: Vec<String> = Vec::new();
        let mut seen_edges: HashSet<(String, String)> = HashSet::new();

        for sf in &source_files {
            let from = sf
                .strip_prefix(project_dir)
                .unwrap_or(sf)
                .display()
                .to_string();
            all_nodes.push(from.clone());
            collect_include_edges(
                sf,
                project_dir,
                &include_dirs,
                show_system,
                &mut all_edges,
                &mut seen_edges,
                &mut HashSet::new(),
            );
        }

        let title = format!("{} includes", manifest.package.name);
        match fmt {
            GraphFormat::Mermaid => render_mermaid_graph(&title, &[], &all_edges, &all_nodes),
            GraphFormat::Dot => render_dot_graph(&title, &[], &all_edges, &all_nodes),
            GraphFormat::Text => unreachable!(),
        }
        return;
    }

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

        let rel = sf.strip_prefix(project_dir).unwrap_or(sf);
        println!(
            "{}{}",
            connector.bright_black(),
            rel.display().to_string().yellow()
        );

        print_include_tree(
            sf,
            project_dir,
            &include_dirs,
            child_prefix,
            &mut globally_seen,
            &mut HashSet::new(),
            show_system,
        );
    }
}

fn collect_source_files(src_dir: &Path) -> Vec<PathBuf> {
    const SOURCE_EXTS: &[&str] = &["c", "cc", "cpp", "cxx", "c++", "cu", "hip", "m", "mm"];
    let mut files = Vec::new();
    if !src_dir.is_dir() {
        return files;
    }
    collect_files_recursive(src_dir, SOURCE_EXTS, &mut files);
    files.sort();
    files
}

fn collect_files_recursive(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
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

fn collect_include_edges(
    file: &Path,
    project_dir: &Path,
    include_dirs: &[PathBuf],
    show_system: bool,
    edges: &mut Vec<GraphEdge>,
    seen_edges: &mut HashSet<(String, String)>,
    in_stack: &mut HashSet<PathBuf>,
) {
    let from = file
        .strip_prefix(project_dir)
        .unwrap_or(file)
        .display()
        .to_string();
    for (inc, is_system) in parse_includes(file) {
        if is_system && !show_system {
            continue;
        }
        let to = if is_system {
            format!("<{inc}>")
        } else {
            match resolve_include(&inc, file, include_dirs) {
                Some(ref resolved) => resolved
                    .strip_prefix(project_dir)
                    .unwrap_or(resolved)
                    .display()
                    .to_string(),
                None => format!("\"{inc}\" (not found)"),
            }
        };
        let key = (from.clone(), to.clone());
        if seen_edges.insert(key) {
            edges.push(GraphEdge {
                from: from.clone(),
                to: to.clone(),
            });
            if !is_system {
                if let Some(resolved) = resolve_include(&inc, file, include_dirs) {
                    if !in_stack.contains(&resolved) {
                        in_stack.insert(resolved.clone());
                        collect_include_edges(
                            &resolved,
                            project_dir,
                            include_dirs,
                            show_system,
                            edges,
                            seen_edges,
                            in_stack,
                        );
                        in_stack.remove(&resolved);
                    }
                }
            }
        }
    }
}

fn parse_includes(path: &Path) -> Vec<(String, bool)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    // Shared directive parser (handles block comments, #import, header units);
    // keep only header-bringing directives, dropping named-module imports.
    use freight::build::include_policy::{parse_includes as parse, DirectiveKind};
    parse(&text)
        .into_iter()
        .filter(|d| d.kind == DirectiveKind::Header)
        .map(|d| (d.name, d.angled))
        .collect()
}

fn resolve_include(include: &str, from_file: &Path, include_dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(parent) = from_file.parent() {
        let candidate = parent.join(include);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for dir in include_dirs {
        let candidate = dir.join(include);
        if candidate.is_file() {
            return Some(candidate);
        }
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
    let visible: Vec<_> = includes
        .iter()
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
                println!(
                    "{}{} {}",
                    branch,
                    format!("\"{inc}\"").yellow(),
                    "[not found]".red()
                );
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

                println!(
                    "{}{} {}",
                    branch,
                    format!("\"{inc}\"").yellow(),
                    rel.display().to_string().bright_black()
                );
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
