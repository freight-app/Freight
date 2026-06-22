//! Bootstrapping resolver for `[build-dependencies]` (host tools).
//!
//! A build-dependency is satisfied in one of three ways, in priority order:
//!
//! 1. **`source = true`** → always build from source (via a build-system plugin),
//!    even if a prebuilt exists.
//! 2. **prebuilt binary** for the requested version → use it (a leaf).
//! 3. **system tool** on the host → use it (a leaf).
//! 4. otherwise **build from source** (via a plugin).
//!
//! Building a tool from source pulls in *its own* build-deps — e.g. CMake 3.30's
//! source is compiled by an older CMake — so resolution recurses. The recursion
//! is well-founded: each source build's builders are a strictly different
//! `(package, version)`, and the chain bottoms out at a prebuilt or system tool
//! (the base case). A chain that revisits the same `(package, version)` is a
//! cycle (error); one that never reaches a leaf is unresolvable (error).
//!
//! This module is the pure algorithm: all environment knowledge (which prebuilts
//! exist, what's on the host, a source package's own build-deps) is behind
//! [`ToolEnv`], so it is fully unit-testable. The real `ToolEnv` (registry /
//! system PATH / manifest queries) and the wiring into the build pipeline land
//! in a later stage.

use std::collections::HashSet;
use std::fmt;

/// A requested build-dependency: a name + version requirement, plus whether the
/// consumer forced a from-source build (`source = true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReq {
    pub name: String,
    pub req: String,
    pub force_source: bool,
}

impl ToolReq {
    pub fn new(name: impl Into<String>, req: impl Into<String>, force_source: bool) -> Self {
        Self {
            name: name.into(),
            req: req.into(),
            force_source,
        }
    }
}

/// How a resolved tool is satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// A system-installed tool on the host (no version pinned). A leaf.
    System,
    /// A prebuilt binary of this exact version. A leaf.
    Prebuilt { version: String },
    /// Built from source (via a build-system plugin), using its own build-deps.
    FromSource { version: String },
}

/// One entry in the resolved build-order: a tool and how it's obtained. The plan
/// is ordered **leaves first**, so building it front-to-back always has each
/// tool's own builders ready before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTool {
    pub name: String,
    pub source: ToolSource,
}

/// Environment queries the resolver needs. Each method does its own version
/// matching and returns the *resolved* concrete version, keeping the resolver
/// free of semver logic. The real impl talks to the registry, the host `PATH`,
/// and fetched manifests; tests use a fake.
pub trait ToolEnv {
    /// Resolved version of a prebuilt binary for `name` satisfying `req`, if one exists.
    fn prebuilt(&self, name: &str, req: &str) -> Option<String>;
    /// Whether a system tool `name` satisfying `req` is available on the host.
    fn system(&self, name: &str, req: &str) -> bool;
    /// Resolved source version for `name` satisfying `req`, if buildable from source.
    fn source(&self, name: &str, req: &str) -> Option<String>;
    /// The build-deps of source package `name`@`version` (their own requirements).
    fn deps_of(&self, name: &str, version: &str) -> Vec<ToolReq>;
}

/// Why resolution failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// A `(package, version)` chain that loops back on itself.
    Cycle(Vec<String>),
    /// No prebuilt, system tool, or buildable source satisfies the requirement.
    Unresolvable { name: String, req: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Cycle(chain) => {
                write!(f, "build-dependency cycle: {}", chain.join(" → "))
            }
            ResolveError::Unresolvable { name, req } => write!(
                f,
                "build-dependency '{name}' ({req}) has no prebuilt, system tool, or buildable source"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve the build-dependency graph into a leaves-first build plan.
pub fn resolve_build_deps(
    roots: &[ToolReq],
    env: &dyn ToolEnv,
) -> Result<Vec<PlannedTool>, ResolveError> {
    let mut plan = Vec::new();
    let mut done: HashSet<(String, String)> = HashSet::new();
    let mut active: Vec<(String, String)> = Vec::new();
    for r in roots {
        resolve_one(r, env, &mut plan, &mut done, &mut active)?;
    }
    Ok(plan)
}

fn key_label(key: &(String, String)) -> String {
    if key.1.is_empty() {
        format!("{} (system)", key.0)
    } else {
        format!("{}@{}", key.0, key.1)
    }
}

fn resolve_one(
    req: &ToolReq,
    env: &dyn ToolEnv,
    plan: &mut Vec<PlannedTool>,
    done: &mut HashSet<(String, String)>,
    active: &mut Vec<(String, String)>,
) -> Result<(), ResolveError> {
    // Decide how the tool is satisfied, and whether that requires recursing into
    // its own build-deps (only a from-source build does).
    let (source, recurse_version) = if req.force_source {
        match env.source(&req.name, &req.req) {
            Some(v) => (ToolSource::FromSource { version: v.clone() }, Some(v)),
            None => {
                return Err(ResolveError::Unresolvable {
                    name: req.name.clone(),
                    req: req.req.clone(),
                })
            }
        }
    } else if let Some(v) = env.prebuilt(&req.name, &req.req) {
        (ToolSource::Prebuilt { version: v }, None)
    } else if env.system(&req.name, &req.req) {
        (ToolSource::System, None)
    } else if let Some(v) = env.source(&req.name, &req.req) {
        (ToolSource::FromSource { version: v.clone() }, Some(v))
    } else {
        return Err(ResolveError::Unresolvable {
            name: req.name.clone(),
            req: req.req.clone(),
        });
    };

    let version = match &source {
        ToolSource::Prebuilt { version } | ToolSource::FromSource { version } => version.clone(),
        ToolSource::System => String::new(),
    };
    let key = (req.name.clone(), version);

    if done.contains(&key) {
        return Ok(()); // already planned (shared across the graph)
    }
    if active.contains(&key) {
        let mut chain: Vec<String> = active.iter().map(key_label).collect();
        chain.push(key_label(&key));
        return Err(ResolveError::Cycle(chain));
    }

    if let Some(ver) = recurse_version {
        active.push(key.clone());
        for sub in env.deps_of(&req.name, &ver) {
            resolve_one(&sub, env, plan, done, active)?;
        }
        active.pop();
    }

    done.insert(key);
    plan.push(PlannedTool {
        name: req.name.clone(),
        source,
    });
    Ok(())
}

// ── Real environment ────────────────────────────────────────────────────────

/// The real [`ToolEnv`], backed by the host `PATH` and fetched package manifests.
///
/// `prebuilt` is always `None` until freight grows a prebuilt-binary registry
/// index; until then the resolver chooses **system tool if present, else build
/// from source** (and `source = true` still forces source). `deps_of` reads a
/// fetched package's own `[build-dependencies]`, so the bootstrapping recursion
/// works for anything already in `.pkgs/`.
pub struct HostToolEnv {
    /// The `.pkgs/` cache where fetched build-deps live.
    pub pkgs_dir: std::path::PathBuf,
}

fn dep_req(dep: &crate::manifest::types::Dependency) -> String {
    use crate::manifest::types::Dependency;
    match dep {
        Dependency::Simple(v) => v.clone(),
        Dependency::Detailed(d) => d.version.clone().unwrap_or_else(|| "*".to_string()),
    }
}

impl ToolEnv for HostToolEnv {
    fn prebuilt(&self, _name: &str, _req: &str) -> Option<String> {
        None // no prebuilt-binary registry index yet
    }

    fn system(&self, name: &str, _req: &str) -> bool {
        // Presence on PATH. (Version-aware system matching is a later refinement.)
        crate::toolchain::detect::which(name).is_some()
    }

    fn source(&self, name: &str, req: &str) -> Option<String> {
        // A declared build-dep is always source-buildable (fetched + built). Use
        // the fetched manifest's version when available, else the requested one.
        if let Ok(m) = crate::manifest::load_manifest(&self.pkgs_dir.join(name)) {
            return Some(m.package.version);
        }
        Some(if req == "*" { "*".to_string() } else { req.to_string() })
    }

    fn deps_of(&self, name: &str, _version: &str) -> Vec<ToolReq> {
        use crate::manifest::types::Dependency;
        let Ok(m) = crate::manifest::load_manifest(&self.pkgs_dir.join(name)) else {
            return Vec::new();
        };
        m.effective_build_dependencies()
            .into_iter()
            .map(|(n, d)| {
                let force = matches!(&d, Dependency::Detailed(dd) if dd.source);
                ToolReq::new(n, dep_req(&d), force)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A fake environment: simple exact-or-`*` version matching, max version wins.
    #[derive(Default)]
    struct Fake {
        prebuilt: HashMap<String, Vec<String>>,
        system: HashSet<String>,
        source: HashMap<String, Vec<String>>,
        deps: HashMap<(String, String), Vec<ToolReq>>,
    }

    fn pick<'a>(versions: Option<&'a Vec<String>>, req: &str) -> Option<String> {
        let vs = versions?;
        let mut matching: Vec<&String> = if req == "*" {
            vs.iter().collect()
        } else {
            vs.iter().filter(|v| v.as_str() == req).collect()
        };
        matching.sort();
        matching.last().map(|s| s.to_string())
    }

    impl ToolEnv for Fake {
        fn prebuilt(&self, name: &str, req: &str) -> Option<String> {
            pick(self.prebuilt.get(name), req)
        }
        fn system(&self, name: &str, _req: &str) -> bool {
            self.system.contains(name)
        }
        fn source(&self, name: &str, req: &str) -> Option<String> {
            pick(self.source.get(name), req)
        }
        fn deps_of(&self, name: &str, version: &str) -> Vec<ToolReq> {
            self.deps
                .get(&(name.to_string(), version.to_string()))
                .cloned()
                .unwrap_or_default()
        }
    }

    #[test]
    fn prefers_prebuilt_then_system() {
        let mut env = Fake::default();
        env.prebuilt.insert("cmake".into(), vec!["3.20".into()]);
        env.system.insert("ninja".into());
        let plan = resolve_build_deps(
            &[ToolReq::new("cmake", "*", false), ToolReq::new("ninja", "*", false)],
            &env,
        )
        .unwrap();
        assert_eq!(
            plan,
            vec![
                PlannedTool { name: "cmake".into(), source: ToolSource::Prebuilt { version: "3.20".into() } },
                PlannedTool { name: "ninja".into(), source: ToolSource::System },
            ]
        );
    }

    #[test]
    fn source_true_forces_source_over_prebuilt() {
        let mut env = Fake::default();
        env.prebuilt.insert("cmake".into(), vec!["3.20".into()]);
        env.source.insert("cmake".into(), vec!["3.20".into()]);
        // cmake source built by a prebuilt make (leaf).
        env.deps.insert(
            ("cmake".into(), "3.20".into()),
            vec![ToolReq::new("make", "*", false)],
        );
        env.system.insert("make".into());
        let plan = resolve_build_deps(&[ToolReq::new("cmake", "*", true)], &env).unwrap();
        assert_eq!(
            plan,
            vec![
                PlannedTool { name: "make".into(), source: ToolSource::System },
                PlannedTool { name: "cmake".into(), source: ToolSource::FromSource { version: "3.20".into() } },
            ]
        );
    }

    #[test]
    fn bootstraps_via_older_prebuilt_leaves_first() {
        // cmake 3.30 has no prebuilt → built from source by cmake <3.30 (prebuilt).
        let mut env = Fake::default();
        env.source.insert("cmake".into(), vec!["3.30".into()]);
        env.prebuilt.insert("cmake".into(), vec!["3.20".into()]); // older prebuilt exists
        env.deps.insert(
            ("cmake".into(), "3.30".into()),
            vec![ToolReq::new("cmake", "3.20", false)],
        );
        let plan = resolve_build_deps(&[ToolReq::new("cmake", "3.30", false)], &env).unwrap();
        assert_eq!(
            plan,
            vec![
                PlannedTool { name: "cmake".into(), source: ToolSource::Prebuilt { version: "3.20".into() } },
                PlannedTool { name: "cmake".into(), source: ToolSource::FromSource { version: "3.30".into() } },
            ]
        );
    }

    #[test]
    fn same_version_self_dependency_is_a_cycle() {
        let mut env = Fake::default();
        env.source.insert("cmake".into(), vec!["3.30".into()]);
        // pathological: cmake 3.30 source needs cmake 3.30 (no prebuilt to break it)
        env.deps.insert(
            ("cmake".into(), "3.30".into()),
            vec![ToolReq::new("cmake", "3.30", true)],
        );
        let err = resolve_build_deps(&[ToolReq::new("cmake", "3.30", true)], &env).unwrap_err();
        assert!(matches!(err, ResolveError::Cycle(_)), "got {err:?}");
    }

    #[test]
    fn no_leaf_is_unresolvable() {
        let mut env = Fake::default();
        env.source.insert("cmake".into(), vec!["3.30".into()]);
        // cmake 3.30 needs a tool that has no prebuilt/system/source.
        env.deps.insert(
            ("cmake".into(), "3.30".into()),
            vec![ToolReq::new("ghost", "*", false)],
        );
        let err = resolve_build_deps(&[ToolReq::new("cmake", "3.30", false)], &env).unwrap_err();
        assert_eq!(
            err,
            ResolveError::Unresolvable { name: "ghost".into(), req: "*".into() }
        );
    }

    #[test]
    fn shared_tool_is_planned_once() {
        let mut env = Fake::default();
        env.prebuilt.insert("ninja".into(), vec!["1.0".into()]);
        let plan = resolve_build_deps(
            &[ToolReq::new("ninja", "*", false), ToolReq::new("ninja", "*", false)],
            &env,
        )
        .unwrap();
        assert_eq!(plan.len(), 1);
    }
}
