//! `freight workspace` — workspace-level inspection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use freight::manifest::types::Dependency;
use freight::manifest::{load_manifest, load_workspace_manifest};

use crate::output::{
    print_error, render_dot_graph, render_mermaid_graph, GraphEdge, GraphFormat,
};
use owo_colors::OwoColorize;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Sub,
}

#[derive(clap::Subcommand)]
enum Sub {
    /// Visualise inter-member dependency relationships in the workspace.
    Graph {
        /// Output format: text (default), mermaid, dot.
        #[arg(long, short = 'f', default_value = "text", value_name = "FORMAT")]
        format: String,
    },
}

impl Args {
    pub fn run(self) {
        match self.cmd {
            Sub::Graph { format } => cmd_graph(&format),
        }
    }
}

/// Walk up from `cwd` to find the workspace-root `freight.toml` (one with a
/// `[workspace]` section) and return `(root_dir, members)`.
fn find_workspace() -> Option<(PathBuf, Vec<String>)> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if let Some(ws) = load_workspace_manifest(&dir) {
            return Some((dir, ws.members));
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn cmd_graph(format: &str) {
    let Some((root, members)) = find_workspace() else {
        print_error("no [workspace] manifest found (run inside a workspace)");
        return;
    };

    // member dir → package name, and canonical dir → name (for resolving path deps).
    let mut name_by_canon: HashMap<PathBuf, String> = HashMap::new();
    let mut nodes: Vec<(String, PathBuf)> = Vec::new(); // (name, dir), declaration order
    for member in &members {
        let dir = root.join(member.trim_end_matches('/'));
        let name = load_manifest(&dir)
            .map(|m| m.package.name)
            .unwrap_or_else(|_| member.clone());
        name_by_canon.insert(canon(&dir), name.clone());
        nodes.push((name, dir));
    }

    // Edge dependent → dependency for every path dep that points at another member.
    let mut edges: Vec<GraphEdge> = Vec::new();
    for (name, dir) in &nodes {
        let Ok(manifest) = load_manifest(dir) else {
            continue;
        };
        for (dep_key, dep) in manifest.effective_dependencies() {
            let Dependency::Detailed(d) = &dep else {
                continue;
            };
            let Some(rel) = &d.path else { continue };
            let target_canon = canon(&dir.join(rel));
            if let Some(target) = name_by_canon.get(&target_canon) {
                if target != name {
                    edges.push(GraphEdge {
                        from: name.clone(),
                        to: target.clone(),
                    });
                }
            } else {
                let _ = dep_key; // path dep outside the workspace — not an inter-member edge
            }
        }
    }

    let node_names: Vec<String> = nodes.iter().map(|(n, _)| n.clone()).collect();
    match GraphFormat::parse(format) {
        GraphFormat::Mermaid => render_mermaid_graph("workspace", &[], &edges, &node_names),
        GraphFormat::Dot => render_dot_graph("workspace", &[], &edges, &node_names),
        GraphFormat::Text => render_text(&node_names, &edges),
    }
}

/// Plain-text adjacency: each member, then the members it depends on.
fn render_text(nodes: &[String], edges: &[GraphEdge]) {
    println!("{}", "workspace members".bold());
    for n in nodes {
        let deps: Vec<&str> = edges
            .iter()
            .filter(|e| &e.from == n)
            .map(|e| e.to.as_str())
            .collect();
        if deps.is_empty() {
            println!("  {n}");
        } else {
            println!("  {n} {} {}", "→".dimmed(), deps.join(", "));
        }
    }
    if edges.is_empty() {
        println!("\n{}", "(no inter-member path dependencies)".dimmed());
    }
}
