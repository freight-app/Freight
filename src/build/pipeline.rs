/// Package dependency graph node.
///
/// Both the root project and every fetched dep are represented by the same
/// struct.  The tree is built lazily during dep resolution: a root node is
/// created when the build starts, and child nodes are appended as deps are
/// resolved.
///
/// # Flat `.pkgs/` pool
///
/// All `.pkgs/` paths are anchored to the root node's `dir`.  To find the
/// dir of any dep, call `node.pkgs_dir().join(&dep_name)`.  Walking `parent`
/// links up to the root gives the pool location without needing a separate
/// `pkgs_root: Option<&Path>` parameter.
///
/// # Build order
///
/// `children` holds every dep of this package **in the order they were
/// resolved** (build-deps first, then regular deps).  The pipeline builds
/// each child before building `self`, so topological order is maintained
/// automatically by the recursive build process.
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, Weak};

// ── Node ──────────────────────────────────────────────────────────────────────

pub struct PackageNode {
    /// Package name from `freight.toml`.
    pub name: String,
    /// For a root project: the version declared in `[package]`.
    /// For a dep being source-built: the resolved version string (e.g. `"1.2.3"`).
    pub version: String,
    /// Build profile used when compiling this node (`"dev"`, `"release"`, …).
    pub profile: String,
    /// Absolute path to the package's source directory.
    pub dir: PathBuf,
    /// `true` when this came from `[build-dependencies]` rather than `[dependencies]`.
    pub is_build_dep: bool,
    /// Immediate parent in the dep tree.  `None` means this is the root project.
    /// Stored as `Weak` to break reference cycles: children hold `Arc<parent>`,
    /// so the parent must not hold a strong ref back.
    pub parent: Option<Weak<PackageNode>>,
    /// Deps of this package in build order.  Each must be fully built before
    /// `self` is compiled.  Populated incrementally during dep resolution.
    pub children: RwLock<Vec<Arc<PackageNode>>>,
}

// ── Construction ──────────────────────────────────────────────────────────────

impl PackageNode {
    /// Create the root project node.
    pub fn new_root(name: impl Into<String>, version: impl Into<String>, profile: impl Into<String>, dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            version: version.into(),
            profile: profile.into(),
            dir,
            is_build_dep: false,
            parent: None,
            children: RwLock::new(vec![]),
        })
    }

    /// Create a dep node whose source lives in the flat pool under the root.
    ///
    /// `dir` is the dep's source directory inside the pool (typically
    /// `root.pkgs_dir().join(name)`).
    pub fn new_dep(
        name: impl Into<String>,
        version: impl Into<String>,
        profile: impl Into<String>,
        dir: PathBuf,
        parent: &Arc<PackageNode>,
        is_build_dep: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            version: version.into(),
            profile: profile.into(),
            dir,
            is_build_dep,
            parent: Some(Arc::downgrade(parent)),
            children: RwLock::new(vec![]),
        })
    }

    /// Add a resolved child dep to this node's build list.
    pub fn push_child(&self, child: Arc<PackageNode>) {
        self.children.write().expect("children lock poisoned").push(child);
    }
}

// ── Tree traversal ────────────────────────────────────────────────────────────

impl PackageNode {
    /// Walk up parent links to find the root project node.
    pub fn root(self: &Arc<Self>) -> Arc<Self> {
        match &self.parent {
            None => Arc::clone(self),
            Some(weak) => weak
                .upgrade()
                .expect("PackageNode: parent was dropped before child")
                .root(),
        }
    }

    /// Absolute path to the flat `.pkgs/` directory.
    ///
    /// Always rooted at the top-level project, so every dep — no matter how
    /// deeply nested — resolves to the same pool.
    pub fn pkgs_dir(self: &Arc<Self>) -> PathBuf {
        self.root().dir.join(".pkgs")
    }

    /// Convenience: path where `dep_name` lives inside the flat pool.
    pub fn dep_dir(self: &Arc<Self>, dep_name: &str) -> PathBuf {
        self.pkgs_dir().join(dep_name)
    }

    /// `true` when this node is the root project (no parent).
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }

    /// The root project's directory — where `.pkgs/` is anchored.
    pub fn pkgs_root_dir(self: &Arc<Self>) -> PathBuf {
        self.root().dir.clone()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

impl PackageNode {
    /// Build a root node from a manifest loaded at `dir`.
    ///
    /// Convenience for the common case in `build_project_with` /
    /// `build_workspace_with` where we have a path and a profile.
    pub fn from_manifest_dir(
        dir: &Path,
        profile: &str,
        manifest: &crate::manifest::types::Manifest,
    ) -> Arc<Self> {
        Self::new_root(
            &manifest.package.name,
            &manifest.package.version,
            profile,
            dir.to_path_buf(),
        )
    }
}
