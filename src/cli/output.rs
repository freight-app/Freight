//! Coloured CLI output helpers — used by every `cmd_*` shell so they speak in
//! the same voice. Lives in the binary because the library has no business
//! formatting for a terminal.

use std::sync::atomic::{AtomicBool, Ordering};

use owo_colors::OwoColorize;

/// Set to `true` by `print_error`; checked by `main()` to decide the process
/// exit code.  Using an atomic avoids threading a `&mut bool` through every
/// command function.
static HAD_ERROR: AtomicBool = AtomicBool::new(false);

/// Returns `true` if any call to `print_error` has occurred in this process.
pub fn had_error() -> bool {
    HAD_ERROR.load(Ordering::Relaxed)
}

pub fn print_success(msg: &str) {
    println!("{} {}", "✓".green().bold(), msg);
}

pub fn print_warning(msg: &str) {
    eprintln!("{} {}", "⚠".yellow().bold(), msg);
}

pub fn print_error(msg: &str) {
    HAD_ERROR.store(true, Ordering::Relaxed);
    eprintln!("{} {}", "✗".red().bold(), msg);
}

pub fn print_status(verb: &str, detail: &str) {
    println!("{:>12} {}", verb.cyan().bold(), detail);
}

/// A line of output captured from a build-plugin tool. `source` is the tool
/// name; stderr lines are tinted to stand out.
pub fn print_script_output(source: &str, text: &str, is_err: bool) {
    let tag = format!("{:>12}", format!("[{source}]"));
    if is_err {
        eprintln!("{} {}", tag.yellow(), text);
    } else {
        println!("{} {}", tag.dimmed(), text);
    }
}

// ── Graph output formats ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    Text,
    Mermaid,
    Dot,
}

impl GraphFormat {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "mermaid" | "md" => Self::Mermaid,
            "dot" | "graphviz" => Self::Dot,
            _ => Self::Text,
        }
    }
}

/// An edge in a directed graph: `from → to`.
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

/// A named group of nodes (used for stages / subgraphs).
pub struct GraphCluster {
    pub id: String,
    pub label: String,
    pub nodes: Vec<String>,
}

/// Render an include/dependency graph in Mermaid format.
pub fn render_mermaid_graph(
    title: &str,
    clusters: &[GraphCluster],
    edges: &[GraphEdge],
    ungrouped: &[String],
) {
    println!("```mermaid");
    println!("---");
    println!("title: {title}");
    println!("---");
    println!("graph LR");

    for cluster in clusters {
        println!(
            "    subgraph {}[\"{}\"]",
            mermaid_id(&cluster.id),
            cluster.label
        );
        for node in &cluster.nodes {
            println!("        {}[\"{}\"]", mermaid_id(node), node);
        }
        println!("    end");
    }
    for node in ungrouped {
        println!("    {}[\"{}\"]", mermaid_id(node), node);
    }
    for edge in edges {
        println!(
            "    {} --> {}",
            mermaid_id(&edge.from),
            mermaid_id(&edge.to)
        );
    }
    println!("```");
}

/// Render a graph in Graphviz DOT format.
pub fn render_dot_graph(
    title: &str,
    clusters: &[GraphCluster],
    edges: &[GraphEdge],
    ungrouped: &[String],
) {
    println!("digraph {} {{", dot_id(title));
    println!("    rankdir=LR");
    println!("    node [shape=box style=filled fillcolor=white]");

    for (i, cluster) in clusters.iter().enumerate() {
        println!("    subgraph cluster_{i} {{");
        println!("        label=\"{}\"", cluster.label);
        println!("        style=filled fillcolor=\"#f0f0f0\"");
        for node in &cluster.nodes {
            println!("        {} [label=\"{}\"]", dot_id(node), node);
        }
        println!("    }}");
    }
    for node in ungrouped {
        println!("    {} [label=\"{}\"]", dot_id(node), node);
    }
    for edge in edges {
        println!("    {} -> {}", dot_id(&edge.from), dot_id(&edge.to));
    }
    println!("}}");
}

fn mermaid_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn dot_id(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}
