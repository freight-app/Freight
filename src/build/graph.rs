/// Flat package dependency graph.
///
/// All packages live in a single `HashMap<name, Arc<PackageNode>>`.  Edges are
/// stored on each node rather than as strong `Arc` links, so there are no
/// reference cycles and no tree traversal is needed to find the root.
///
/// # Resolution
///
/// `PackageGraph::insert` performs resolution at insertion time:
/// - If the name is new, the node is accepted.
/// - If it already exists, the incoming `version_req` must be compatible
///   (semver intersection). Features are unioned; conflicting defines are an error.
///
/// # Target directories
///
/// `PackageGraph::target_dir(name)`:
/// - Root package → `root_dir/target`
/// - Any dep      → `root_dir/target/deps/<name>`
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── DepKind ───────────────────────────────────────────────────────────────────

/// How a dep edge is classified in the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepKind {
    Build,
    Dev,
}

// ── DepRef ────────────────────────────────────────────────────────────────────

/// Reference from one package to another in the flat graph.
#[derive(Debug, Clone)]
pub struct DepRef {
    pub version_req: String,
    pub features: Vec<String>,
    pub defines: Vec<String>,
    /// `None` = normal; `Some(Build)` = build-dep; `Some(Dev)` = dev-dep.
    pub kind: Option<DepKind>,
}

impl DepRef {
    pub fn normal(version_req: impl Into<String>) -> Self {
        Self { version_req: version_req.into(), features: vec![], defines: vec![], kind: None }
    }

    pub fn build_dep(version_req: impl Into<String>) -> Self {
        Self { kind: Some(DepKind::Build), ..Self::normal(version_req) }
    }

    pub fn dev_dep(version_req: impl Into<String>) -> Self {
        Self { kind: Some(DepKind::Dev), ..Self::normal(version_req) }
    }
}

// ── PackageNode ───────────────────────────────────────────────────────────────

pub struct PackageNode {
    pub name: String,
    pub version: String,
    /// Absolute path to the package's source directory.
    pub dir: PathBuf,
    /// Immediate dep references keyed by dep name.
    pub deps: HashMap<String, DepRef>,
}

impl PackageNode {
    pub fn new(name: impl Into<String>, version: impl Into<String>, dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { name: name.into(), version: version.into(), dir, deps: HashMap::new() })
    }

    pub fn from_manifest_dir(
        dir: &Path,
        manifest: &crate::manifest::types::Manifest,
    ) -> Arc<Self> {
        Self::new(&manifest.package.name, &manifest.package.version, dir.to_path_buf())
    }
}

// ── PackageGraph ──────────────────────────────────────────────────────────────

pub struct PackageGraph {
    /// Name of the root project package (used to distinguish root vs dep target dirs).
    pub root_name: String,
    /// Source directory of the root project — anchors the flat `.pkgs/` pool.
    pub root_dir: PathBuf,
    /// Flat registry of all packages (root + deps). Key = package name.
    pub packages: HashMap<String, Arc<PackageNode>>,
}

impl PackageGraph {
    pub fn new(root: Arc<PackageNode>) -> Self {
        let root_name = root.name.clone();
        let root_dir = root.dir.clone();
        let mut packages = HashMap::new();
        packages.insert(root_name.clone(), root);
        Self { root_name, root_dir, packages }
    }

    /// Single-node graph for a top-level (root) project build.
    pub fn root_only(name: impl Into<String>, version: impl Into<String>, dir: PathBuf) -> Self {
        Self::new(PackageNode::new(name, version, dir))
    }

    /// Single-node graph for a dep source-build.
    ///
    /// `pkg_dir` is the dep's own source directory (e.g. `.pkgs/vecmath/`).
    /// `root_dir` is the root project directory that owns the flat `.pkgs/` pool.
    pub fn for_dep(
        name: impl Into<String>,
        version: impl Into<String>,
        pkg_dir: PathBuf,
        root_dir: PathBuf,
    ) -> Self {
        let name: String = name.into();
        let node = PackageNode::new(name.clone(), version, pkg_dir);
        let mut packages = HashMap::new();
        packages.insert(name.clone(), node);
        Self { root_name: name, root_dir, packages }
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    pub fn pkgs_dir(&self) -> PathBuf {
        self.root_dir.join(".pkgs")
    }

    pub fn dep_source_dir(&self, dep_name: &str) -> PathBuf {
        self.pkgs_dir().join(dep_name)
    }

    /// Build-artifact directory for `name`.
    pub fn target_dir(&self, name: &str) -> PathBuf {
        if name == self.root_name {
            self.root_dir.join("target")
        } else {
            self.root_dir.join("target").join("deps").join(name)
        }
    }

    pub fn root_target_dir(&self) -> PathBuf {
        self.target_dir(&self.root_name)
    }

    pub fn is_root(&self, name: &str) -> bool {
        name == self.root_name
    }

    // ── Insertion / resolution ────────────────────────────────────────────────

    /// Insert `node` as a dep of `parent_name`.  If a package with the same
    /// name already exists, checks semver compatibility and unions features.
    /// Returns `Ok(true)` if newly inserted, `Ok(false)` if already present.
    pub fn insert(
        &mut self,
        node: Arc<PackageNode>,
        parent_name: &str,
        dep_ref: DepRef,
    ) -> Result<bool, ResolveError> {
        let name = node.name.clone();
        if let Some(existing) = self.packages.get(&name) {
            check_version_compat(&name, &existing.version, &dep_ref.version_req)?;
            if let Some(parent) = Arc::get_mut(
                self.packages.get_mut(parent_name).expect("parent must be in graph"),
            ) {
                merge_dep_ref(&mut parent.deps, name.clone(), dep_ref)?;
            }
            Ok(false)
        } else {
            self.packages.insert(name.clone(), node);
            if let Some(parent) = Arc::get_mut(
                self.packages.get_mut(parent_name).expect("parent must be in graph"),
            ) {
                merge_dep_ref(&mut parent.deps, name.clone(), dep_ref)?;
            }
            Ok(true)
        }
    }

    pub fn add_dep_node(
        &mut self,
        node: Arc<PackageNode>,
        parent_name: &str,
        version_req: impl Into<String>,
        kind: Option<DepKind>,
    ) -> Result<bool, ResolveError> {
        self.insert(node, parent_name, DepRef { version_req: version_req.into(), features: vec![], defines: vec![], kind })
    }

    // ── Topological traversal ─────────────────────────────────────────────────

    /// Package names in build order (deps before dependents; root last).
    pub fn topo_order(&self) -> Vec<&str> {
        let mut visited: HashSet<&str> = HashSet::new();
        let mut order: Vec<&str> = Vec::new();
        topo_visit(&self.root_name, self, &mut visited, &mut order);
        order
    }
}

fn topo_visit<'g>(
    name: &'g str,
    graph: &'g PackageGraph,
    visited: &mut HashSet<&'g str>,
    order: &mut Vec<&'g str>,
) {
    if !visited.insert(name) { return; }
    if let Some(node) = graph.packages.get(name) {
        let mut deps: Vec<&str> = node.deps.keys().map(String::as_str).collect();
        deps.sort_unstable();
        for dep in deps { topo_visit(dep, graph, visited, order); }
    }
    order.push(name);
}

// ── Resolution error ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ResolveError {
    VersionConflict { name: String, existing: String, required: String },
    DefineConflict { name: String, define: String, a: String, b: String },
    InvalidVersion { name: String, version: String },
    InvalidRequirement { name: String, req: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionConflict { name, existing, required } => write!(
                f, "dependency conflict: '{name}' resolved at {existing} but requires {required}"
            ),
            Self::DefineConflict { name, define, a, b } => write!(
                f, "dependency conflict: '{name}' define '{define}': '{a}' vs '{b}'"
            ),
            Self::InvalidVersion { name, version } => {
                write!(f, "'{name}': invalid version '{version}'")
            }
            Self::InvalidRequirement { name, req } => {
                write!(f, "'{name}': invalid requirement '{req}'")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

// ── Semver helpers ────────────────────────────────────────────────────────────

fn parse_req(req_str: &str) -> Option<semver::VersionReq> {
    if req_str == "*" { return semver::VersionReq::parse("*").ok(); }
    if let Ok(r) = semver::VersionReq::parse(req_str) { return Some(r); }
    let padded = if req_str.matches('.').count() == 1 {
        format!("^{req_str}.0")
    } else {
        format!("^{req_str}")
    };
    semver::VersionReq::parse(&padded).ok()
}

fn check_version_compat(name: &str, existing: &str, incoming_req: &str) -> Result<(), ResolveError> {
    if incoming_req == "*" { return Ok(()); }
    let ver = semver::Version::parse(existing).map_err(|_| ResolveError::InvalidVersion {
        name: name.to_string(), version: existing.to_string(),
    })?;
    let req = parse_req(incoming_req).ok_or_else(|| ResolveError::InvalidRequirement {
        name: name.to_string(), req: incoming_req.to_string(),
    })?;
    if !req.matches(&ver) {
        return Err(ResolveError::VersionConflict {
            name: name.to_string(), existing: existing.to_string(), required: incoming_req.to_string(),
        });
    }
    Ok(())
}

fn merge_dep_ref(deps: &mut HashMap<String, DepRef>, name: String, incoming: DepRef) -> Result<(), ResolveError> {
    if let Some(existing) = deps.get_mut(&name) {
        for f in &incoming.features {
            if !existing.features.contains(f) { existing.features.push(f.clone()); }
        }
        for def in &incoming.defines {
            let (key, val) = def.split_once('=').unwrap_or((def.as_str(), ""));
            if let Some(existing_def) = existing.defines.iter().find(|d| {
                let (k, _) = d.split_once('=').unwrap_or((d.as_str(), ""));
                k == key
            }) {
                let (_, existing_val) = existing_def.split_once('=').unwrap_or(("", ""));
                if existing_val != val {
                    return Err(ResolveError::DefineConflict {
                        name, define: key.to_string(), a: existing_val.to_string(), b: val.to_string(),
                    });
                }
            } else {
                existing.defines.push(def.clone());
            }
        }
    } else {
        deps.insert(name, incoming);
    }
    Ok(())
}
