use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// Generate Markdown docs for this project (output format: md)
    #[arg(long, short, value_name = "FORMAT")]
    pub format: Option<String>,
    /// Generate man pages for all freight subcommands
    #[arg(long)]
    pub man: bool,
    /// Output directory for man pages (default: target/man/)
    #[arg(long, value_name = "DIR", requires = "man")]
    pub out_dir: Option<String>,
}

impl Args {
    pub fn run(self) {
        cmd_doc(self.format.as_deref(), self.man, self.out_dir.as_deref());
    }
}

use freight::doc::{extract_dir, extract_file, DocSet};
use freight::doc::{self, collect_stdlib, DocDependency, PackageDoc, StdlibMsg};
use freight::manifest::types::{Dependency, Manifest};
use freight::manifest::{find_manifest_dir, load_manifest};
use freight::toolchain::freight_home;

use crate::output::{print_error, print_status, print_success, print_warning};

// ── freight doc ─────────────────────────────────────────────────────────────────

pub fn cmd_doc(format: Option<&str>, man: bool, out_dir: Option<&str>) {
    if man {
        cmd_man(out_dir);
    } else if format.is_some() {
        generate_docs();
    } else if let Err(e) = open_dependency_tui() {
        print_error(&format!("failed to open dependency docs: {e}"));
    }
}

fn generate_docs() {
    let cwd = std::env::current_dir().expect("cannot read cwd");
    let project_dir = find_manifest_dir(&cwd).unwrap_or_else(|| cwd.clone());
    let out_dir = project_dir.join("target").join("doc");

    let mut source_dirs: Vec<PathBuf> = Vec::new();

    match load_manifest(&project_dir) {
        Ok(manifest) => {
            // Library source + header dirs
            if let Some(lib) = &manifest.lib {
                for s in &lib.srcs {
                    let d = project_dir.join(s);
                    let dir = if d.is_dir() {
                        d
                    } else {
                        d.parent()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| project_dir.clone())
                    };
                    if dir.is_dir() && !source_dirs.contains(&dir) {
                        source_dirs.push(dir);
                    }
                }
                for hdr in &lib.hdrs {
                    if let Some(parent) = project_dir.join(hdr).parent().map(PathBuf::from) {
                        if parent.is_dir() && !source_dirs.contains(&parent) {
                            source_dirs.push(parent);
                        }
                    }
                }
            }
            // Binary source dirs — take the parent directory of the src path
            for bin in &manifest.bins {
                let abs = project_dir.join(&bin.src);
                let dir = if abs.is_dir() {
                    abs
                } else {
                    abs.parent()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| project_dir.clone())
                };
                if dir.is_dir() && !source_dirs.contains(&dir) {
                    source_dirs.push(dir);
                }
            }
            // Default fallback: src/
            if source_dirs.is_empty() {
                let src = project_dir.join("src");
                if src.is_dir() {
                    source_dirs.push(src);
                }
            }
            // Path dependencies
            for (name, dep) in &manifest.dependencies {
                if let Dependency::Detailed(d) = dep {
                    if let Some(rel) = &d.path {
                        let dep_dir = project_dir.join(rel);
                        if dep_dir.is_dir() {
                            print_status("     Dep", name);
                            source_dirs.push(dep_dir);
                        }
                    }
                }
            }
        }
        Err(_) => {
            let src = project_dir.join("src");
            source_dirs.push(if src.is_dir() {
                src
            } else {
                project_dir.clone()
            });
        }
    }

    if source_dirs.is_empty() {
        print_error("no source directories to scan");
        return;
    }

    let mut all_items = Vec::new();
    for dir in &source_dirs {
        if !dir.is_dir() {
            print_warning(&format!("skipping missing: {}", dir.display()));
            continue;
        }
        print_status("Scanning", &dir.display().to_string());
        all_items.extend(extract_dir(dir).items);
    }

    if all_items.is_empty() {
        print_warning(
            "no documented items found — add doc comments (///, /**, !>, …) to your sources",
        );
        return;
    }

    let total = all_items.len();
    let combined = DocSet {
        items: all_items,
        source_root: project_dir,
    };

    match doc::generate(combined, &out_dir) {
        Ok(()) => print_success(&format!(
            "{total} items [md] → {}",
            out_dir.join("index.md").display()
        )),
        Err(e) => print_error(&format!("failed to write docs: {e}")),
    }
}

// ── man page generation (freight doc --man) ───────────────────────────────────

fn cmd_man(out_dir: Option<&str>) {
    let out = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("man"));

    if let Err(e) = std::fs::create_dir_all(&out) {
        print_error(&format!("cannot create output dir: {e}"));
        return;
    }

    let cmd = crate::cli_command();
    let mut count = 0;
    gen_man_pages(&cmd, "freight", &out, &mut count);

    print_success(&format!("{count} man pages → {}", out.display()));
    println!("  Preview : man -l {}/freight.1", out.display());
    println!(
        "  Install : sudo cp {}/*.1 /usr/local/share/man/man1/",
        out.display()
    );
}

fn gen_man_pages(cmd: &clap::Command, prefix: &str, out_dir: &Path, count: &mut usize) {
    // clap::Command::name() requires 'static; Box::leak is acceptable in a
    // one-shot CLI that exits immediately after generating the pages.
    let static_name: &'static str = Box::leak(prefix.to_string().into_boxed_str());
    let page_cmd = cmd.clone().name(static_name);
    let man = clap_mangen::Man::new(page_cmd);
    let path = out_dir.join(format!("{prefix}.1"));

    match std::fs::File::create(&path) {
        Ok(mut f) => {
            if man.render(&mut f).is_ok() {
                print_status("Generate", &format!("{prefix}.1"));
                *count += 1;
            } else {
                print_warning(&format!("render failed for {prefix}.1"));
            }
        }
        Err(e) => print_warning(&format!("cannot write {}: {e}", path.display())),
    }

    for sub in cmd.get_subcommands() {
        gen_man_pages(sub, &format!("{prefix}-{}", sub.get_name()), out_dir, count);
    }
}

// ── freight doc dependency browser ────────────────────────────────────────────

fn open_dependency_tui() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd.clone());

    if !io::stdout().is_terminal() {
        let deps = collect_doc_dependencies(&project_dir);
        print_dependency_table(&deps);
        return Ok(());
    }

    let mut packages: Vec<PackageDoc> = Vec::new();

    // Current project — prefer [lib] hdrs, then srcs, then src/.
    let manifest = load_manifest(&project_dir).ok();
    let pkg_name = manifest
        .as_ref()
        .map(|m| m.package.name.clone())
        .unwrap_or_else(|| {
            project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string()
        });
    let pkg_ver = manifest
        .as_ref()
        .map(|m| m.package.version.clone())
        .unwrap_or_else(|| "0.0.0".to_string());
    let readme = read_readme(&project_dir);

    let items = extract_pkg_items(&project_dir, manifest.as_ref());
    if !items.is_empty() {
        print_status("Scanning", &pkg_name);
        packages.push(PackageDoc {
            name: pkg_name,
            version: pkg_ver,
            items,
            readme,
        });
    }

    // Dependencies — prefer their [lib] hdrs too.
    let deps = collect_doc_dependencies(&project_dir);
    for dep in &deps {
        let Some(ref dir) = dep.path else { continue };
        let dep_manifest = load_manifest(dir).ok();
        let items = extract_pkg_items(dir, dep_manifest.as_ref());
        if items.is_empty() {
            continue;
        }
        print_status("     Dep", &dep.name);
        let readme = read_readme(dir);
        packages.push(PackageDoc {
            name: dep.name.clone(),
            version: dep.version.clone(),
            items,
            readme,
        });
    }

    if packages.is_empty() {
        print_warning("no documented items found");
        println!("hint: add doc comments (///, /**, !>, …) to your sources");
        return Ok(());
    }

    // Scan stdlib headers in the background so the TUI opens immediately.
    let (tx, rx) = std::sync::mpsc::channel::<StdlibMsg>();
    std::thread::spawn(move || collect_stdlib(tx));
    doc::browse(packages, rx)
}

/// Extract doc items from `dir`, using `[lib] hdrs` when available.
/// Falls back to `[lib] srcs`, then `src/`.
fn extract_pkg_items(
    dir: &Path,
    manifest: Option<&freight::manifest::types::Manifest>,
) -> Vec<freight::doc::DocItem> {
    // Collect candidate files from public headers.
    let hdr_files: Vec<PathBuf> = manifest
        .and_then(|m| m.lib.as_ref())
        .map(|lib| lib.hdrs.iter().map(|h| dir.join(h)).collect())
        .unwrap_or_default();

    if !hdr_files.is_empty() {
        let mut items = Vec::new();
        for path in &hdr_files {
            if path.is_file() {
                items.extend(extract_file(path));
            }
        }
        if !items.is_empty() {
            return items;
        }
    }

    // Fallback: scan src dirs from srcs, then src/.
    let src_dirs: Vec<PathBuf> = manifest
        .and_then(|m| m.lib.as_ref())
        .map(|lib| {
            lib.srcs
                .iter()
                .map(|s| {
                    let p = dir.join(s);
                    if p.is_dir() {
                        p
                    } else {
                        p.parent()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| dir.to_path_buf())
                    }
                })
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();

    if !src_dirs.is_empty() {
        let mut items = Vec::new();
        for d in &src_dirs {
            items.extend(extract_dir(d).items);
        }
        return items;
    }

    let fallback = dir.join("src");
    if fallback.is_dir() {
        extract_dir(&fallback).items
    } else {
        extract_dir(dir).items
    }
}

fn read_readme(dir: &Path) -> Option<String> {
    for name in &["README.md", "Readme.md", "readme.md", "README"] {
        let p = dir.join(name);
        if p.is_file() {
            return std::fs::read_to_string(p).ok();
        }
    }
    None
}

fn collect_doc_dependencies(project_dir: &Path) -> Vec<DocDependency> {
    let mut deps = Vec::new();
    if let Ok(manifest) = load_manifest(project_dir) {
        collect_manifest_dependencies(project_dir, &manifest, "local", false, &mut deps);
        collect_manifest_dependencies(project_dir, &manifest, "local", true, &mut deps);
    }
    collect_global_dependencies(&mut deps);
    deps.sort_by(|a, b| (a.scope, &a.name).cmp(&(b.scope, &b.name)));
    deps.dedup_by(|a, b| a.scope == b.scope && a.name == b.name && a.path == b.path);
    deps
}

fn collect_manifest_dependencies(
    project_dir: &Path,
    manifest: &Manifest,
    scope: &'static str,
    dev: bool,
    out: &mut Vec<DocDependency>,
) {
    let deps: Vec<(String, Dependency)> = if dev {
        manifest
            .dev_dependencies
            .iter()
            .map(|(name, dep)| (name.clone(), dep.clone()))
            .collect()
    } else {
        manifest.effective_dependencies().into_iter().collect()
    };
    for (name, dep) in deps {
        let mut item = dependency_summary(project_dir, &name, &dep, scope);
        if dev {
            item.scope = "local-dev";
        }
        out.push(item);
    }
}

fn dependency_summary(
    project_dir: &Path,
    name: &str,
    dep: &Dependency,
    scope: &'static str,
) -> DocDependency {
    let (kind, version, source, path) = match dep {
        Dependency::Simple(version) => {
            let dir = project_dir.join(".pkgs").join(name);
            (
                "registry".to_string(),
                version.clone(),
                "freight.dev".to_string(),
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if freight::manifest::types::is_platform_dep(name) => (
            "platform".to_string(),
            d.version.clone().unwrap_or_else(|| "*".into()),
            name.to_string(),
            None,
        ),
        Dependency::Detailed(d) if d.path.is_some() => {
            let rel = d.path.as_deref().unwrap_or_default();
            let dir = project_dir.join(rel);
            (
                "path".to_string(),
                manifest_version(&dir)
                    .unwrap_or_else(|| d.version.clone().unwrap_or_else(|| "*".into())),
                rel.to_string(),
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if d.git.is_some() => {
            let dir = project_dir.join(".pkgs").join(name);
            let source = d.git.clone().unwrap_or_default();
            (
                "git".to_string(),
                git_ref(d),
                source,
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if d.url.is_some() => {
            let dir = project_dir.join(".pkgs").join(name);
            let source = d.url.clone().unwrap_or_default();
            (
                "url".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                source,
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) => {
            let dir = project_dir.join(".pkgs").join(name);
            (
                "registry".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                "freight.dev".to_string(),
                dir.exists().then_some(dir),
            )
        }
    };
    let docs = path.as_deref().map(find_doc_files).unwrap_or_default();
    DocDependency {
        name: name.to_string(),
        scope,
        kind,
        version,
        source,
        path,
        docs,
    }
}

fn collect_global_dependencies(out: &mut Vec<DocDependency>) {
    let Some(home) = freight_home() else {
        return;
    };
    for root in [
        home.join("deps"),
        home.join("registry"),
        home.join("registry").join("src"),
    ] {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = manifest_version(&dir).unwrap_or_else(|| "installed".into());
            let docs = find_doc_files(&dir);
            out.push(DocDependency {
                name,
                scope: "global",
                kind: "cached".into(),
                version,
                source: root.display().to_string(),
                path: Some(dir),
                docs,
            });
        }
    }
}

fn manifest_version(dir: &Path) -> Option<String> {
    load_manifest(dir).ok().map(|m| m.package.version)
}

fn git_ref(d: &freight::manifest::types::DetailedDep) -> String {
    d.rev
        .as_deref()
        .or(d.tag.as_deref())
        .or(d.branch.as_deref())
        .or(d.version.as_deref())
        .unwrap_or("*")
        .to_string()
}

fn find_doc_files(dir: &Path) -> Vec<PathBuf> {
    let candidates = [
        dir.join("target/doc/index.md"),
        dir.join("target/doc/index.html"),
        dir.join("docs/index.md"),
        dir.join("README.md"),
        dir.join("README"),
    ];
    candidates.into_iter().filter(|p| p.exists()).collect()
}

fn print_dependency_table(deps: &[DocDependency]) {
    println!("freight dependency docs");
    for dep in deps {
        let location = dep
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not installed on disk".into());
        println!(
            "- [{}] {} {} ({}) — {} from {}",
            dep.scope, dep.name, dep.version, dep.kind, location, dep.source
        );
    }
}
