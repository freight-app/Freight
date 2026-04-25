use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::error::CraneError;
use crate::manifest::{load_manifest, types::{Dependency, Manifest}};

// ── Public types ──────────────────────────────────────────────────────────────

/// A dependency resolved to a local directory, ready to be compiled.
pub struct ResolvedDep {
    pub name: String,
    /// Absolute path to the dep's source root (contains crane.toml and src/).
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Walk the dependency tree of `root_manifest` and return all compilable deps
/// in topological build order (leaves — packages with no deps of their own — first).
///
/// - Version deps (`name = "0.3"`) → resolved from `.deps/{name}/`
/// - Path deps (`path = "..."`) → resolved from that path
/// - System deps (`system = "..."`) → skipped (linked by name, no source)
/// - Git deps → resolved from `.deps/{name}/` after `crane fetch`
///
/// Only one level of availability is checked: if dep A requires dep B, B must
/// already be present in `.deps/`. Crane refuses to download transitively —
/// run `crane fetch` to populate `.deps/` before building.
pub fn resolve_dep_graph(
    root_dir: &Path,
    root_manifest: &Manifest,
    include_dev: bool,
) -> Result<Vec<ResolvedDep>, CraneError> {
    let mut nodes: HashMap<String, (PathBuf, Manifest)> = HashMap::new();
    let mut deps_of: HashMap<String, Vec<String>> = HashMap::new();

    let initial = direct_compilable_deps(root_dir, root_manifest, include_dev);
    let mut queue: VecDeque<(String, PathBuf)> = initial.into_iter().collect();

    while let Some((name, dir)) = queue.pop_front() {
        if nodes.contains_key(&name) { continue; }

        if !dir.exists() {
            return Err(CraneError::ManifestParse(format!(
                "dependency '{name}' not found at '{}'. \
                 Run `crane fetch` to download missing dependencies.",
                dir.display(),
            )));
        }

        let manifest = load_manifest(&dir)
            .map_err(|e| CraneError::ManifestParse(format!("dep '{name}': {e}")))?;

        // Check sub-deps are available; we don't download them, but we do walk them.
        let sub_deps = direct_compilable_deps(&dir, &manifest, false);
        for (sub_name, sub_dir) in &sub_deps {
            if !sub_dir.exists() {
                return Err(CraneError::ManifestParse(format!(
                    "dep '{name}' requires '{sub_name}', which is not in .deps/. \
                     Run `crane fetch` to download all dependencies.",
                )));
            }
        }

        let sub_names: Vec<String> = sub_deps.iter().map(|(n, _)| n.clone()).collect();
        deps_of.insert(name.clone(), sub_names);
        nodes.insert(name, (dir, manifest));

        for sub in sub_deps {
            queue.push_back(sub);
        }
    }

    let build_order = topo_sort(&nodes.keys().cloned().collect::<Vec<_>>(), &deps_of)?;

    // Move out of the HashMap in sorted order.
    let mut remaining = nodes;
    Ok(build_order.into_iter().map(|name| {
        let (dir, manifest) = remaining.remove(&name).unwrap();
        ResolvedDep { name, dir, manifest }
    }).collect())
}

/// Absolute include directories that `dep` exports to its dependants.
///
/// Auto-detects `include/` and `inc/` at the dep root (both common conventions),
/// then appends any paths from `[compiler].includes`.
pub fn dep_include_dirs(dep_dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for candidate in &["include", "inc"] {
        let d = dep_dir.join(candidate);
        if d.is_dir() {
            dirs.push(d);
        }
    }
    for p in &manifest.compiler.includes.paths {
        let abs = dep_dir.join(p);
        if !dirs.contains(&abs) {
            dirs.push(abs);
        }
    }
    dirs
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn direct_compilable_deps(
    project_dir: &Path,
    manifest: &Manifest,
    include_dev: bool,
) -> Vec<(String, PathBuf)> {
    let mut result: Vec<(String, PathBuf)> = Vec::new();
    for (name, dep) in manifest.effective_dependencies() {
        if let Some(dir) = compilable_dep_dir(project_dir, &name, &dep) {
            result.push((name, dir));
        }
    }
    if include_dev {
        for (name, dep) in &manifest.dev_dependencies {
            if result.iter().any(|(n, _)| n == name) { continue; }
            if let Some(dir) = compilable_dep_dir(project_dir, name, dep) {
                result.push((name.clone(), dir));
            }
        }
    }
    result
}

fn compilable_dep_dir(project_dir: &Path, name: &str, dep: &Dependency) -> Option<PathBuf> {
    match dep {
        Dependency::Simple(_) => {
            // Version dep → .deps/{name}/
            Some(project_dir.join(".deps").join(name))
        }
        Dependency::Detailed(d) => {
            if d.system.is_some() { return None; }
            // Foreign deps are built by their own build system (build/foreign.rs).
            if d.build_system.is_some() { return None; }
            if d.git.is_some() {
                // Git dep lands in .deps/{name}/ after `crane fetch`
                return Some(project_dir.join(".deps").join(name));
            }
            if let Some(path) = &d.path {
                return Some(project_dir.join(path));
            }
            None
        }
    }
}

// ── Topological sort (Kahn's algorithm) ──────────────────────────────────────

pub(crate) fn topo_sort(
    names: &[String],
    deps_of: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, CraneError> {
    // in_degree[A] = number of packages A depends on
    let mut in_degree: HashMap<String, usize> = names.iter()
        .map(|n| (n.clone(), deps_of.get(n).map_or(0, |v| v.len())))
        .collect();

    // rev_adj[B] = packages that depend on B (so when B is ready, decrement their count)
    let mut rev_adj: HashMap<String, Vec<String>> = HashMap::new();
    for name in names {
        for dep in deps_of.get(name).into_iter().flatten() {
            rev_adj.entry(dep.clone()).or_default().push(name.clone());
        }
    }

    // Start with packages that have no dependencies (leaves).
    let mut queue: VecDeque<String> = in_degree.iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    // Sort for deterministic order when multiple leaves are ready at once.
    let mut sorted_queue: Vec<String> = queue.drain(..).collect();
    sorted_queue.sort();
    queue.extend(sorted_queue);

    let mut result: Vec<String> = Vec::new();

    while let Some(node) = queue.pop_front() {
        result.push(node.clone());
        let mut next: Vec<String> = Vec::new();
        for dependant in rev_adj.get(&node).into_iter().flatten() {
            let d = in_degree.get_mut(dependant).unwrap();
            *d -= 1;
            if *d == 0 {
                next.push(dependant.clone());
            }
        }
        next.sort();
        queue.extend(next);
    }

    if result.len() != names.len() {
        return Err(CraneError::DependencyCycle(
            "circular dependency detected among local packages".into(),
        ));
    }

    Ok(result)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn deps(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs.iter().map(|(k, v)| {
            (k.to_string(), v.iter().map(|s| s.to_string()).collect())
        }).collect()
    }

    #[test]
    fn topo_sort_no_deps() {
        let order = topo_sort(&names(&["a", "b", "c"]), &HashMap::new()).unwrap();
        assert_eq!(order.len(), 3);
        // No ordering constraint — just all present
        assert!(order.contains(&"a".to_string()));
        assert!(order.contains(&"b".to_string()));
        assert!(order.contains(&"c".to_string()));
    }

    #[test]
    fn topo_sort_linear_chain() {
        // c depends on b, b depends on a → order: a, b, c
        let d = deps(&[("b", &["a"]), ("c", &["b"])]);
        let order = topo_sort(&names(&["a", "b", "c"]), &d).unwrap();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("a") < pos("b"), "a before b");
        assert!(pos("b") < pos("c"), "b before c");
    }

    #[test]
    fn topo_sort_diamond() {
        // d depends on b and c; b and c both depend on a → a is first, d is last
        let d = deps(&[
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b", "c"]),
        ]);
        let order = topo_sort(&names(&["a", "b", "c", "d"]), &d).unwrap();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn topo_sort_independent_deps_are_sorted_deterministically() {
        // a and b are both independent leaves — sorted alphabetically
        let order = topo_sort(&names(&["b", "a"]), &HashMap::new()).unwrap();
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn topo_sort_cycle_returns_error() {
        let d = deps(&[("a", &["b"]), ("b", &["a"])]);
        assert!(matches!(
            topo_sort(&names(&["a", "b"]), &d),
            Err(CraneError::DependencyCycle(_))
        ));
    }

    #[test]
    fn dep_include_dirs_returns_inc_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("inc")).unwrap();
        let manifest = crate::manifest::load_manifest_str(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n[[bin]]\nname=\"p\"\nsrc=\"src/main.cpp\"\n"
        ).unwrap();
        let dirs = dep_include_dirs(dir.path(), &manifest);
        assert!(dirs.iter().any(|d| d.ends_with("inc")));
    }

    #[test]
    fn dep_include_dirs_empty_when_no_inc() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = crate::manifest::load_manifest_str(
            "[package]\nname=\"p\"\nversion=\"0.1.0\"\n[language.cpp]\n[[bin]]\nname=\"p\"\nsrc=\"src/main.cpp\"\n"
        ).unwrap();
        let dirs = dep_include_dirs(dir.path(), &manifest);
        assert!(dirs.is_empty());
    }
}
