//! `freight metadata` — emit a machine-readable JSON description of the resolved
//! package and dependency graph, the freight analogue of `cargo metadata`.
//!
//! Intended for editor/tooling consumption. The output is a single JSON object
//! on stdout; diagnostics go to stderr so the stream stays parseable.

use std::collections::BTreeSet;
use std::path::Path;

use freight::build::deps::resolve_dep_graph;
use freight::manifest::types::{Dependency, Manifest};
use freight::manifest::{load_manifest, load_workspace_manifest, workspace::find_workspace_root};

use serde_json::{json, Value};

use crate::output::print_error;

/// Bump when the shape of the emitted JSON changes incompatibly.
const FORMAT_VERSION: u32 = 1;

#[derive(clap::Args)]
pub struct Args {
    /// Output only the root package — skip walking the resolved dependency graph.
    #[arg(long)]
    pub no_deps: bool,
    /// Emit compact single-line JSON instead of pretty-printed.
    #[arg(long)]
    pub compact: bool,
}

impl Args {
    pub fn run(self) {
        // `locate_project_dir` already reports an error and flags the exit code.
        let Some(project_dir) = super::common::locate_project_dir() else {
            return;
        };

        let manifest = match load_manifest(&project_dir) {
            Ok(m) => m,
            Err(e) => {
                print_error(&format!("{e}"));
                return;
            }
        };

        let value = build_metadata(&project_dir, &manifest, self.no_deps);

        let rendered = if self.compact {
            serde_json::to_string(&value)
        } else {
            serde_json::to_string_pretty(&value)
        };
        match rendered {
            Ok(s) => println!("{s}"),
            Err(e) => print_error(&format!("serializing metadata: {e}")),
        }
    }
}

fn build_metadata(project_dir: &Path, manifest: &Manifest, no_deps: bool) -> Value {
    let mut packages = vec![package_json(
        &manifest.package.name,
        project_dir,
        manifest,
        0,
        "local",
    )];

    if !no_deps {
        match resolve_dep_graph(project_dir, manifest, true, &BTreeSet::new()) {
            Ok(resolved) => {
                for dep in resolved {
                    packages.push(package_json(
                        &dep.name,
                        &dep.dir,
                        &dep.manifest,
                        dep.depth,
                        "local",
                    ));
                }
            }
            Err(e) => {
                // Best-effort: a missing `.pkgs/` checkout shouldn't make
                // `metadata` fail outright — emit the root package and note why
                // the graph is incomplete on stderr.
                eprintln!(
                    "warning: dependency graph incomplete ({e}); \
                     run `freight fetch`, or pass --no-deps to silence this"
                );
            }
        }
    }

    let workspace = find_workspace_root(project_dir).map(|root| {
        let members = load_workspace_manifest(&root)
            .map(|w| w.members)
            .unwrap_or_default();
        json!({
            "root": root.to_string_lossy(),
            "members": members,
        })
    });

    json!({
        "format_version": FORMAT_VERSION,
        "root": manifest.package.name,
        "target_directory": project_dir.join("target").to_string_lossy(),
        "workspace": workspace,
        "packages": packages,
    })
}

fn package_json(name: &str, dir: &Path, manifest: &Manifest, depth: usize, source: &str) -> Value {
    let mut targets: Vec<Value> = Vec::new();
    if let Some(lib) = &manifest.lib {
        targets.push(json!({
            "kind": "lib",
            "name": manifest.package.name,
            "type": lib.lib_type,
        }));
    }
    for bin in &manifest.bins {
        targets.push(json!({
            "kind": "bin",
            "name": bin.name,
        }));
    }

    let mut dependencies: Vec<Value> = Vec::new();
    for (n, d) in manifest.effective_dependencies() {
        dependencies.push(dep_json(&n, &d, "normal"));
    }
    for (n, d) in &manifest.build_dependencies {
        dependencies.push(dep_json(n, d, "build"));
    }
    for (n, d) in &manifest.dev_dependencies {
        dependencies.push(dep_json(n, d, "dev"));
    }
    dependencies.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    let mut languages: Vec<&String> = manifest.language.keys().collect();
    languages.sort();

    json!({
        "name": name,
        "version": manifest.package.version,
        "manifest_path": dir.join("freight.toml").to_string_lossy(),
        "source": source,
        "depth": depth,
        "languages": languages,
        "features": manifest.features,
        "provides": manifest.package.provides,
        "targets": targets,
        "dependencies": dependencies,
    })
}

fn dep_json(name: &str, dep: &Dependency, kind: &str) -> Value {
    let mut obj = json!({ "name": name, "kind": kind });
    let map = obj.as_object_mut().expect("json object");
    match dep {
        Dependency::Simple(v) => {
            map.insert("source".into(), "registry".into());
            map.insert("req".into(), v.clone().into());
        }
        Dependency::Detailed(d) => {
            if let Some(p) = &d.path {
                map.insert("source".into(), "path".into());
                map.insert("path".into(), p.clone().into());
            } else if d.is_git() {
                map.insert("source".into(), "git".into());
                if let Some(u) = &d.url {
                    map.insert("url".into(), u.clone().into());
                }
                for (k, v) in [("branch", &d.branch), ("tag", &d.tag), ("rev", &d.rev)] {
                    if let Some(val) = v {
                        map.insert(k.into(), val.clone().into());
                    }
                }
            } else if let Some(u) = &d.url {
                map.insert("source".into(), "url".into());
                map.insert("url".into(), u.clone().into());
            } else {
                map.insert("source".into(), "registry".into());
                if let Some(v) = &d.version {
                    map.insert("req".into(), v.clone().into());
                }
            }
            if !d.features.is_empty() {
                map.insert("features".into(), json!(d.features));
            }
            if d.optional {
                map.insert("optional".into(), true.into());
            }
            if !d.default_features {
                map.insert("default_features".into(), false.into());
            }
        }
    }
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_metadata_shape() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("freight.toml"),
            "[package]\nname=\"app\"\nversion=\"1.2.3\"\n[language.cpp]\n\
             [[bin]]\nname=\"app\"\nsrc=\"src/main.cpp\"\n\
             [dependencies]\nfmt=\">=10.0\"\n",
        )
        .unwrap();
        let manifest = load_manifest(dir.path()).unwrap();

        // no_deps avoids needing a fetched `.pkgs/` graph.
        let md = build_metadata(dir.path(), &manifest, true);
        assert_eq!(md["format_version"], json!(FORMAT_VERSION));
        assert_eq!(md["root"], json!("app"));

        let pkgs = md["packages"].as_array().unwrap();
        assert_eq!(pkgs.len(), 1);
        let app = &pkgs[0];
        assert_eq!(app["version"], json!("1.2.3"));
        assert_eq!(app["depth"], json!(0));
        assert_eq!(app["targets"][0]["kind"], json!("bin"));

        let dep = &app["dependencies"][0];
        assert_eq!(dep["name"], json!("fmt"));
        assert_eq!(dep["source"], json!("registry"));
        assert_eq!(dep["req"], json!(">=10.0"));
    }
}
