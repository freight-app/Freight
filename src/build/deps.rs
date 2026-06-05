use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::error::FreightError;
use crate::manifest::{
    load_manifest,
    types::{Dependency, Manifest},
};

// ── Public types ──────────────────────────────────────────────────────────────

/// A dependency resolved to a local directory, ready to be compiled.
pub struct ResolvedDep {
    pub name: String,
    /// Absolute path to the dep's source root (contains freight.toml and src/).
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// Distance from the root project: 1 = direct dep, 2 = dep's dep, etc.
    pub depth: usize,
}

/// Walk the dependency tree of `root_manifest` and return all compilable deps
/// in topological build order (leaves — packages with no deps of their own — first).
///
/// - Version deps (`name = "0.3"`) → resolved via pkg-config, system stubs, or registry
/// - Path deps (`path = "..."`) → resolved from that path
/// - System deps (`system = "..."`) → skipped (linked by name, no source)
/// - Git deps → resolved from `.pkgs/{name}/` after `freight fetch`
///
/// Only one level of availability is checked: if dep A requires dep B, B must
/// already be present in `.pkgs/`. Freight refuses to download transitively —
/// run `freight fetch` to populate `.pkgs/` before building.
pub fn resolve_dep_graph(
    root_dir: &Path,
    root_manifest: &Manifest,
    include_dev: bool,
    activated_deps: &BTreeSet<String>,
) -> Result<Vec<ResolvedDep>, FreightError> {
    let mut nodes: HashMap<String, (PathBuf, Manifest)> = HashMap::new();
    let mut deps_of: HashMap<String, Vec<String>> = HashMap::new();
    let mut depths: HashMap<String, usize> = HashMap::new();

    let initial = direct_compilable_deps(
        root_dir,
        root_dir,
        root_manifest,
        include_dev,
        activated_deps,
    );
    // Queue carries (name, dir, depth): direct deps of root start at depth 1.
    let mut queue: VecDeque<(String, PathBuf, usize)> =
        initial.into_iter().map(|(n, d)| (n, d, 1)).collect();

    while let Some((name, dir, depth)) = queue.pop_front() {
        if nodes.contains_key(&name) {
            continue;
        }

        if !dir.exists() {
            return Err(FreightError::ManifestParse(format!(
                "dependency '{name}' not found at '{}'. \
                 Run `freight fetch` to download missing dependencies.",
                dir.display(),
            )));
        }

        let manifest = load_manifest(&dir)
            .map_err(|e| FreightError::ManifestParse(format!("dep '{name}': {e}")))?;

        // All deps — including transitive ones — live in the root project's flat
        // .pkgs/ pool, not in a nested .pkgs/ inside each dep.  Path deps in a
        // transitive manifest are relative to that dep's own directory, but
        // version/git deps always resolve against root_dir.
        let empty = BTreeSet::new();
        let sub_deps = direct_compilable_deps(root_dir, &dir, &manifest, false, &empty);
        for (sub_name, sub_dir) in &sub_deps {
            if !sub_dir.exists() {
                return Err(FreightError::ManifestParse(format!(
                    "dep '{name}' requires '{sub_name}', which is not present. \
                     Run `freight fetch` to download missing dependencies.",
                )));
            }
        }

        let sub_names: Vec<String> = sub_deps.iter().map(|(n, _)| n.clone()).collect();
        deps_of.insert(name.clone(), sub_names);
        depths.insert(name.clone(), depth);
        nodes.insert(name, (dir, manifest));

        for sub in sub_deps {
            queue.push_back((sub.0, sub.1, depth + 1));
        }
    }

    let build_order = topo_sort(&nodes.keys().cloned().collect::<Vec<_>>(), &deps_of)?;

    let mut remaining = nodes;
    Ok(build_order
        .into_iter()
        .map(|name| {
            let depth = depths[&name];
            let (dir, manifest) = remaining.remove(&name).unwrap();
            ResolvedDep {
                name,
                dir,
                manifest,
                depth,
            }
        })
        .collect())
}

/// Check slot conflicts among active deps and apply hierarchy-based substitution.
///
/// Called after `resolve_dep_graph` with the full resolved list plus the root
/// manifest itself (depth 0, always wins).
///
/// - Different depths → shallower dep wins; deeper dep is added to the returned
///   drop list and a note is printed to stderr.
/// - Same depth → hard error: neither has priority, the user must resolve it.
///
/// Returns the list of dep names that should be dropped from the build.
pub fn check_slot_conflicts(
    resolved: &[ResolvedDep],
    root_manifest: &Manifest,
) -> Result<Vec<String>, FreightError> {
    // slot → (dep_name, depth); depth 0 = root project (always wins)
    let mut claimed: HashMap<String, (String, usize)> = HashMap::new();
    let mut to_drop: Vec<String> = Vec::new();

    for slot in &root_manifest.package.provides {
        claimed.insert(slot.clone(), (root_manifest.package.name.clone(), 0));
    }

    for dep in resolved {
        for slot in &dep.manifest.package.provides {
            if let Some((existing_name, existing_depth)) = claimed.get(slot).cloned() {
                if dep.depth < existing_depth {
                    // This dep is closer to root — it wins; drop the previous claimant.
                    eprintln!(
                        "note: using '{}' instead of '{}' (both provide '{}')",
                        dep.name, existing_name, slot,
                    );
                    if !to_drop.contains(&existing_name) {
                        to_drop.push(existing_name.clone());
                    }
                    claimed.insert(slot.clone(), (dep.name.clone(), dep.depth));
                } else if dep.depth > existing_depth {
                    // Existing dep is closer to root — keep it; drop this dep.
                    eprintln!(
                        "note: using '{}' instead of '{}' (both provide '{}')",
                        existing_name, dep.name, slot,
                    );
                    if !to_drop.contains(&dep.name) {
                        to_drop.push(dep.name.clone());
                    }
                } else {
                    // Same depth — true conflict, user must resolve.
                    return Err(FreightError::SlotConflict(
                        existing_name.clone(),
                        dep.name.clone(),
                        slot.clone(),
                    ));
                }
            } else {
                claimed.insert(slot.clone(), (dep.name.clone(), dep.depth));
            }
        }
    }

    Ok(to_drop)
}

/// Absolute include directories that `dep` exports to its dependants.
///
/// If the lib declares `hdrs`, derive include dirs from the parent directories
/// of those headers. Otherwise auto-detect `include/` and `inc/` at the dep root.
/// Always appends any paths from `[compiler].includes`.
pub fn dep_include_dirs(dep_dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    let hdrs_declared = manifest
        .lib
        .as_ref()
        .map(|l| !l.hdrs.is_empty())
        .unwrap_or(false);
    if hdrs_declared {
        for hdr in &manifest.lib.as_ref().unwrap().hdrs {
            if let Some(parent) = std::path::Path::new(hdr).parent() {
                if !parent.as_os_str().is_empty() {
                    let abs = dep_dir.join(parent);
                    if abs.is_dir() && !dirs.contains(&abs) {
                        dirs.push(abs);
                    }
                }
            }
        }
    } else {
        for candidate in &["include", "inc"] {
            let d = dep_dir.join(candidate);
            if d.is_dir() {
                dirs.push(d);
            }
        }
    }

    for p in &manifest.compiler.includes {
        let abs = dep_dir.join(p);
        if !dirs.contains(&abs) {
            dirs.push(abs);
        }
    }
    dirs
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// `root_dir`      — the root project's directory; version/git deps resolve to
///                   `root_dir/.pkgs/{name}/` (flat, shared pool).
/// `declaring_dir` — the directory of the manifest that declares this dep;
///                   path deps (`path = "../foo"`) are relative to this.
///
/// For the root project both are the same. For transitive deps, `root_dir`
/// stays fixed while `declaring_dir` is the dep's own directory.
fn direct_compilable_deps(
    root_dir: &Path,
    declaring_dir: &Path,
    manifest: &Manifest,
    include_dev: bool,
    activated_deps: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    let mut result: Vec<(String, PathBuf)> = Vec::new();
    for (name, dep) in manifest.effective_dependencies() {
        // Skip optional deps that haven't been activated via a `dep:name` feature entry.
        if let Dependency::Detailed(ref d) = dep {
            if d.optional && !activated_deps.contains(&name) {
                continue;
            }
        }
        if let Some(dir) = compilable_dep_dir(root_dir, declaring_dir, &name, &dep) {
            result.push((name, dir));
        }
    }
    if include_dev {
        for (name, dep) in &manifest.dev_dependencies {
            if result.iter().any(|(n, _)| n == name) {
                continue;
            }
            if let Some(dir) = compilable_dep_dir(root_dir, declaring_dir, name, dep) {
                result.push((name.clone(), dir));
            }
        }
    }
    result
}

fn compilable_dep_dir(
    root_dir: &Path,
    declaring_dir: &Path,
    name: &str,
    dep: &Dependency,
) -> Option<PathBuf> {
    match dep {
        Dependency::Simple(_) => {
            // Version dep → resolved by foreign package lookup (pkg-config → system stubs → registry).
            None
        }
        Dependency::Detailed(d) => {
            if crate::manifest::types::is_platform_dep(name) {
                return None;
            }
            let dep_dir = if d.is_git() {
                // Git dep → root .deps/{name}/ (flat pool)
                root_dir.join(".pkgs").join(name)
            } else if let Some(p) = &d.path {
                // Path dep → relative to the manifest that declares it
                declaring_dir.join(p)
            } else {
                return None;
            };
            // Explicitly foreign, or auto-detected as foreign (and not a freight project).
            if d.dep_type.is_some()
                || (!(d.path.is_some() && dep_dir.join("freight.toml").exists())
                    && crate::adaptors::detect_build_system(&dep_dir).is_some())
            {
                return None;
            }
            Some(dep_dir)
        }
    }
}

// ── Topological sort (Kahn's algorithm) ──────────────────────────────────────

pub(crate) fn topo_sort(
    names: &[String],
    deps_of: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, FreightError> {
    // in_degree[A] = number of packages A depends on
    let mut in_degree: HashMap<String, usize> = names
        .iter()
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
    let mut queue: VecDeque<String> = in_degree
        .iter()
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
        return Err(FreightError::DependencyCycle(
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
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
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
        let d = deps(&[("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
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
            Err(FreightError::DependencyCycle(_))
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
