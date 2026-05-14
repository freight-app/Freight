use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::error::FreightError;
use crate::manifest::{
    load_manifest,
    types::{Dependency, Manifest},
};

/// Output syntaxes supported by `freight graph`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GraphFormat {
    Dot,
    Mermaid,
}

impl GraphFormat {
    pub fn parse(value: &str) -> Result<Self, FreightError> {
        match value.to_ascii_lowercase().as_str() {
            "dot" => Ok(Self::Dot),
            "mermaid" | "mmd" => Ok(Self::Mermaid),
            other => Err(FreightError::ManifestParse(format!(
                "unknown graph format '{other}'; expected 'dot' or 'mermaid'"
            ))),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DependencyGraph {
    nodes: BTreeMap<String, GraphNode>,
    edges: BTreeSet<GraphEdge>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct GraphNode {
    label: String,
    version: Option<String>,
    root: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct GraphEdge {
    from: String,
    to: String,
    kind: String,
}

/// Build a dependency graph starting at `root_manifest`.
///
/// The graph always includes direct manifest dependencies. Transitive
/// dependencies are expanded for Freight manifests that are available locally:
/// path dependencies are loaded from their path, while git dependencies are
/// loaded from the root project's `.deps/{name}` directory when already fetched.
/// System/package/url dependencies are represented as leaves because they do
/// not have local Freight manifests to inspect.
pub fn build_dependency_graph(
    root_dir: &Path,
    root_manifest: &Manifest,
    include_dev: bool,
) -> DependencyGraph {
    let mut graph = DependencyGraph {
        nodes: BTreeMap::new(),
        edges: BTreeSet::new(),
    };

    let root_id = node_id(&root_manifest.package.name, 0);
    graph.nodes.insert(
        root_id.clone(),
        GraphNode {
            label: root_manifest.package.name.clone(),
            version: Some(root_manifest.package.version.clone()),
            root: true,
        },
    );

    let mut queue = VecDeque::from([(
        root_id,
        root_dir.to_path_buf(),
        root_manifest.clone(),
        0usize,
    )]);
    let mut expanded = BTreeSet::new();
    let mut expanded_dirs = BTreeSet::new();

    while let Some((parent_id, declaring_dir, manifest, depth)) = queue.pop_front() {
        if !expanded.insert(parent_id.clone()) {
            continue;
        }

        let declaring_key = canonical_or_original(&declaring_dir);
        if !expanded_dirs.insert(declaring_key) {
            continue;
        }

        for (name, dep, is_dev) in sorted_dependencies(&manifest, include_dev && depth == 0) {
            let kind = dependency_kind(&dep, is_dev);
            let child_id = node_id(&name, depth + 1);
            let (child_label, child_version, child_manifest, child_dir) =
                inspect_local_dep(root_dir, &declaring_dir, &name, &dep)
                    .map(|(dir, child_manifest)| {
                        (
                            child_manifest.package.name.clone(),
                            Some(child_manifest.package.version.clone()),
                            Some(child_manifest),
                            Some(dir),
                        )
                    })
                    .unwrap_or_else(|| (name.clone(), dependency_version(&dep), None, None));

            graph.nodes.entry(child_id.clone()).or_insert(GraphNode {
                label: child_label,
                version: child_version,
                root: false,
            });
            graph.edges.insert(GraphEdge {
                from: parent_id.clone(),
                to: child_id.clone(),
                kind,
            });

            if let (Some(child_manifest), Some(child_dir)) = (child_manifest, child_dir) {
                queue.push_back((child_id, child_dir, child_manifest, depth + 1));
            }
        }
    }

    graph
}

impl DependencyGraph {
    pub fn render(&self, format: GraphFormat) -> String {
        match format {
            GraphFormat::Dot => self.render_dot(),
            GraphFormat::Mermaid => self.render_mermaid(),
        }
    }

    fn render_dot(&self) -> String {
        let mut out = String::from("digraph freight {\n  rankdir=LR;\n");
        for (id, node) in &self.nodes {
            let shape = if node.root { "box" } else { "ellipse" };
            let _ = writeln!(
                out,
                "  \"{}\" [label=\"{}\", shape={}];",
                dot_escape(id),
                dot_escape(&node_label(node, "\\n")),
                shape,
            );
        }
        for edge in &self.edges {
            let _ = writeln!(
                out,
                "  \"{}\" -> \"{}\" [label=\"{}\"];",
                dot_escape(&edge.from),
                dot_escape(&edge.to),
                dot_escape(&edge.kind),
            );
        }
        out.push_str("}\n");
        out
    }

    fn render_mermaid(&self) -> String {
        let mut out = String::from("flowchart LR\n");
        for (id, node) in &self.nodes {
            let shape = if node.root { ('[', ']') } else { ('(', ')') };
            let _ = writeln!(
                out,
                "  {}{}\"{}\"{}",
                mermaid_id(id),
                shape.0,
                mermaid_escape(&node_label(node, "<br/>")),
                shape.1,
            );
        }
        for edge in &self.edges {
            let _ = writeln!(
                out,
                "  {} -->|{}| {}",
                mermaid_id(&edge.from),
                mermaid_escape(&edge.kind),
                mermaid_id(&edge.to),
            );
        }
        out
    }
}

fn sorted_dependencies(manifest: &Manifest, include_dev: bool) -> Vec<(String, Dependency, bool)> {
    let mut deps: Vec<_> = manifest
        .effective_dependencies()
        .into_iter()
        .map(|(name, dep)| (name, dep, false))
        .collect();

    if include_dev {
        deps.extend(
            manifest
                .dev_dependencies
                .iter()
                .map(|(name, dep)| (name.clone(), dep.clone(), true)),
        );
    }

    deps.sort_by(|(left, _, left_dev), (right, _, right_dev)| {
        left.cmp(right).then(left_dev.cmp(right_dev))
    });
    deps
}

fn inspect_local_dep(
    root_dir: &Path,
    declaring_dir: &Path,
    name: &str,
    dep: &Dependency,
) -> Option<(PathBuf, Manifest)> {
    let Dependency::Detailed(details) = dep else {
        return None;
    };

    if details.system.is_some() || details.url.is_some() || details.backend.is_some() {
        return None;
    }

    let dir = if let Some(path) = &details.path {
        declaring_dir.join(path)
    } else if details.git.is_some() {
        root_dir.join(".deps").join(name)
    } else {
        return None;
    };

    load_manifest(&dir).ok().map(|manifest| (dir, manifest))
}

fn dependency_kind(dep: &Dependency, is_dev: bool) -> String {
    let base = match dep {
        Dependency::Simple(_) => "package",
        Dependency::Detailed(details) if details.path.is_some() => "path",
        Dependency::Detailed(details) if details.git.is_some() => "git",
        Dependency::Detailed(details) if details.url.is_some() => "url",
        Dependency::Detailed(details)
            if details.system.is_some() || details.pkg_config.is_some() =>
        {
            "system"
        }
        Dependency::Detailed(_) => "package",
    };
    if is_dev {
        format!("dev {base}")
    } else {
        base.to_string()
    }
}

fn dependency_version(dep: &Dependency) -> Option<String> {
    match dep {
        Dependency::Simple(version) => Some(version.clone()),
        Dependency::Detailed(details) => details.version.clone(),
    }
}

fn node_id(name: &str, depth: usize) -> String {
    format!("{depth}:{name}")
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn node_label(node: &GraphNode, newline: &str) -> String {
    match &node.version {
        Some(version) if !version.is_empty() => format!("{}{}{}", node.label, newline, version),
        _ => node.label.clone(),
    }
}

fn dot_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn mermaid_escape(value: &str) -> String {
    value.replace('"', "#quot;").replace('|', "#124;")
}

fn mermaid_id(value: &str) -> String {
    let mut out = String::from("n");
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn manifest(src: &str) -> Manifest {
        crate::manifest::load_manifest_str(src).unwrap()
    }

    #[test]
    fn renders_direct_deps_as_dot() {
        let root = manifest(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = "1.2"
            ssl = { system = "ssl" }
            "#,
        );

        let graph = build_dependency_graph(Path::new("."), &root, false);
        let dot = graph.render(GraphFormat::Dot);

        assert!(dot.contains("digraph freight"));
        assert!(dot.contains("\"0:app\" -> \"1:ssl\" [label=\"system\"]"));
        assert!(dot.contains("\"0:app\" -> \"1:zlib\" [label=\"package\"]"));
    }

    #[test]
    fn expands_local_path_deps() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("lib")).unwrap();
        fs::write(
            tmp.path().join("lib/freight.toml"),
            r#"
            [package]
            name = "lib"
            version = "0.2.0"

            [dependencies]
            leaf = "1"
            "#,
        )
        .unwrap();

        let root = manifest(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            lib = { path = "lib" }
            "#,
        );

        let graph = build_dependency_graph(tmp.path(), &root, false);
        let mermaid = graph.render(GraphFormat::Mermaid);

        assert!(mermaid.contains("n0_app -->|path| n1_lib"));
        assert!(mermaid.contains("n1_lib -->|package| n2_leaf"));
    }
}
