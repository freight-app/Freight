/// Flat package dependency graph.
///
/// All packages — the root project and every fetched dep — live in a single
/// `HashMap<name, Arc<PackageNode>>` owned by `PackageGraph`. Edges are stored
/// on each node as `HashMap<name, DepRef>` rather than as strong `Arc` links,
/// so there are no reference cycles and no tree traversal is needed to find the
/// root.
///
/// # Resolution
///
/// `PackageGraph::insert` performs resolution at insertion time:
/// - If the name is new, the node is accepted and its `DepRef` recorded.
/// - If the name already exists, the incoming `version_req` must be compatible
///   with the stored resolved version (semver intersection). Features are
///   unioned. Conflicting defines are an error.
///
/// # Target directories
///
/// `PackageGraph::target_dir(name)`:
/// - Root package → `root_dir/target`
/// - Any other package → `root_dir/target/deps/<name>`
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
    /// Semver requirement string from the manifest (e.g. `"1.2"`, `">=1.0"`, `"*"`).
    pub version_req: String,
    /// Features requested for this dep.
    pub features: Vec<String>,
    /// Extra preprocessor defines injected when building this dep.
    pub defines: Vec<String>,
    /// `None` = normal; `Some(Build)` = build-dep; `Some(Dev)` = dev-dep.
    pub kind: Option<DepKind>,
}

impl DepRef {
    pub fn normal(version_req: impl Into<String>) -> Self {
        Self {
            version_req: version_req.into(),
            features: vec![],
            defines: vec![],
            kind: None,
        }
    }

    pub fn build_dep(version_req: impl Into<String>) -> Self {
        Self {
            kind: Some(DepKind::Build),
            ..Self::normal(version_req)
        }
    }

    pub fn dev_dep(version_req: impl Into<String>) -> Self {
        Self {
            kind: Some(DepKind::Dev),
            ..Self::normal(version_req)
        }
    }
}

// ── PackageNode ───────────────────────────────────────────────────────────────

pub struct PackageNode {
    /// Package name from `freight.toml`.
    pub name: String,
    /// Resolved version (e.g. `"1.2.3"`).
    pub version: String,
    /// Build profile (`"dev"`, `"release"`, …).
    pub profile: String,
    /// Absolute path to the package's source directory.
    pub dir: PathBuf,
    /// Immediate dep references keyed by dep name.
    pub deps: HashMap<String, DepRef>,
}

// ── PackageGraph ──────────────────────────────────────────────────────────────

pub struct PackageGraph {
    /// Name of the root project package.
    pub root_name: String,
    /// Source directory of the root project.
    pub root_dir: PathBuf,
    /// Active build profile for all packages in this graph.
    pub profile: String,
    /// Flat registry of all packages (root + deps). Key = package name.
    pub packages: HashMap<String, Arc<PackageNode>>,
}

// ── Resolution error ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ResolveError {
    /// `name` is already present with version `existing` which does not satisfy
    /// the incoming requirement `required`.
    VersionConflict {
        name: String,
        existing: String,
        required: String,
    },
    /// `name` is required with conflicting preprocessor defines that cannot be
    /// merged (key `define` has values `a` vs `b`).
    DefineConflict {
        name: String,
        define: String,
        a: String,
        b: String,
    },
    /// The version string `version` could not be parsed as a semver version.
    InvalidVersion { name: String, version: String },
    /// The requirement string `req` could not be parsed as a semver requirement.
    InvalidRequirement { name: String, req: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionConflict {
                name,
                existing,
                required,
            } => write!(
                f,
                "dependency conflict: '{name}' is already resolved at {existing} \
                 but another dep requires {required}"
            ),
            Self::DefineConflict { name, define, a, b } => write!(
                f,
                "dependency conflict: '{name}' has incompatible define '{define}': \
                 '{a}' vs '{b}'"
            ),
            Self::InvalidVersion { name, version } => {
                write!(f, "'{name}': invalid version '{version}'")
            }
            Self::InvalidRequirement { name, req } => {
                write!(f, "'{name}': invalid version requirement '{req}'")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

// ── PackageGraph impl ─────────────────────────────────────────────────────────

impl PackageGraph {
    /// Create a graph seeded with the root project.
    pub fn new(root: Arc<PackageNode>) -> Self {
        let root_name = root.name.clone();
        let root_dir = root.dir.clone();
        let profile = root.profile.clone();
        let mut packages = HashMap::new();
        packages.insert(root_name.clone(), root);
        Self {
            root_name,
            root_dir,
            profile,
            packages,
        }
    }

    /// Insert `node` as a dep of `parent_name`, recording `dep_ref` as the
    /// edge.  If a package with the same name already exists in the graph,
    /// the incoming `dep_ref.version_req` must be satisfied by the stored
    /// resolved version; features are unioned; defines are checked for
    /// conflicts.
    ///
    /// Returns `Ok(false)` when the package was already present and compatible
    /// (no build needed again), `Ok(true)` when it was freshly inserted.
    pub fn insert(
        &mut self,
        node: Arc<PackageNode>,
        parent_name: &str,
        dep_ref: DepRef,
    ) -> Result<bool, ResolveError> {
        let name = node.name.clone();

        if let Some(existing) = self.packages.get(&name) {
            // Package already registered — check semver compatibility.
            check_version_compat(&name, &existing.version, &dep_ref.version_req)?;

            // Record the edge on the parent anyway (it still depends on this pkg).
            if let Some(parent) = Arc::get_mut(
                self.packages
                    .get_mut(parent_name)
                    .expect("parent must be in graph"),
            ) {
                merge_dep_ref(&mut parent.deps, name.clone(), dep_ref)?;
            }

            Ok(false)
        } else {
            // New package — insert it, then record the edge on the parent.
            self.packages.insert(name.clone(), node);

            if let Some(parent) = Arc::get_mut(
                self.packages
                    .get_mut(parent_name)
                    .expect("parent must be in graph"),
            ) {
                merge_dep_ref(&mut parent.deps, name.clone(), dep_ref)?;
            }

            Ok(true)
        }
    }

    // ── Path helpers ──────────────────────────────────────────────────────────

    /// Absolute path to the flat `.pkgs/` pool (always anchored at the root).
    pub fn pkgs_dir(&self) -> PathBuf {
        self.root_dir.join(".pkgs")
    }

    /// Absolute path to `<dep_name>`'s source directory in the flat pool.
    pub fn dep_source_dir(&self, dep_name: &str) -> PathBuf {
        self.pkgs_dir().join(dep_name)
    }

    /// Where build artifacts for `name` are written.
    ///
    /// - Root package → `root_dir/target`
    /// - Any dep → `root_dir/target/deps/<name>`
    pub fn target_dir(&self, name: &str) -> PathBuf {
        if name == self.root_name {
            self.root_dir.join("target")
        } else {
            self.root_dir.join("target").join("deps").join(name)
        }
    }

    /// Convenience: `target_dir` for the root package.
    pub fn root_target_dir(&self) -> PathBuf {
        self.target_dir(&self.root_name)
    }

    /// `true` if `name` is the root package of this graph.
    pub fn is_root(&self, name: &str) -> bool {
        name == self.root_name
    }

    // ── Build-order traversal ─────────────────────────────────────────────────

    /// Return package names in topological build order (deps before dependents).
    ///
    /// The root package is always last.  Packages without recorded deps come
    /// first (stable ordering within the same depth level).
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
    if !visited.insert(name) {
        return;
    }
    if let Some(node) = graph.packages.get(name) {
        // Visit deps first (build order: leaves before root).
        let mut dep_names: Vec<&str> = node.deps.keys().map(String::as_str).collect();
        dep_names.sort_unstable(); // deterministic order
        for dep in dep_names {
            topo_visit(dep, graph, visited, order);
        }
    }
    order.push(name);
}

// ── PackageNode construction ──────────────────────────────────────────────────

impl PackageNode {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        profile: impl Into<String>,
        dir: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            version: version.into(),
            profile: profile.into(),
            dir,
            deps: HashMap::new(),
        })
    }

    /// Build a node from a manifest loaded at `dir`.
    pub fn from_manifest_dir(
        dir: &Path,
        profile: &str,
        manifest: &crate::manifest::types::Manifest,
    ) -> Arc<Self> {
        Self::new(
            &manifest.package.name,
            &manifest.package.version,
            profile,
            dir.to_path_buf(),
        )
    }
}

// ── Semver helpers ────────────────────────────────────────────────────────────

/// Parse `req_str` into a semver `VersionReq`, handling bare versions like
/// `"1.2.3"` (treated as `^1.2.3`) and `"*"`.
fn parse_req(req_str: &str) -> Option<semver::VersionReq> {
    if req_str == "*" {
        return semver::VersionReq::parse("*").ok();
    }
    // Try as-is first (handles `>=1.0`, `^1.2`, etc.)
    if let Ok(r) = semver::VersionReq::parse(req_str) {
        return Some(r);
    }
    // Bare version like "1.2.3" or "1.2" — treat as caret req.
    let padded = if req_str.matches('.').count() == 1 {
        format!("^{req_str}.0")
    } else {
        format!("^{req_str}")
    };
    semver::VersionReq::parse(&padded).ok()
}

fn check_version_compat(
    name: &str,
    existing_version: &str,
    incoming_req: &str,
) -> Result<(), ResolveError> {
    if incoming_req == "*" {
        return Ok(());
    }
    let ver =
        semver::Version::parse(existing_version).map_err(|_| ResolveError::InvalidVersion {
            name: name.to_string(),
            version: existing_version.to_string(),
        })?;
    let req = parse_req(incoming_req).ok_or_else(|| ResolveError::InvalidRequirement {
        name: name.to_string(),
        req: incoming_req.to_string(),
    })?;
    if !req.matches(&ver) {
        return Err(ResolveError::VersionConflict {
            name: name.to_string(),
            existing: existing_version.to_string(),
            required: incoming_req.to_string(),
        });
    }
    Ok(())
}

/// Merge `incoming` into `deps[name]`.  Features are unioned; defines are
/// checked for key=value conflicts (same key, different value → error).
fn merge_dep_ref(
    deps: &mut HashMap<String, DepRef>,
    name: String,
    incoming: DepRef,
) -> Result<(), ResolveError> {
    if let Some(existing) = deps.get_mut(&name) {
        // Union features.
        for f in &incoming.features {
            if !existing.features.contains(f) {
                existing.features.push(f.clone());
            }
        }
        // Check defines for conflicts.
        for def in &incoming.defines {
            let (key, val) = def.split_once('=').unwrap_or((def.as_str(), ""));
            if let Some(existing_def) = existing.defines.iter().find(|d| {
                let (k, _) = d.split_once('=').unwrap_or((d.as_str(), ""));
                k == key
            }) {
                let (_, existing_val) = existing_def.split_once('=').unwrap_or(("", ""));
                if existing_val != val {
                    return Err(ResolveError::DefineConflict {
                        name,
                        define: key.to_string(),
                        a: existing_val.to_string(),
                        b: val.to_string(),
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

// ── Backward-compat shim ──────────────────────────────────────────────────────
//
// The build pipeline in `mod.rs` and `adaptors/mod.rs` still uses a
// `PackageGraph` passed as `Option<&PackageGraph>`.  When `None`, a temporary
// root-only graph is created internally.  All callers that previously accepted
// `Option<&Arc<PackageNode>>` now accept `Option<&PackageGraph>`.

impl PackageGraph {
    /// Create a minimal single-node graph for an isolated root build.
    /// `dir` is both the package source directory and the root project directory.
    pub fn root_only(
        name: impl Into<String>,
        version: impl Into<String>,
        profile: impl Into<String>,
        dir: PathBuf,
    ) -> Self {
        let node = PackageNode::new(name, version, profile, dir);
        Self::new(node)
    }

    /// Create a single-node graph for a dep source-build.
    ///
    /// `pkg_dir` is the dep's own source directory (e.g. `.pkgs/vecmath/`).
    /// `root_dir` is the root project directory that owns the flat `.pkgs/` pool.
    pub fn for_dep(
        name: impl Into<String>,
        version: impl Into<String>,
        profile: impl Into<String>,
        pkg_dir: PathBuf,
        root_dir: PathBuf,
    ) -> Self {
        let name: String = name.into();
        let node = PackageNode::new(name.clone(), version, profile.into(), pkg_dir);
        let profile = node.profile.clone();
        let mut packages = HashMap::new();
        packages.insert(name.clone(), node);
        Self {
            root_name: name,
            root_dir,
            profile,
            packages,
        }
    }

    /// Add a dep node to this graph, returning whether it was newly inserted.
    /// Uses `DepRef::normal` for the edge.  For the common source-build path
    /// where fine-grained edge metadata isn't yet resolved.
    pub fn add_dep_node(
        &mut self,
        node: Arc<PackageNode>,
        parent_name: &str,
        version_req: impl Into<String>,
        kind: Option<DepKind>,
    ) -> Result<bool, ResolveError> {
        let dep_ref = DepRef {
            version_req: version_req.into(),
            features: vec![],
            defines: vec![],
            kind,
        };
        self.insert(node, parent_name, dep_ref)
    }
}

// ── Pipeline goal / config / output ──────────────────────────────────────────

/// What the pipeline should do after compiling sources.
#[derive(Debug, Clone)]
pub enum PipelineGoal {
    /// Compile and link production targets.
    Build,
    /// Compile, then compile+link+run files in `tests/`.
    Test {
        /// If set, only run test files whose stem matches.
        filter: Option<String>,
    },
    /// Compile, then compile+link+run files in `benches/` with timing.
    Bench {
        /// If set, only run bench files whose stem matches.
        filter: Option<String>,
    },
}

impl Default for PipelineGoal {
    fn default() -> Self {
        Self::Build
    }
}

impl PipelineGoal {
    /// Whether dev-dependencies should be included in the dep graph.
    pub fn include_dev_deps(&self) -> bool {
        matches!(self, Self::Test { .. })
    }
}

/// Configuration for a single `run_pipeline_at` invocation.
#[derive(Debug, Clone, Default)]
pub struct PipelineConfig {
    /// Build profile (e.g. `"dev"`, `"release"`).
    pub profile: String,
    /// Feature flags requested on the command line.
    pub features: Vec<String>,
    /// Whether to activate default features.  Usually `true`.
    pub use_defaults: bool,
    /// Override the compiler target triple (cross-compilation).
    pub target_override: Option<String>,
    /// Replace the profile's `sanitize` list when non-empty.
    pub sanitize_override: Vec<String>,
    /// What to do after sources are compiled.
    pub goal: PipelineGoal,
}
