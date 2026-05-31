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

use docify::extract::{extract_dir, DocItem, DocKind, DocSet};
use docify::render;
use freight_core::manifest::types::{Dependency, Manifest};
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::freight_home;

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

    match render(&combined, &out_dir) {
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

#[derive(Debug, Clone)]
struct DocDependency {
    name: String,
    scope: &'static str,
    kind: String,
    version: String,
    source: String,
    path: Option<PathBuf>,
    docs: Vec<PathBuf>,
}

fn open_dependency_tui() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let project_dir = find_manifest_dir(&cwd).unwrap_or(cwd);
    let deps = collect_doc_dependencies(&project_dir);

    if deps.is_empty() {
        print_warning("no installed local or global dependencies found");
        println!("hint: add dependencies to freight.toml and run `freight fetch`, or use `freight doc --format md` to generate API docs");
        return Ok(());
    }

    if !io::stdout().is_terminal() {
        print_dependency_table(&deps);
        return Ok(());
    }

    // Detect terminal image capabilities before entering raw mode.
    #[cfg(feature = "rich-math")]
    let ctx = {
        use ratatui_image::picker::{Picker, ProtocolType};
        let picker = Picker::from_query_stdio()
            .ok()
            .filter(|p| !matches!(p.protocol_type(), ProtocolType::Halfblocks));
        RenderCtx { picker }
    };
    #[cfg(not(feature = "rich-math"))]
    let ctx = RenderCtx::new();

    run_dependency_tui(&deps, ctx)
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
        Dependency::Detailed(d) if freight_core::manifest::types::is_platform_dep(name) => (
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

fn git_ref(d: &freight_core::manifest::types::DetailedDep) -> String {
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

// ── ratatui TUI ───────────────────────────────────────────────────────────────

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};

// ── Rich-math image types (feature-gated) ─────────────────────────────────────

enum ContentBlock {
    Lines(Vec<Line<'static>>),
    #[cfg(feature = "rich-math")]
    MathImage {
        state: ratatui_image::protocol::StatefulProtocol,
        height_lines: u16,
    },
}

impl ContentBlock {
    fn line_count(&self) -> usize {
        match self {
            Self::Lines(ls) => ls.len(),
            #[cfg(feature = "rich-math")]
            Self::MathImage { height_lines, .. } => *height_lines as usize,
        }
    }
}

struct RenderCtx {
    #[cfg(feature = "rich-math")]
    picker: Option<ratatui_image::picker::Picker>,
}

impl RenderCtx {
    #[allow(dead_code)]
    fn new() -> Self {
        RenderCtx {
            #[cfg(feature = "rich-math")]
            picker: None,
        }
    }
}

// ── TUI app types ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum TreeNodeKind {
    Dep,
    SectionHdr, // "Classes & Types", "Namespaces", "Free Symbols" — collapsible section header
    Group,      // namespace — navigates to NamespacePage when activated
    Symbol,
    Readme,
}

/// One node in the unified package/API tree.
struct TreeNode {
    label: String,
    depth: usize,
    kind: TreeNodeKind,
    expanded: bool,
    item_idx: Option<usize>, // for Symbol nodes: index into doc_items
    dep_idx: Option<usize>,  // for Dep nodes: index into deps
    loaded: bool,            // for Dep nodes: true once items have been loaded
}

/// Which panel currently holds keyboard focus.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Left,
    Content,
    Meta,
}

/// What the centre panel currently shows.
#[derive(Clone, PartialEq)]
enum NavMode {
    Welcome,
    Readme(usize),
    DepOverview(usize),
    TypePage(usize, usize),       // (dep_idx, item_idx)
    NamespacePage(usize, String), // (dep_idx, ns_name)
    SymbolDetail(usize, usize),   // (dep_idx, item_idx)
}

struct DocApp<'a> {
    deps: &'a [DocDependency],
    ctx: RenderCtx,
    focus: Focus,

    // Unified tree (deps as roots, namespaces + symbols as children)
    tree: Vec<TreeNode>,
    tree_cursor: usize,
    tree_offset: usize,
    tree_visible: usize,
    doc_items: Vec<DocItem>, // flat: all loaded items from all deps (append-only)

    // Per-dep item ranges — populated on first load; index matches deps[].
    dep_item_ranges: Vec<Option<(usize, usize)>>, // (offset, count) into doc_items

    // Which dep's content is currently shown in the centre panel.
    content_dep_idx: Option<usize>,
    nav_mode: NavMode,

    // Content (centre)
    blocks: Vec<ContentBlock>,
    total_lines: usize,
    scroll: usize,
    content_links: Vec<(usize, String)>, // virtual_line → link target name
    content_area: Rect,                  // updated each frame for click hit-testing
    tree_area: Rect,                     // updated each frame for click hit-testing (tree panel)
    item_vlines: Vec<usize>,             // sorted virtual line of each item section (for Tab)
    item_line_map: std::collections::HashMap<usize, usize>, // item_idx → first virtual line

    // Metadata (right panel)
    meta_lines: Vec<Line<'static>>,
    meta_scroll: usize,
    meta_visible: usize,
}

impl<'a> DocApp<'a> {
    fn new(deps: &'a [DocDependency], ctx: RenderCtx) -> Self {
        let tree = deps
            .iter()
            .enumerate()
            .map(|(i, dep)| TreeNode {
                label: format!("{} {}", dep.name, dep.version),
                depth: 0,
                kind: TreeNodeKind::Dep,
                expanded: false,
                item_idx: None,
                dep_idx: Some(i),
                loaded: false,
            })
            .collect();
        let n = deps.len();
        Self {
            deps,
            ctx,
            focus: Focus::Left,
            tree,
            tree_cursor: 0,
            tree_offset: 0,
            tree_visible: 0,
            doc_items: Vec::new(),
            dep_item_ranges: vec![None; n],
            content_dep_idx: None,
            nav_mode: NavMode::Welcome,
            blocks: Vec::new(),
            total_lines: 0,
            scroll: 0,
            content_links: Vec::new(),
            content_area: Rect::default(),
            tree_area: Rect::default(),
            item_vlines: Vec::new(),
            item_line_map: std::collections::HashMap::new(),
            meta_lines: Vec::new(),
            meta_scroll: 0,
            meta_visible: 0,
        }
    }

    /// Extract items and build the sidebar sub-tree for a dep, if not already done.
    fn load_dep_if_needed(&mut self, tree_idx: usize) {
        if self.tree[tree_idx].loaded {
            return;
        }
        let dep_idx = self.tree[tree_idx].dep_idx.unwrap();

        let item_offset = self.doc_items.len();
        let new_items = extract_dep_items(&self.deps[dep_idx]);
        let item_count = new_items.len();
        self.doc_items.extend(new_items);
        self.dep_item_ranges[dep_idx] = Some((item_offset, item_count));

        let sub = build_api_subtree(
            &self.doc_items[item_offset..item_offset + item_count],
            item_offset,
            1,
            dep_idx,
        );

        let has_readme = readme_exists(&self.deps[dep_idx]);
        let mut ins = tree_idx + 1;
        if has_readme {
            self.tree.insert(
                ins,
                TreeNode {
                    label: "README".to_string(),
                    depth: 1,
                    kind: TreeNodeKind::Readme,
                    expanded: false,
                    item_idx: None,
                    dep_idx: Some(dep_idx),
                    loaded: false,
                },
            );
            ins += 1;
        }
        for (i, node) in sub.into_iter().enumerate() {
            self.tree.insert(ins + i, node);
        }
        self.tree[tree_idx].loaded = true;
    }

    /// Activate a Dep node: load items (first time), show overview page, toggle expanded.
    fn open_dep_node(&mut self, tree_idx: usize) {
        let dep_idx = self.tree[tree_idx].dep_idx.unwrap();
        self.meta_lines = render_pkg_meta(&self.deps[dep_idx]);
        self.meta_scroll = 0;

        self.load_dep_if_needed(tree_idx);

        self.nav_mode = NavMode::DepOverview(dep_idx);
        self.rebuild_content();
        self.tree[tree_idx].expanded = !self.tree[tree_idx].expanded;
        self.focus = Focus::Content;
    }

    /// Rebuild `blocks` / `links` / `vlines` from `self.nav_mode`.
    fn rebuild_content(&mut self) {
        self.blocks.clear();
        self.content_links.clear();
        self.item_vlines.clear();
        self.item_line_map.clear();
        self.scroll = 0;

        let mode = self.nav_mode.clone();
        match mode {
            NavMode::Welcome => {}

            NavMode::Readme(dep_idx) => {
                let (rb, rl) = load_readme_content(&self.deps[dep_idx], &self.ctx);
                self.blocks.extend(rb);
                self.content_links.extend(rl);
                self.content_dep_idx = Some(dep_idx);
            }

            NavMode::DepOverview(dep_idx) => {
                if let Some((offset, count)) = self.dep_item_ranges[dep_idx] {
                    let (blks, lnks) =
                        render_overview_blocks(&self.deps[dep_idx], &self.doc_items[offset..offset + count]);
                    self.blocks = blks;
                    self.content_links = lnks;
                }
                self.content_dep_idx = Some(dep_idx);
            }

            NavMode::TypePage(dep_idx, item_idx) => {
                if let Some((offset, count)) = self.dep_item_ranges[dep_idx] {
                    if item_idx < self.doc_items.len() {
                        let (blks, lnks) = render_type_page_blocks(
                            &self.doc_items[item_idx],
                            item_idx,
                            &self.doc_items[offset..offset + count],
                            offset,
                            &self.ctx,
                        );
                        self.blocks = blks;
                        self.content_links = lnks;
                    }
                }
                self.content_dep_idx = Some(dep_idx);
            }

            NavMode::NamespacePage(dep_idx, ref ns) => {
                let ns = ns.clone();
                if let Some((offset, count)) = self.dep_item_ranges[dep_idx] {
                    let (blks, lnks) =
                        render_ns_page_blocks(&ns, &self.doc_items[offset..offset + count], offset, &self.ctx);
                    self.blocks = blks;
                    self.content_links = lnks;
                }
                self.content_dep_idx = Some(dep_idx);
            }

            NavMode::SymbolDetail(dep_idx, item_idx) => {
                if self.dep_item_ranges[dep_idx].is_some() {
                    if item_idx < self.doc_items.len() {
                        let item = self.doc_items[item_idx].clone();
                        load_all_items(
                            std::slice::from_ref(&item),
                            item_idx,
                            &self.ctx,
                            &mut self.blocks,
                            &mut self.content_links,
                            &mut self.item_vlines,
                            &mut self.item_line_map,
                        );
                    }
                }
                self.content_dep_idx = Some(dep_idx);
            }
        }

        self.total_lines = self.blocks.iter().map(ContentBlock::line_count).sum();
    }

    fn open_tree_item(&mut self) {
        let vis = visible_nodes(&self.tree);
        let Some(&idx) = vis.get(self.tree_cursor) else {
            return;
        };

        match self.tree[idx].kind {
            TreeNodeKind::Dep => {
                self.open_dep_node(idx);
                let new_len = visible_nodes(&self.tree).len();
                self.tree_cursor = self.tree_cursor.min(new_len.saturating_sub(1));
            }
            TreeNodeKind::SectionHdr => {
                self.tree[idx].expanded = !self.tree[idx].expanded;
                let new_len = visible_nodes(&self.tree).len();
                self.tree_cursor = self.tree_cursor.min(new_len.saturating_sub(1));
            }
            TreeNodeKind::Group => {
                // Navigate to namespace page.
                if let Some(dep_idx) = self.tree[idx].dep_idx {
                    let ns = self.tree[idx].label.clone();
                    self.nav_mode = NavMode::NamespacePage(dep_idx, ns);
                    self.rebuild_content();
                    self.focus = Focus::Content;
                }
            }
            TreeNodeKind::Symbol => {
                if let (Some(item_idx), Some(dep_idx)) =
                    (self.tree[idx].item_idx, self.tree[idx].dep_idx)
                {
                    let item = &self.doc_items[item_idx];
                    self.nav_mode = if is_type_kind(&item.kind) {
                        NavMode::TypePage(dep_idx, item_idx)
                    } else {
                        NavMode::SymbolDetail(dep_idx, item_idx)
                    };
                    self.rebuild_content();
                    self.focus = Focus::Content;
                }
            }
            TreeNodeKind::Readme => {
                let dep_idx = self.tree[idx].dep_idx.unwrap();
                self.nav_mode = NavMode::Readme(dep_idx);
                self.rebuild_content();
                self.focus = Focus::Content;
            }
        }
    }

    fn jump_to_item(&mut self, item_idx: usize) {
        let dep_idx = self.dep_item_ranges.iter().position(|r| {
            r.map_or(false, |(off, cnt)| item_idx >= off && item_idx < off + cnt)
        });
        if let Some(dep_idx) = dep_idx {
            let item = &self.doc_items[item_idx];
            self.nav_mode = if is_type_kind(&item.kind) {
                NavMode::TypePage(dep_idx, item_idx)
            } else {
                NavMode::SymbolDetail(dep_idx, item_idx)
            };
            self.rebuild_content();
            self.focus = Focus::Content;
        }
    }

    fn navigate_link(&mut self, target: &str) {
        let found = self.doc_items.iter().position(|item| {
            item.name == target
                || item.name.ends_with(&format!("::{target}"))
                || item.name.ends_with(&format!(".{target}"))
        });
        if let Some(item_idx) = found {
            self.jump_to_item(item_idx);
        }
    }

    /// Advance scroll to the next function section (Tab in content panel).
    fn next_declaration(&mut self) {
        if self.item_vlines.is_empty() {
            return;
        }
        let next = self
            .item_vlines
            .iter()
            .find(|&&vl| vl > self.scroll)
            .copied()
            .unwrap_or(self.item_vlines[0]); // wrap around
        self.scroll = next;
    }

    /// Scroll back to the previous function section (Shift-Tab in content panel).
    fn prev_declaration(&mut self) {
        if self.item_vlines.is_empty() {
            return;
        }
        let prev = self
            .item_vlines
            .iter()
            .rev()
            .find(|&&vl| vl < self.scroll)
            .copied()
            .unwrap_or(*self.item_vlines.last().unwrap()); // wrap around
        self.scroll = prev;
    }
}

fn run_dependency_tui(deps: &[DocDependency], ctx: RenderCtx) -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_doc_app(&mut terminal, deps, ctx);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen,
    )?;
    terminal.show_cursor()?;
    result
}

fn run_doc_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    deps: &[DocDependency],
    ctx: RenderCtx,
) -> anyhow::Result<()> {
    let mut app = DocApp::new(deps, ctx);

    loop {
        terminal.draw(|frame| draw_app(frame, &mut app))?;

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if handle_key(&mut app, key) {
                    break;
                }
            }
            Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                handle_mouse_click(&mut app, m.column, m.row);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Returns `true` when the app should quit.
fn handle_key(app: &mut DocApp<'_>, key: KeyEvent) -> bool {
    match key {
        KeyEvent {
            code: KeyCode::Char('q'),
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            ..
        } => return true,

        // Left arrow → tree panel; Right arrow → content panel.
        KeyEvent {
            code: KeyCode::Left,
            ..
        } => {
            app.focus = Focus::Left;
        }
        KeyEvent {
            code: KeyCode::Right,
            ..
        } => {
            app.focus = Focus::Content;
        }

        // Tab: Left→next dep node; Content→next declaration; Meta→Left.
        // Shift-Tab: Left→prev dep node; Content→prev declaration; Meta→Left.
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            ..
        } => match app.focus {
            Focus::Left => {
                let vis = visible_nodes(&app.tree);
                let next = vis
                    .iter()
                    .position(|&ti| {
                        app.tree[ti].kind == TreeNodeKind::Dep
                            && ti > vis.get(app.tree_cursor).copied().unwrap_or(0)
                    })
                    .unwrap_or(app.tree_cursor);
                app.tree_cursor = next;
            }
            Focus::Content => {
                app.next_declaration();
            }
            Focus::Meta => {
                app.focus = Focus::Left;
            }
        },
        KeyEvent {
            code: KeyCode::BackTab,
            ..
        } => match app.focus {
            Focus::Left => {
                let vis = visible_nodes(&app.tree);
                let cur_ti = vis.get(app.tree_cursor).copied().unwrap_or(0);
                let prev = vis
                    .iter()
                    .rposition(|&ti| app.tree[ti].kind == TreeNodeKind::Dep && ti < cur_ti)
                    .unwrap_or(app.tree_cursor);
                app.tree_cursor = prev;
            }
            Focus::Content => {
                app.prev_declaration();
            }
            _ => {
                app.focus = Focus::Left;
            }
        },

        KeyEvent {
            code: KeyCode::Esc, ..
        }
        | KeyEvent {
            code: KeyCode::Backspace,
            ..
        } => {
            app.focus = Focus::Left;
        }

        _ => match app.focus {
            Focus::Left => handle_left_key(app, key),
            Focus::Content => handle_content_key(app, key),
            Focus::Meta => handle_meta_key(app, key),
        },
    }
    false
}

fn handle_left_key(app: &mut DocApp<'_>, key: KeyEvent) {
    match key {
        KeyEvent {
            code: KeyCode::Up, ..
        }
        | KeyEvent {
            code: KeyCode::Char('k'),
            ..
        } => {
            app.tree_cursor = app.tree_cursor.saturating_sub(1);
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('j'),
            ..
        } => {
            let vis_len = visible_nodes(&app.tree).len();
            app.tree_cursor = (app.tree_cursor + 1).min(vis_len.saturating_sub(1));
        }
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => {
            let v = app.tree_visible.max(1);
            app.tree_cursor = app.tree_cursor.saturating_sub(v);
        }
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        } => {
            let v = app.tree_visible.max(1);
            let n = visible_nodes(&app.tree).len().saturating_sub(1);
            app.tree_cursor = (app.tree_cursor + v).min(n);
        }
        KeyEvent {
            code: KeyCode::Home,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('g'),
            ..
        } => {
            app.tree_cursor = 0;
        }
        KeyEvent {
            code: KeyCode::End, ..
        }
        | KeyEvent {
            code: KeyCode::Char('G'),
            ..
        } => {
            app.tree_cursor = visible_nodes(&app.tree).len().saturating_sub(1);
        }
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            app.open_tree_item();
        }
        _ => {}
    }
}

fn handle_content_key(app: &mut DocApp<'_>, key: KeyEvent) {
    let total = app.total_lines;
    match key {
        KeyEvent {
            code: KeyCode::Up, ..
        }
        | KeyEvent {
            code: KeyCode::Char('k'),
            ..
        } => {
            app.scroll = app.scroll.saturating_sub(1);
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('j'),
            ..
        } => {
            app.scroll = (app.scroll + 1).min(total.saturating_sub(1));
        }
        KeyEvent {
            code: KeyCode::PageUp,
            ..
        } => {
            app.scroll = app.scroll.saturating_sub(20);
        }
        KeyEvent {
            code: KeyCode::PageDown,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char(' '),
            ..
        } => {
            app.scroll = (app.scroll + 20).min(total.saturating_sub(1));
        }
        KeyEvent {
            code: KeyCode::Home,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('g'),
            ..
        } => {
            app.scroll = 0;
        }
        KeyEvent {
            code: KeyCode::End, ..
        }
        | KeyEvent {
            code: KeyCode::Char('G'),
            ..
        } => {
            app.scroll = total.saturating_sub(1);
        }
        // Enter: follow the first link visible on screen (at or after current scroll).
        KeyEvent {
            code: KeyCode::Enter,
            ..
        } => {
            let visible_h = app.content_area.height as usize;
            let lo = app.scroll;
            let hi = app.scroll + visible_h;
            if let Some((_, target)) = app
                .content_links
                .iter()
                .find(|(vl, _)| *vl >= lo && *vl < hi)
            {
                let target = target.clone();
                app.navigate_link(&target);
            }
        }
        _ => {}
    }
}

fn handle_meta_key(app: &mut DocApp<'_>, key: KeyEvent) {
    let total = app.meta_lines.len();
    match key {
        KeyEvent {
            code: KeyCode::Up, ..
        }
        | KeyEvent {
            code: KeyCode::Char('k'),
            ..
        } => {
            app.meta_scroll = app.meta_scroll.saturating_sub(1);
        }
        KeyEvent {
            code: KeyCode::Down,
            ..
        }
        | KeyEvent {
            code: KeyCode::Char('j'),
            ..
        } => {
            app.meta_scroll = (app.meta_scroll + 1).min(total.saturating_sub(1));
        }
        KeyEvent {
            code: KeyCode::Home,
            ..
        } => {
            app.meta_scroll = 0;
        }
        KeyEvent {
            code: KeyCode::End, ..
        } => {
            app.meta_scroll = total.saturating_sub(1);
        }
        _ => {}
    }
}

fn handle_mouse_click(app: &mut DocApp<'_>, col: u16, row: u16) {
    // Tree panel: click selects and activates the item (same as Enter).
    let t = app.tree_area;
    if t.width > 0 && col >= t.x && col < t.x + t.width && row >= t.y && row < t.y + t.height {
        let item_offset = (row - t.y) as usize;
        let vis = visible_nodes(&app.tree);
        let vis_idx = app.tree_offset + item_offset;
        if vis_idx < vis.len() {
            app.tree_cursor = vis_idx;
            app.open_tree_item();
        }
        return;
    }

    // Content panel: click follows a link at the clicked virtual line.
    let a = app.content_area;
    if a.width == 0 {
        return;
    }
    if col >= a.x && col < a.x + a.width && row >= a.y && row < a.y + a.height {
        let vline = app.scroll + (row - a.y) as usize;
        if let Some((_, target)) = app.content_links.iter().find(|(vl, _)| *vl == vline) {
            let target = target.clone();
            app.navigate_link(&target);
        }
    }
}

fn draw_app(frame: &mut ratatui::Frame, app: &mut DocApp<'_>) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Min(10),
            Constraint::Percentage(20),
        ])
        .split(rows[0]);

    draw_tree(frame, app, cols[0]);
    draw_content(frame, app, cols[1]);
    draw_meta(frame, app, cols[2]);

    draw_help_bar(frame, app, rows[1]);
}

fn draw_help_bar(frame: &mut ratatui::Frame, app: &DocApp<'_>, area: Rect) {
    let text = match app.focus {
        Focus::Left => "↑/↓  PgUp/Dn  Tab next pkg  Enter open  →content  q quit",
        Focus::Content => "↑/↓  PgUp/Dn  Tab next section  Enter/click link  ← tree  q quit",
        Focus::Meta => "↑/↓  Tab/← focus  q quit",
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

/// Unified package/API tree (left panel).
fn draw_tree(frame: &mut ratatui::Frame, app: &mut DocApp<'_>, area: Rect) {
    let focused = app.focus == Focus::Left;
    let vis = visible_nodes(&app.tree);
    let title = format!("Packages ({})", app.deps.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.tree_area = inner;

    let visible = inner.height as usize;
    app.tree_visible = visible;

    if app.tree_cursor < app.tree_offset {
        app.tree_offset = app.tree_cursor;
    } else if visible > 0 && app.tree_cursor >= app.tree_offset + visible {
        app.tree_offset = app.tree_cursor + 1 - visible;
    }

    let items: Vec<ListItem> = vis
        .iter()
        .skip(app.tree_offset)
        .take(visible)
        .map(|&ti| {
            let node = &app.tree[ti];
            let pad = "  ".repeat(node.depth);
            match node.kind {
                TreeNodeKind::Dep => {
                    let scope_color = node
                        .dep_idx
                        .and_then(|i| app.deps.get(i))
                        .map(|d| match d.scope {
                            "local" => Color::Cyan,
                            "local-dev" => Color::Blue,
                            "global" => Color::DarkGray,
                            _ => Color::Reset,
                        })
                        .unwrap_or(Color::Reset);
                    let arrow = if node.expanded { "▾ " } else { "▸ " };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{pad}{arrow}"), Style::default().fg(scope_color)),
                        Span::styled(
                            node.label.clone(),
                            Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD),
                        ),
                    ]))
                }
                TreeNodeKind::SectionHdr => {
                    let arrow = if node.expanded { "▾ " } else { "▸ " };
                    ListItem::new(Line::from(vec![
                        Span::raw(pad),
                        Span::styled(
                            format!("{arrow}{}", node.label),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        ),
                    ]))
                }
                TreeNodeKind::Group => {
                    // Namespace node — non-expandable, navigates to NamespacePage.
                    ListItem::new(Line::from(vec![
                        Span::raw(pad),
                        Span::styled("[ns]  ", Style::default().fg(Color::Yellow)),
                        Span::raw(node.label.clone()),
                    ]))
                }
                TreeNodeKind::Symbol => {
                    let (badge, badge_color) = node
                        .item_idx
                        .and_then(|ii| app.doc_items.get(ii))
                        .map(|it| kind_badge_info(&it.kind))
                        .unwrap_or(("[???]", Color::DarkGray));
                    ListItem::new(Line::from(vec![
                        Span::raw(pad),
                        Span::styled(badge, Style::default().fg(badge_color)),
                        Span::raw("  "),
                        Span::raw(node.label.clone()),
                    ]))
                }
                TreeNodeKind::Readme => ListItem::new(Line::from(vec![
                    Span::raw(pad),
                    Span::styled("[doc]  ", Style::default().fg(Color::Cyan)),
                    Span::raw("README"),
                ])),
            }
        })
        .collect();

    let sel_in_view = app.tree_cursor.saturating_sub(app.tree_offset);
    let mut state = ListState::default().with_offset(0);
    state.select(Some(sel_in_view));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::default())
            .highlight_symbol(""),
        inner,
        &mut state,
    );
}

fn content_title(app: &DocApp<'_>) -> String {
    match &app.nav_mode {
        NavMode::Welcome => "docs".to_string(),
        NavMode::Readme(di) => format!(
            "README — {}",
            app.deps.get(*di).map(|d| d.name.as_str()).unwrap_or("")
        ),
        NavMode::DepOverview(di) => format!(
            "Overview — {}",
            app.deps.get(*di).map(|d| d.name.as_str()).unwrap_or("")
        ),
        NavMode::TypePage(_, ii) => {
            let item = app.doc_items.get(*ii);
            format!(
                "{} — {}",
                item.map(|i| i.kind.label()).unwrap_or("type"),
                item.map(|i| local_name_of(&i.name)).unwrap_or("")
            )
        }
        NavMode::NamespacePage(_, ns) => format!("namespace — {ns}"),
        NavMode::SymbolDetail(_, ii) => {
            let item = app.doc_items.get(*ii);
            format!(
                "{} — {}",
                item.map(|i| i.kind.label()).unwrap_or("item"),
                item.map(|i| local_name_of(&i.name)).unwrap_or("")
            )
        }
    }
}

fn draw_content(frame: &mut ratatui::Frame, app: &mut DocApp<'_>, area: Rect) {
    let focused = app.focus == Focus::Content;
    let outer = Block::default()
        .title(content_title(app))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Style::default()
        } else {
            Style::default().fg(Color::DarkGray)
        });
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    app.content_area = inner; // store for mouse click hit-testing

    if app.blocks.is_empty() {
        frame.render_widget(
            Paragraph::new("Select a dependency and press Enter to view docs.")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let scroll = app.scroll;
    let visible_h = inner.height as usize;
    let mut vy: usize = 0;

    for block in app.blocks.iter_mut() {
        let bh = block.line_count();
        let bstart = vy;
        let bend = vy + bh;
        vy = bend;

        if bend <= scroll || bstart >= scroll + visible_h {
            continue;
        }

        let screen_top = bstart.saturating_sub(scroll);
        let skip = scroll.saturating_sub(bstart);

        match block {
            ContentBlock::Lines(lines) => {
                let take = (bh - skip).min(visible_h - screen_top);
                let rect = Rect {
                    x: inner.x,
                    y: inner.y + screen_top as u16,
                    width: inner.width,
                    height: take as u16,
                };
                frame.render_widget(Paragraph::new(lines[skip..skip + take].to_vec()), rect);
            }
            #[cfg(feature = "rich-math")]
            ContentBlock::MathImage {
                state,
                height_lines,
            } => {
                let rect = Rect {
                    x: inner.x,
                    y: inner.y + screen_top as u16,
                    width: inner.width,
                    height: ((visible_h - screen_top) as u16).min(*height_lines),
                };
                frame.render_stateful_widget(ratatui_image::StatefulImage::default(), rect, state);
            }
        }
    }
}

fn draw_meta(frame: &mut ratatui::Frame, app: &mut DocApp<'_>, area: Rect) {
    let focused = app.focus == Focus::Meta;
    let block = Block::default()
        .title(if focused { "Info [focus]" } else { "Info" })
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.meta_lines.is_empty() {
        frame.render_widget(
            Paragraph::new("  (open a dep)").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let visible = inner.height as usize;
    app.meta_visible = visible;
    app.meta_scroll = app.meta_scroll.min(app.meta_lines.len().saturating_sub(1));

    let vis: Vec<Line<'static>> = app
        .meta_lines
        .iter()
        .skip(app.meta_scroll)
        .take(visible)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(vis), inner);
}

// ── Image rendering (feature-gated) ──────────────────────────────────────────

#[cfg(feature = "rich-math")]
fn latex_display_to_block(
    latex: &str,
    picker: &ratatui_image::picker::Picker,
) -> Option<ContentBlock> {
    use resvg::{tiny_skia, usvg};

    let svg = mathjax_svg_rs::render_tex(
        latex,
        &mathjax_svg_rs::Options {
            font_size: 32.0,
            ..Default::default()
        },
    )
    .ok()?;

    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).ok()?;
    let w = tree.size().width() as u32;
    let h = tree.size().height() as u32;
    if w == 0 || h == 0 {
        return None;
    }

    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());

    let rgba = pixmap.take();
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let dyn_img = image::DynamicImage::ImageRgba8(img);

    let cell_h = picker.font_size().height.max(1);
    let height_lines = ((h + cell_h as u32 - 1) / cell_h as u32) as u16;

    let state = picker.new_resize_protocol(dyn_img);
    Some(ContentBlock::MathImage {
        state,
        height_lines,
    })
}

// ── Doc content loading ───────────────────────────────────────────────────────

/// Render all `items` into a single content area, sorted Doxygen-style:
/// language → namespace → kind (Types, Functions, Variables), with section
/// header lines emitted before each group transition.
fn load_all_items(
    items: &[DocItem],
    item_offset: usize,
    ctx: &RenderCtx,
    out_blocks: &mut Vec<ContentBlock>,
    out_links: &mut Vec<(usize, String)>,
    out_vlines: &mut Vec<usize>,
    out_line_map: &mut std::collections::HashMap<usize, usize>,
) {
    // Build a display-order list: (lang, ns, kind_rank, local_name, idx)
    let mut order: Vec<(String, String, u8, String, usize)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| !it.name.is_empty())
        .map(|(i, it)| {
            let lang = it.lang.label().to_string();
            let ns = it.name.rfind("::").or_else(|| it.name.rfind('.'))
                .map_or(String::new(), |p| it.name[..p].to_string());
            let local = it.name.rfind("::").or_else(|| it.name.rfind('.'))
                .map_or(it.name.as_str(), |p| &it.name[p + 2..]);
            (lang, ns, kind_rank(&it.kind), local.to_lowercase(), item_offset + i)
        })
        .collect();
    order.sort_by(|a, b| (a.0.as_str(), a.1.as_str(), a.2, a.3.as_str())
        .cmp(&(b.0.as_str(), b.1.as_str(), b.2, b.3.as_str())));

    let mut last_lang: &str = "";
    let mut last_ns:   &str = "";
    let mut last_rank: Option<u8> = None;

    for (ref lang, ref ns, rank, _, idx) in &order {
        // Language section header
        if lang.as_str() != last_lang {
            out_blocks.push(ContentBlock::Lines(vec![
                Line::raw(""),
                section_rule_line(lang),
                Line::raw(""),
            ]));
            last_lang = lang.as_str();
            last_ns   = "";
            last_rank = None;
        }
        // Namespace sub-header (only when namespace is non-empty)
        if !ns.is_empty() && ns.as_str() != last_ns {
            out_blocks.push(ContentBlock::Lines(vec![
                heading_line(pulldown_cmark::HeadingLevel::H3, ns),
                Line::raw(""),
            ]));
            last_ns   = ns.as_str();
            last_rank = None;
        }
        // Kind sub-header
        if Some(*rank) != last_rank {
            last_rank = Some(*rank);
            let label = kind_section_label(*rank);
            out_blocks.push(ContentBlock::Lines(vec![
                section_kind_line(label),
                Line::raw(""),
            ]));
        }

        let item = &items[idx - item_offset];
        let start_vline: usize = out_blocks.iter().map(|b| b.line_count()).sum();
        out_vlines.push(start_vline);
        out_line_map.insert(*idx, start_vline);

        let md = docify::render_tui::items_to_markdown(std::slice::from_ref(item));
        let (item_blocks, item_links) = markdown_to_blocks(&md, 80, ctx);
        for (vl, target) in item_links {
            out_links.push((start_vline + vl, target));
        }
        out_blocks.extend(item_blocks);
    }
}

/// Horizontal rule with centred language label — e.g. `──── C++ ────`.
fn section_rule_line(lang: &str) -> Line<'static> {
    let label = format!("  {}  ", lang);
    let bars  = "─".repeat(6);
    Line::from(vec![
        Span::styled(bars.clone(), Style::default().fg(Color::DarkGray)),
        Span::styled(label.to_owned(), Style::default().fg(Color::LightCyan).add_modifier(Modifier::BOLD)),
        Span::styled(bars, Style::default().fg(Color::DarkGray)),
    ])
}

/// Doxygen-style kind separator, e.g. `· Functions ················`.
fn section_kind_line(label: &str) -> Line<'static> {
    let prefix = format!("· {} ", label);
    let dots   = "·".repeat(50_usize.saturating_sub(prefix.chars().count()));
    Line::from(vec![
        Span::styled("· ".to_owned(), Style::default().fg(Color::DarkGray)),
        Span::styled(label.to_owned(), Style::default().fg(Color::Yellow)),
        Span::styled(format!(" {}", dots), Style::default().fg(Color::DarkGray)),
    ])
}

fn kind_rank(kind: &DocKind) -> u8 {
    match kind {
        DocKind::Class | DocKind::Struct | DocKind::Interface
        | DocKind::Enum | DocKind::Typedef => 0,
        DocKind::Module => 1,
        DocKind::Function | DocKind::Subroutine | DocKind::Macro => 2,
        DocKind::Variable => 3,
        _ => 4,
    }
}

fn kind_section_label(rank: u8) -> &'static str {
    match rank {
        0 => "Types",
        1 => "Modules",
        2 => "Functions",
        3 => "Variables",
        _ => "Other",
    }
}

/// Return `true` if `dep` has any readable README / doc file.
fn readme_exists(dep: &DocDependency) -> bool {
    if dep
        .docs
        .iter()
        .any(|p| !p.extension().map_or(false, |e| e == "html"))
    {
        return true;
    }
    dep.path.as_ref().map_or(false, |root| {
        ["README.md", "readme.md", "README"]
            .iter()
            .any(|n| root.join(n).exists())
    })
}

/// Load the README / markdown docs for the centre panel when a dep is first opened.
fn load_readme_content(
    dep: &DocDependency,
    ctx: &RenderCtx,
) -> (Vec<ContentBlock>, Vec<(usize, String)>) {
    // Prefer pre-generated doc files (target/doc/index.md, docs/index.md, README.md, …)
    for doc_path in &dep.docs {
        if let Ok(text) = std::fs::read_to_string(doc_path) {
            if !text.is_empty() {
                return markdown_to_blocks(&text, 80, ctx);
            }
        }
    }
    // Also try README.md directly in the dep root if not in docs list
    if let Some(root) = &dep.path {
        for name in &["README.md", "readme.md", "README"] {
            let p = root.join(name);
            if !dep.docs.contains(&p) {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    if !text.is_empty() {
                        return markdown_to_blocks(&text, 80, ctx);
                    }
                }
            }
        }
    }
    (
        vec![ContentBlock::Lines(vec![
            Line::raw(format!("No README found for '{}'.", dep.name)),
            Line::raw(""),
            Line::raw("Use [2] Files to browse the API tree."),
        ])],
        Vec::new(),
    )
}

/// Extract all doc items from a dependency's source tree.
fn extract_dep_items(dep: &DocDependency) -> Vec<DocItem> {
    let Some(dep_dir) = &dep.path else {
        return Vec::new();
    };
    let src_dir = dep_dir.join("src");
    let scan_dir = if src_dir.is_dir() {
        src_dir
    } else {
        dep_dir.clone()
    };
    docify::extract::extract_dir(&scan_dir).items
}

/// Build API sub-tree nodes for one dependency's items, mirroring the Doxygen
/// web sidebar: three collapsible sections (Classes & Types / Namespaces / Free
/// Symbols).  Empty sections are omitted.
///
/// `item_offset` is the position of `items[0]` in the global `doc_items` vec.
/// `base_depth` is the depth of the generated nodes (1 for direct dep children).
/// `dep_idx` is stored on every generated node for navigation.
fn build_api_subtree(
    items: &[DocItem],
    item_offset: usize,
    base_depth: usize,
    dep_idx: usize,
) -> Vec<TreeNode> {
    use std::collections::BTreeSet;

    let mut type_indices: Vec<usize> = Vec::new();
    let mut namespaces: BTreeSet<String> = BTreeSet::new();
    let mut free_indices: Vec<usize> = Vec::new();

    for (i, item) in items.iter().enumerate() {
        if item.name.is_empty() {
            continue;
        }
        let global = item_offset + i;
        let has_ns = item.name.contains("::") || item.name.contains('.');

        if is_type_kind(&item.kind) {
            type_indices.push(global);
        } else if has_ns {
            // Collect the top-level namespace prefix.
            let p = item.name.rfind("::").or_else(|| item.name.rfind('.')).unwrap();
            namespaces.insert(item.name[..p].to_string());
        } else {
            free_indices.push(global);
        }
    }

    // Sort type and free lists by local name.
    type_indices.sort_by(|&a, &b| {
        local_name_of(&items[a - item_offset].name)
            .cmp(local_name_of(&items[b - item_offset].name))
    });
    free_indices.sort_by(|&a, &b| {
        local_name_of(&items[a - item_offset].name)
            .cmp(local_name_of(&items[b - item_offset].name))
    });

    let mut tree: Vec<TreeNode> = Vec::new();
    let sym_depth = base_depth + 1;

    // --- Classes & Types ---
    if !type_indices.is_empty() {
        tree.push(TreeNode {
            label: "Classes & Types".to_string(),
            depth: base_depth,
            kind: TreeNodeKind::SectionHdr,
            expanded: true,
            item_idx: None,
            dep_idx: Some(dep_idx),
            loaded: false,
        });
        for global in type_indices {
            let item = &items[global - item_offset];
            tree.push(TreeNode {
                label: local_name_of(&item.name).to_string(),
                depth: sym_depth,
                kind: TreeNodeKind::Symbol,
                expanded: false,
                item_idx: Some(global),
                dep_idx: Some(dep_idx),
                loaded: false,
            });
        }
    }

    // --- Namespaces ---
    if !namespaces.is_empty() {
        tree.push(TreeNode {
            label: "Namespaces".to_string(),
            depth: base_depth,
            kind: TreeNodeKind::SectionHdr,
            expanded: true,
            item_idx: None,
            dep_idx: Some(dep_idx),
            loaded: false,
        });
        for ns in &namespaces {
            tree.push(TreeNode {
                label: ns.clone(),
                depth: sym_depth,
                kind: TreeNodeKind::Group,
                expanded: false,
                item_idx: None,
                dep_idx: Some(dep_idx),
                loaded: false,
            });
        }
    }

    // --- Free Symbols ---
    if !free_indices.is_empty() {
        tree.push(TreeNode {
            label: "Free Symbols".to_string(),
            depth: base_depth,
            kind: TreeNodeKind::SectionHdr,
            expanded: true,
            item_idx: None,
            dep_idx: Some(dep_idx),
            loaded: false,
        });
        for global in free_indices {
            let item = &items[global - item_offset];
            tree.push(TreeNode {
                label: local_name_of(&item.name).to_string(),
                depth: sym_depth,
                kind: TreeNodeKind::Symbol,
                expanded: false,
                item_idx: Some(global),
                dep_idx: Some(dep_idx),
                loaded: false,
            });
        }
    }

    tree
}

/// Return indices of currently-visible tree nodes, respecting collapsed sections and deps.
fn visible_nodes(tree: &[TreeNode]) -> Vec<usize> {
    let mut vis = Vec::new();
    let mut skip: Option<usize> = None;
    for (i, node) in tree.iter().enumerate() {
        if let Some(d) = skip {
            if node.depth > d {
                continue;
            }
            skip = None;
        }
        vis.push(i);
        if matches!(node.kind, TreeNodeKind::SectionHdr | TreeNodeKind::Dep) && !node.expanded {
            skip = Some(node.depth);
        }
    }
    vis
}

// ── Doxygen-style page renderers ──────────────────────────────────────────────

fn is_type_kind(kind: &DocKind) -> bool {
    matches!(
        kind,
        DocKind::Class | DocKind::Struct | DocKind::Interface | DocKind::Enum | DocKind::Typedef
    )
}

fn local_name_of(name: &str) -> &str {
    name.rfind("::").or_else(|| name.rfind('.'))
        .map(|p| &name[p + 2..])
        .unwrap_or(name)
}

fn kind_badge_info(kind: &DocKind) -> (&'static str, Color) {
    match kind {
        DocKind::Class     => ("[cls]", Color::LightGreen),
        DocKind::Struct    => ("[str]", Color::LightGreen),
        DocKind::Enum      => ("[enm]", Color::LightMagenta),
        DocKind::Interface => ("[ifc]", Color::LightYellow),
        DocKind::Typedef   => ("[typ]", Color::Cyan),
        DocKind::Module    => ("[mod]", Color::Yellow),
        DocKind::Function  => ("[fn] ", Color::LightBlue),
        DocKind::Subroutine => ("[sub]", Color::LightBlue),
        DocKind::Macro     => ("[mac]", Color::LightRed),
        DocKind::Variable  => ("[var]", Color::Gray),
        DocKind::Unknown   => ("[???]", Color::DarkGray),
    }
}

/// Render one item as a single summary row: badge  name  brief.
fn item_row_line(item: &DocItem, width: usize) -> Line<'static> {
    let (badge, badge_color) = kind_badge_info(&item.kind);
    let local = local_name_of(&item.name).to_string();
    let brief = item.brief.as_str().trim().to_string();

    let name_w: usize = 22;
    let used = 2 + 5 + 2 + name_w + 2; // "  " + badge(5) + "  " + name + "  "
    let brief_w = width.saturating_sub(used);

    let name_col: String = if local.chars().count() <= name_w {
        format!("{:<width$}", local, width = name_w)
    } else {
        let s: String = local.chars().take(name_w.saturating_sub(1)).collect();
        format!("{s}…")
    };
    let brief_col: String = if brief_w == 0 || brief.is_empty() {
        String::new()
    } else if brief.chars().count() <= brief_w {
        brief
    } else {
        let s: String = brief.chars().take(brief_w.saturating_sub(1)).collect();
        format!("{s}…")
    };

    Line::from(vec![
        Span::raw("  "),
        Span::styled(badge, Style::default().fg(badge_color)),
        Span::raw("  "),
        Span::styled(
            name_col,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        ),
        Span::raw("  "),
        Span::styled(brief_col, Style::default().fg(Color::DarkGray)),
    ])
}

/// Overview page for a dependency (mirrors `navIndex` in the web UI).
/// Returns (blocks, content_links) — links let mouse-clicks navigate to detail pages.
fn render_overview_blocks(
    dep: &DocDependency,
    items: &[DocItem],
) -> (Vec<ContentBlock>, Vec<(usize, String)>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut links: Vec<(usize, String)> = Vec::new();

    lines.push(Line::raw(""));
    lines.push(heading_line(
        pulldown_cmark::HeadingLevel::H1,
        &format!("{} {}", dep.name, dep.version),
    ));
    lines.push(Line::raw(""));

    let types: Vec<&DocItem> = items
        .iter()
        .filter(|i| is_type_kind(&i.kind) && !i.name.is_empty())
        .collect();
    let free: Vec<&DocItem> = items
        .iter()
        .filter(|i| !i.name.is_empty() && !is_type_kind(&i.kind) && !i.name.contains("::") && !i.name.contains('.'))
        .collect();
    // Unique namespace prefixes (from namespaced items).
    let mut ns_set: std::collections::BTreeSet<String> = Default::default();
    for item in items.iter().filter(|i| !i.name.is_empty()) {
        if let Some(p) = item.name.rfind("::").or_else(|| item.name.rfind('.')) {
            ns_set.insert(item.name[..p].to_string());
        }
    }

    if !types.is_empty() {
        lines.push(section_kind_line("Classes & Types"));
        lines.push(Line::raw(""));
        for item in &types {
            let vl = lines.len();
            links.push((vl, item.name.clone()));
            lines.push(item_row_line(item, 76));
        }
        lines.push(Line::raw(""));
    }

    if !ns_set.is_empty() {
        lines.push(section_kind_line("Namespaces"));
        lines.push(Line::raw(""));
        for ns in &ns_set {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("[ns]  ", Style::default().fg(Color::Yellow)),
                Span::raw(ns.clone()),
            ]));
        }
        lines.push(Line::raw(""));
    }

    if !free.is_empty() {
        lines.push(section_kind_line("Free Symbols"));
        lines.push(Line::raw(""));
        for item in &free {
            let vl = lines.len();
            links.push((vl, item.name.clone()));
            lines.push(item_row_line(item, 76));
        }
        lines.push(Line::raw(""));
    }

    if types.is_empty() && ns_set.is_empty() && free.is_empty() {
        lines.push(Line::styled(
            "  No documented symbols found.",
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "  Open README for more information.",
            Style::default().fg(Color::DarkGray),
        ));
    }

    (vec![ContentBlock::Lines(lines)], links)
}

/// Type detail page (mirrors `navClass` / `navSymbol` in the web UI).
/// Shows the type's own docs then a summary table of its members.
fn render_type_page_blocks(
    type_item: &DocItem,
    _type_item_global: usize,
    all_items: &[DocItem],
    _item_offset: usize,
    ctx: &RenderCtx,
) -> (Vec<ContentBlock>, Vec<(usize, String)>) {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut links: Vec<(usize, String)> = Vec::new();

    // Header.
    let (badge, badge_color) = kind_badge_info(&type_item.kind);
    blocks.push(ContentBlock::Lines(vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                format!("{}  ", badge),
                Style::default().fg(badge_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                type_item.name.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::styled("─".repeat(60), Style::default().fg(Color::DarkGray)),
        Line::raw(""),
    ]));

    // Full doc for the type itself.
    let md = docify::render_tui::items_to_markdown(std::slice::from_ref(type_item));
    let (item_blocks, item_links) = markdown_to_blocks(&md, 76, ctx);
    let base_vl: usize = blocks.iter().map(|b| b.line_count()).sum();
    for (vl, tgt) in item_links {
        links.push((base_vl + vl, tgt));
    }
    blocks.extend(item_blocks);

    // Member tables: search for items with prefix "TypeName::" or "TypeName.".
    let prefix_cc = format!("{}::", type_item.name);
    let prefix_dot = format!("{}.", type_item.name);

    let mut fns: Vec<&DocItem> = Vec::new();
    let mut vars: Vec<&DocItem> = Vec::new();

    for item in all_items.iter().filter(|i| {
        !i.name.is_empty()
            && (i.name.starts_with(&prefix_cc) || i.name.starts_with(&prefix_dot))
    }) {
        if matches!(item.kind, DocKind::Function | DocKind::Subroutine | DocKind::Macro) {
            fns.push(item);
        } else {
            vars.push(item);
        }
    }

    for (label, members) in [("Member Functions", &fns), ("Fields & Constants", &vars)] {
        if members.is_empty() {
            continue;
        }
        blocks.push(ContentBlock::Lines(vec![
            Line::raw(""),
            section_kind_line(label),
            Line::raw(""),
        ]));
        for member in members.iter() {
            let row_vl: usize = blocks.iter().map(|b| b.line_count()).sum();
            links.push((row_vl, member.name.clone()));
            blocks.push(ContentBlock::Lines(vec![item_row_line(member, 76)]));
        }
    }

    blocks.push(ContentBlock::Lines(vec![Line::raw("")]));
    (blocks, links)
}

/// Namespace detail page (mirrors `navNamespace` in the web UI).
/// Shows all items whose name has `ns` as a prefix.
fn render_ns_page_blocks(
    ns: &str,
    all_items: &[DocItem],
    _item_offset: usize,
    _ctx: &RenderCtx,
) -> (Vec<ContentBlock>, Vec<(usize, String)>) {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let mut links: Vec<(usize, String)> = Vec::new();

    blocks.push(ContentBlock::Lines(vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("[mod]  ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(
                ns.to_string(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::styled("─".repeat(60), Style::default().fg(Color::DarkGray)),
        Line::raw(""),
    ]));

    let prefix_cc = format!("{ns}::");
    let prefix_dot = format!("{ns}.");

    let mut types: Vec<&DocItem> = Vec::new();
    let mut fns: Vec<&DocItem> = Vec::new();
    let mut vars: Vec<&DocItem> = Vec::new();

    for item in all_items.iter().filter(|i| {
        !i.name.is_empty()
            && (i.name.starts_with(&prefix_cc) || i.name.starts_with(&prefix_dot))
    }) {
        if is_type_kind(&item.kind) {
            types.push(item);
        } else if matches!(item.kind, DocKind::Function | DocKind::Subroutine | DocKind::Macro) {
            fns.push(item);
        } else {
            vars.push(item);
        }
    }

    for (label, members) in [
        ("Types", &types),
        ("Functions", &fns),
        ("Variables & Constants", &vars),
    ] {
        if members.is_empty() {
            continue;
        }
        blocks.push(ContentBlock::Lines(vec![
            section_kind_line(label),
            Line::raw(""),
        ]));
        for member in members.iter() {
            let row_vl: usize = blocks.iter().map(|b| b.line_count()).sum();
            links.push((row_vl, member.name.clone()));
            blocks.push(ContentBlock::Lines(vec![item_row_line(member, 76)]));
        }
        blocks.push(ContentBlock::Lines(vec![Line::raw("")]));
    }

    if types.is_empty() && fns.is_empty() && vars.is_empty() {
        blocks.push(ContentBlock::Lines(vec![Line::styled(
            format!("  No items found in '{ns}'."),
            Style::default().fg(Color::DarkGray),
        )]));
    }

    (blocks, links)
}

/// Render package metadata from the dep's freight.toml as styled lines.
fn render_pkg_meta(dep: &DocDependency) -> Vec<Line<'static>> {
    let key_sty = Style::default().fg(Color::DarkGray);
    let link_sty = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED);
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();

    macro_rules! kv {
        ($k:literal, $v:expr) => {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<11} ", $k), key_sty),
                Span::raw($v.to_string()),
            ]));
        };
        ($k:literal, link $v:expr) => {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<11} ", $k), key_sty),
                Span::styled($v.to_string(), link_sty),
            ]));
        };
    }

    kv!("name", &dep.name);
    kv!("version", &dep.version);
    kv!("kind", &dep.kind);
    kv!("source", &dep.source);

    if let Some(root) = &dep.path {
        if let Ok(manifest) = load_manifest(root) {
            let pkg = &manifest.package;

            if !pkg.license.is_empty() {
                kv!("license", &pkg.license);
            }

            if !pkg.description.is_empty() {
                lines.push(Line::raw(""));
                for word_line in word_wrap(&pkg.description, 22) {
                    lines.push(Line::raw(word_line));
                }
            }

            if !pkg.authors.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled("authors", bold)));
                for a in &pkg.authors {
                    lines.push(Line::from(format!("  {a}")));
                }
            }

            if let Some(repo) = &pkg.repository {
                lines.push(Line::raw(""));
                kv!("repository", link repo);
            }

            if !pkg.keywords.is_empty() {
                lines.push(Line::raw(""));
                kv!("keywords", pkg.keywords.join(", "));
            }

            if !pkg.provides.is_empty() {
                kv!("provides", pkg.provides.join(", "));
            }

            let dep_count = manifest.dependencies.len();
            if dep_count > 0 {
                lines.push(Line::raw(""));
                kv!("deps", format!("{dep_count}"));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled("(no manifest)", key_sty)));
    }
    lines
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line = word.to_owned();
        } else if line.len() + 1 + word.len() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_owned();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn math_inline(text: &str) -> String {
    docify::util::latex::render_math_block(text)
}

// ── Markdown → TUI blocks ─────────────────────────────────────────────────────

/// Emit a display-math region — rasterised image when `rich-math` is available,
/// otherwise a Unicode single-line fallback.
fn emit_display_math(builder: &mut DocBlockBuilder, latex: &str, _ctx: &RenderCtx) {
    #[cfg(feature = "rich-math")]
    if let Some(picker) = _ctx.picker.as_ref() {
        if let Some(img_block) = latex_display_to_block(latex, picker) {
            if let ContentBlock::MathImage {
                state,
                height_lines,
            } = img_block
            {
                builder.push_image(state, height_lines);
                builder.push(Line::raw(""));
                return;
            }
        }
    }
    let rendered = docify::util::latex::render_math_block(latex);
    builder.push(Line::from(vec![
        Span::raw("    ".to_owned()),
        Span::raw(rendered),
    ]));
    builder.push(Line::raw(""));
}

/// Render a fenced or indented code block as a rounded-corner bordered box.
fn render_code_block(lang: &str, code: &str, width: usize) -> Vec<Line<'static>> {
    let bdr = Style::default().fg(Color::DarkGray);
    let code_sty = Style::default().fg(Color::LightGreen);
    let inner = width.saturating_sub(4);
    let mut out = Vec::new();

    let top = if lang.is_empty() {
        format!("  ╭{}", "─".repeat(inner + 2))
    } else {
        let label = format!(" {lang} ");
        let llen = label.chars().count();
        let bars = (inner + 2).saturating_sub(llen + 1);
        format!("  ╭─{label}{}", "─".repeat(bars))
    };
    out.push(Line::from(Span::styled(top, bdr)));

    for src in code.trim_end_matches('\n').lines() {
        let content: String = src.chars().take(inner).collect();
        out.push(Line::from(vec![
            Span::styled("  │ ".to_owned(), bdr),
            Span::styled(content, code_sty),
        ]));
    }
    out.push(Line::from(Span::styled(
        format!("  ╰{}", "─".repeat(inner + 2)),
        bdr,
    )));
    out
}

/// Render a markdown table with box-drawing borders.
fn render_md_table(header: &[String], rows: &[Vec<String>]) -> Vec<Line<'static>> {
    let bdr = Style::default().fg(Color::DarkGray);
    let hdr_sty = Style::default().add_modifier(Modifier::BOLD);
    let code_sty = Style::default().fg(Color::LightGreen);

    let ncols = header
        .len()
        .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    if ncols == 0 {
        return vec![];
    }

    let col_widths: Vec<usize> = (0..ncols)
        .map(|c| {
            let hw = header.get(c).map_or(0, |s| s.chars().count());
            let rw = rows
                .iter()
                .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                .max()
                .unwrap_or(0);
            hw.max(rw).max(3)
        })
        .collect();

    let make_rule = |l: &str, m: &str, r: &str| -> Line<'static> {
        let mut s = format!("  {l}");
        for (i, &w) in col_widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if i + 1 < ncols { m } else { r });
        }
        Line::from(Span::styled(s, bdr))
    };

    let make_row = |cells: &[String], cell_sty: Style| -> Line<'static> {
        let mut spans = vec![Span::styled("  │".to_owned(), bdr)];
        for (c, &w) in col_widths.iter().enumerate() {
            let text = cells.get(c).map_or("", String::as_str);
            let (content, sty) = if text.starts_with('`') && text.ends_with('`') && text.len() > 2 {
                (&text[1..text.len() - 1], code_sty)
            } else {
                (text, cell_sty)
            };
            let padded: String = format!(" {content:<w$} ", w = w)
                .chars()
                .take(w + 2)
                .collect();
            spans.push(Span::styled(padded, sty));
            spans.push(Span::styled("│".to_owned(), bdr));
        }
        Line::from(spans)
    };

    let is_separator = |row: &[String]| {
        !row.is_empty()
            && row
                .iter()
                .all(|s| s.trim().chars().all(|c| c == '─' || c == '-' || c == ' '))
    };

    let mut out = vec![make_rule("┌", "┬", "┐")];
    out.push(make_row(header, hdr_sty));
    out.push(make_rule("├", "┼", "┤"));
    for row in rows {
        if is_separator(row) {
            out.push(make_rule("├", "┼", "┤"));
        } else {
            out.push(make_row(row, Style::default()));
        }
    }
    out.push(make_rule("└", "┴", "┘"));
    out
}

/// First-line and continuation-line indent prefixes for the current list/blockquote context.
fn list_item_prefixes(stack: &[(bool, u64)], bq: usize, item_first: bool) -> (String, String) {
    let mut first = String::new();
    let mut cont = String::new();
    for _ in 0..bq {
        first.push_str("▌ ");
        cont.push_str("▌ ");
    }
    let depth = stack.len();
    for (d, (ordered, num)) in stack.iter().enumerate() {
        if d + 1 < depth {
            first.push_str("  ");
            cont.push_str("  ");
        } else if item_first {
            if *ordered {
                let b = format!("{num}. ");
                let p = " ".repeat(b.len());
                first.push_str(&b);
                cont.push_str(&p);
            } else {
                first.push_str("• ");
                cont.push_str("  ");
            }
        } else {
            let pad = " ".repeat(if *ordered {
                format!("{num}. ").len()
            } else {
                2
            });
            first.push_str(&pad);
            cont.push_str(&pad);
        }
    }
    (first, cont)
}

/// Render a heading as a highlighted strip from column 0 with a rounded right end (◗).
///
/// H2 headings (function/type declarations) are syntax-coloured: return type in
/// LightBlue, the function name in Yellow+Bold, parameter list in lighter tones.
/// H3+ use a single colour.
fn heading_line(level: pulldown_cmark::HeadingLevel, text: &str) -> Line<'static> {
    use pulldown_cmark::HeadingLevel as HL;
    let bg = Color::DarkGray;
    let indent = match level {
        HL::H1 => 0,
        HL::H2 => 1,
        HL::H3 => 2,
        HL::H4 => 3,
        HL::H5 => 4,
        HL::H6 => 5,
    };
    let pad = " ".repeat(indent * 2);
    let end_sty = Style::default().fg(bg);

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(format!("{pad} "), Style::default().bg(bg)));

    match level {
        HL::H2 => colorize_sig_spans(text, bg, &mut spans),
        _ => {
            let fg = match level {
                HL::H1 => Color::LightCyan,
                HL::H3 => Color::Magenta,
                _ => Color::White,
            };
            spans.push(Span::styled(
                format!("{text} "),
                Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
            ));
        }
    }
    spans.push(Span::styled("◗", end_sty));
    Line::from(spans)
}

/// Break a C/C++/Fortran function signature into colour-coded spans on a DarkGray bg.
///
/// Heuristic: everything before the first `(` is "return-type + name"; the last
/// whitespace-delimited token before `(` is the function name (Yellow+Bold); the
/// rest is the return type (LightBlue).  Inside the parameter list each word that
/// looks like a C type keyword is styled LightCyan; everything else is White.
fn colorize_sig_spans(sig: &str, bg: Color, out: &mut Vec<Span<'static>>) {
    let bg_plain = Style::default().bg(bg).fg(Color::White);
    let bg_ret = Style::default().bg(bg).fg(Color::LightBlue);
    let bg_name = Style::default()
        .bg(bg)
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let bg_type = Style::default().bg(bg).fg(Color::LightCyan);
    let bg_punct = Style::default().bg(bg).fg(Color::DarkGray);

    // Find the opening paren of the parameter list.
    let paren = sig.find('(');
    if paren.is_none() {
        // Not a function — struct/class/typedef/variable: highlight kind keyword.
        let t = sig.trim();
        let kind_end = t.find(' ').unwrap_or(t.len());
        let (kw, rest) = t.split_at(kind_end);
        let kw_sty = match kw {
            "struct" | "class" | "enum" => Style::default()
                .bg(bg)
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            "typedef" | "using" => Style::default()
                .bg(bg)
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
            "const" | "static" => Style::default()
                .bg(bg)
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
            _ => bg_name,
        };
        out.push(Span::styled(format!("{kw}"), kw_sty));
        out.push(Span::styled(format!("{rest} "), bg_plain));
        return;
    }
    let paren = paren.unwrap();
    let before = sig[..paren].trim_end();
    let params = &sig[paren..]; // "(int a, int b)"

    // Split return-type from function name.
    // The last token before `(` is the name (may have * prefix for pointer-returning fns).
    let last_space = before.rfind(|c: char| c.is_ascii_whitespace() || c == '*');
    let (ret_part, name_part) = if let Some(p) = last_space {
        let split_at = if before.as_bytes().get(p) == Some(&b'*') {
            p
        } else {
            p + 1
        };
        (&before[..split_at], &before[split_at..])
    } else {
        ("", before)
    };

    if !ret_part.is_empty() {
        out.push(Span::styled(format!("{} ", ret_part.trim_end()), bg_ret));
    }
    let name_clean = name_part.trim_start_matches('*');
    let ptr_stars = &name_part[..name_part.len() - name_clean.len()];
    if !ptr_stars.is_empty() {
        out.push(Span::styled(ptr_stars.to_owned(), bg_punct));
    }
    out.push(Span::styled(name_clean.to_owned(), bg_name));

    // Colour-tokenize the parameter list.
    colorize_param_list(params, bg, bg_type, bg_plain, bg_punct, out);
    out.push(Span::styled(" ", bg_plain));
}

/// Tokenize `(int a, double *b, ...)` and push styled spans.
fn colorize_param_list(
    params: &str,
    bg: Color,
    type_sty: Style,
    plain_sty: Style,
    punct_sty: Style,
    out: &mut Vec<Span<'static>>,
) {
    // Simple tokenizer: split on whitespace and punctuation, preserving them.
    let mut tok = String::new();
    let mut chars = params.chars().peekable();

    // Track whether we just emitted a type token — the next identifier is a name.
    let mut last_was_type = false;

    let flush = |tok: &mut String, last_was_type: &mut bool, out: &mut Vec<Span<'static>>| {
        if tok.is_empty() {
            return;
        }
        let s = std::mem::take(tok);
        let sty = if is_c_type_keyword(&s) {
            *last_was_type = true;
            type_sty
        } else if *last_was_type {
            *last_was_type = false;
            plain_sty.fg(Color::White)
        } else {
            plain_sty
        };
        out.push(Span::styled(s, sty));
    };

    while let Some(c) = chars.next() {
        if c.is_alphanumeric() || c == '_' {
            tok.push(c);
        } else {
            flush(&mut tok, &mut last_was_type, out);
            // Punctuation/space
            let sty = match c {
                '(' | ')' | ',' | ';' => punct_sty,
                '*' | '&' => Style::default().bg(bg).fg(Color::LightMagenta),
                ' ' => {
                    out.push(Span::styled(" ", plain_sty));
                    continue;
                }
                _ => plain_sty,
            };
            out.push(Span::styled(c.to_string(), sty));
        }
    }
    flush(&mut tok, &mut last_was_type, out);
}

fn is_c_type_keyword(s: &str) -> bool {
    matches!(
        s,
        "int"
            | "long"
            | "short"
            | "char"
            | "void"
            | "float"
            | "double"
            | "bool"
            | "unsigned"
            | "signed"
            | "const"
            | "volatile"
            | "restrict"
            | "static"
            | "inline"
            | "extern"
            | "register"
            | "auto"
            | "struct"
            | "class"
            | "enum"
            | "union"
            | "typename"
            | "template"
            | "size_t"
            | "ssize_t"
            | "uint8_t"
            | "uint16_t"
            | "uint32_t"
            | "uint64_t"
            | "uintptr_t"
            | "int8_t"
            | "int16_t"
            | "int32_t"
            | "int64_t"
            | "intptr_t"
            | "ptrdiff_t"
            | "nullptr_t"
            | "string"
            | "vector"
            | "map"
            | "set"
    )
}

// ── Block builder ─────────────────────────────────────────────────────────────

struct DocBlockBuilder {
    blocks: Vec<ContentBlock>,
    pending: Vec<Line<'static>>,
}

impl DocBlockBuilder {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn push(&mut self, line: Line<'static>) {
        self.pending.push(line);
    }

    fn virtual_line(&self) -> usize {
        self.blocks.iter().map(|b| b.line_count()).sum::<usize>() + self.pending.len()
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.blocks
                .push(ContentBlock::Lines(std::mem::take(&mut self.pending)));
        }
    }

    #[cfg(feature = "rich-math")]
    fn push_image(&mut self, state: ratatui_image::protocol::StatefulProtocol, height_lines: u16) {
        self.flush();
        self.blocks.push(ContentBlock::MathImage {
            state,
            height_lines,
        });
    }

    fn finish(mut self) -> Vec<ContentBlock> {
        self.flush();
        self.blocks
    }
}

/// Render a markdown string to TUI content blocks, also returning clickable link positions.
///
/// Links are emitted as `(virtual_line, target_name)` pairs. Targets come from the anchor
/// portion of the URL: `[foo](#foo)` → `("foo", virtual_line)`. Clicking on such a line
/// in the content panel navigates to the named symbol.
fn markdown_to_blocks(
    text: &str,
    width: usize,
    ctx: &RenderCtx,
) -> (Vec<ContentBlock>, Vec<(usize, String)>) {
    use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_MATH
        | Options::ENABLE_TASKLISTS;

    let mut builder = DocBlockBuilder::new();
    let mut links: Vec<(usize, String)> = Vec::new();

    // Each word carries (text, style, optional_link_dest).
    let mut words: Vec<(String, Style, Option<String>)> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let mut current_link: Option<String> = None;
    let code_sty = Style::default().fg(Color::LightGreen);
    let link_sty = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED);

    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_text = String::new();

    let mut hd_level: Option<HeadingLevel> = None;
    let mut hd_text: String = String::new();

    let mut tbl_header: Vec<String> = Vec::new();
    let mut tbl_rows: Vec<Vec<String>> = Vec::new();
    let mut tbl_row: Vec<String> = Vec::new();
    let mut tbl_cell: String = String::new();
    let mut in_thead = false;
    let mut in_cell = false;

    let mut list_stack: Vec<(bool, u64)> = Vec::new();
    let mut item_first = false;

    let mut bq: usize = 0;

    // Drain `words` into word-wrapped Lines.  Records a link entry for any
    // wrapped line that contains at least one word with a link destination.
    macro_rules! flush_words {
        ($fp:expr, $cp:expr) => {{
            if !words.is_empty() {
                let fp: String = $fp;
                let cp: String = $cp;
                let fa = width.saturating_sub(fp.chars().count().min(width));
                let ca = width.saturating_sub(cp.chars().count().min(width));
                let mut cur: Vec<Span<'static>> = vec![Span::raw(fp)];
                let mut cur_link: Option<String> = None;
                let mut len: usize = 0;
                let mut fst = true;
                for (word, sty, ldest) in words.drain(..) {
                    if let Some(ref d) = ldest {
                        cur_link = Some(d.clone());
                    }
                    let wl = word.chars().count();
                    let av = if fst { fa } else { ca };
                    if len == 0 {
                        cur.push(Span::styled(word, sty));
                        len = wl;
                    } else if len + 1 + wl <= av {
                        cur.push(Span::styled(format!(" {word}"), sty));
                        len += 1 + wl;
                    } else {
                        if let Some(ref d) = cur_link.take() {
                            links.push((builder.virtual_line(), d.clone()));
                        }
                        builder.push(Line::from(std::mem::take(&mut cur)));
                        fst = false;
                        cur = vec![Span::raw(cp.clone()), Span::styled(word, sty)];
                        len = wl;
                    }
                }
                if len > 0 {
                    if let Some(ref d) = cur_link {
                        links.push((builder.virtual_line(), d.clone()));
                    }
                    builder.push(Line::from(cur));
                }
                builder.push(Line::raw(""));
            }
        }};
    }

    macro_rules! cur_fp_cp {
        () => {
            if !list_stack.is_empty() {
                list_item_prefixes(&list_stack, bq, item_first)
            } else {
                let p = "▌ ".repeat(bq);
                (p.clone(), p)
            }
        };
    }

    macro_rules! isty {
        () => {{
            let mut s = Style::default();
            if bold {
                s = s.add_modifier(Modifier::BOLD);
            }
            if italic {
                s = s.add_modifier(Modifier::ITALIC);
            }
            if strike {
                s = s.add_modifier(Modifier::CROSSED_OUT);
            }
            s
        }};
    }

    for event in Parser::new_ext(text, opts) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                hd_level = Some(level);
                hd_text.clear();
            }
            Event::End(TagEnd::Heading(level)) => {
                builder.push(heading_line(level, &hd_text));
                builder.push(Line::raw(""));
                hd_level = None;
                hd_text.clear();
            }

            // Cross-reference links: [name](#name) or [name](name)
            Event::Start(Tag::Link { dest_url, .. }) => {
                let dest = dest_url.trim_start_matches('#').to_string();
                if !dest.is_empty() {
                    current_link = Some(dest);
                }
            }
            Event::End(TagEnd::Link) => {
                current_link = None;
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                let (fp, cp) = cur_fp_cp!();
                flush_words!(fp, cp);
                item_first = false;
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code_text.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                for l in render_code_block(&code_lang, &code_text, width) {
                    builder.push(l);
                }
                builder.push(Line::raw(""));
                in_code = false;
                code_lang.clear();
                code_text.clear();
            }

            Event::Start(Tag::BlockQuote(_)) => {
                bq += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                bq = bq.saturating_sub(1);
            }

            Event::Start(Tag::List(n)) => {
                list_stack.push((n.is_some(), n.map(|v| v.saturating_sub(1)).unwrap_or(0)));
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                if list_stack.is_empty() {
                    builder.push(Line::raw(""));
                }
            }
            Event::Start(Tag::Item) => {
                if let Some(last) = list_stack.last_mut() {
                    if last.0 {
                        last.1 += 1;
                    }
                }
                item_first = true;
            }
            Event::End(TagEnd::Item) => {
                if !words.is_empty() {
                    let (fp, cp) = list_item_prefixes(&list_stack, bq, item_first);
                    flush_words!(fp, cp);
                }
                item_first = false;
            }

            Event::Start(Tag::Table(_)) => {
                tbl_header.clear();
                tbl_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                for l in render_md_table(&tbl_header, &tbl_rows) {
                    builder.push(l);
                }
                builder.push(Line::raw(""));
            }
            Event::Start(Tag::TableHead) => {
                in_thead = true;
                tbl_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                tbl_header = std::mem::take(&mut tbl_row);
                in_thead = false;
            }
            Event::Start(Tag::TableRow) => {
                tbl_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !in_thead {
                    tbl_rows.push(std::mem::take(&mut tbl_row));
                }
            }
            Event::Start(Tag::TableCell) => {
                tbl_cell.clear();
                in_cell = true;
            }
            Event::End(TagEnd::TableCell) => {
                tbl_row.push(std::mem::take(&mut tbl_cell));
                in_cell = false;
            }

            Event::Rule => {
                builder.push(Line::styled(
                    "─".repeat(width),
                    Style::default().fg(Color::DarkGray),
                ));
                builder.push(Line::raw(""));
            }

            Event::Start(Tag::Strong) => {
                bold = true;
            }
            Event::End(TagEnd::Strong) => {
                bold = false;
            }
            Event::Start(Tag::Emphasis) => {
                italic = true;
            }
            Event::End(TagEnd::Emphasis) => {
                italic = false;
            }
            Event::Start(Tag::Strikethrough) => {
                strike = true;
            }
            Event::End(TagEnd::Strikethrough) => {
                strike = false;
            }

            Event::Text(t) => {
                if in_code {
                    code_text.push_str(&t);
                } else if hd_level.is_some() {
                    hd_text.push_str(&t);
                } else if in_cell {
                    tbl_cell.push_str(&t);
                } else {
                    let sty = if current_link.is_some() {
                        link_sty
                    } else {
                        isty!()
                    };
                    for word in t.split_whitespace() {
                        words.push((word.to_owned(), sty, current_link.clone()));
                    }
                }
            }

            Event::Code(t) => {
                if hd_level.is_some() {
                    hd_text.push_str(&format!("`{t}`"));
                } else if in_cell {
                    tbl_cell.push_str(&format!("`{t}`"));
                } else {
                    let sty = if current_link.is_some() {
                        link_sty
                    } else {
                        code_sty
                    };
                    words.push((t.into_string(), sty, current_link.clone()));
                }
            }

            Event::SoftBreak => {}
            Event::HardBreak => {
                if !words.is_empty() {
                    let (fp, cp) = cur_fp_cp!();
                    flush_words!(fp, cp);
                    item_first = false;
                }
            }

            Event::InlineMath(tex) => {
                let rendered = math_inline(&tex);
                let sty = isty!();
                if hd_level.is_some() {
                    hd_text.push_str(&rendered);
                } else if in_cell {
                    tbl_cell.push_str(&rendered);
                } else {
                    for word in rendered.split_whitespace() {
                        words.push((word.to_owned(), sty, None));
                    }
                }
            }

            Event::DisplayMath(tex) => {
                let (fp, cp) = cur_fp_cp!();
                flush_words!(fp, cp);
                item_first = false;
                emit_display_math(&mut builder, &tex, ctx);
            }

            _ => {}
        }
    }

    if !words.is_empty() {
        let p = "▌ ".repeat(bq);
        flush_words!(p.clone(), p);
    }

    let blocks = builder.finish();
    if blocks.is_empty() {
        return (
            vec![ContentBlock::Lines(vec![Line::raw("(no content)")])],
            Vec::new(),
        );
    }
    (blocks, links)
}

#[cfg(test)]
mod tree_tests {
    use super::*;
    use docify::extract::{DocItem, DocKind, DocLanguage, DocMeta};
    use std::path::PathBuf;

    fn make_item(name: &str, kind: DocKind) -> DocItem {
        DocItem {
            name: name.to_string(),
            kind,
            brief: "A brief.".to_string(),
            body: String::new(),
            tags: Vec::new(),
            file: PathBuf::from("test.cpp"),
            line: 1,
            lang: DocLanguage::Cpp,
            signature: String::new(),
            meta: DocMeta::default(),
        }
    }

    #[test]
    fn build_api_subtree_namespaced_items_visible() {
        // Class in a namespace → "Classes & Types" section.
        // Functions in a namespace → "Namespaces" section (group node for the ns).
        let items = vec![
            make_item("stats::mean", DocKind::Function),
            make_item("stats::variance", DocKind::Function),
            make_item("stats::OrderStatistics", DocKind::Class),
            make_item("", DocKind::Unknown), // @file block — should be skipped
        ];
        let sub = build_api_subtree(&items, 0, 1, 0);

        let hdrs: Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::SectionHdr)).collect();
        let groups: Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::Group)).collect();
        let syms: Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::Symbol)).collect();

        // "Classes & Types" (OrderStatistics) + "Namespaces" (stats group)
        assert_eq!(hdrs.len(), 2, "Classes & Types and Namespaces section headers");
        assert_eq!(groups.len(), 1, "one namespace group: stats");
        assert_eq!(syms.len(), 1, "one type symbol: OrderStatistics");
        assert_eq!(hdrs[0].label, "Classes & Types");
        assert_eq!(hdrs[1].label, "Namespaces");
        assert_eq!(groups[0].label, "stats");
        assert_eq!(syms[0].label, "OrderStatistics");

        let mut tree = vec![TreeNode {
            label: "dep 0.1".to_string(),
            depth: 0,
            kind: TreeNodeKind::Dep,
            expanded: true,
            item_idx: None,
            dep_idx: Some(0),
            loaded: true,
        }];
        tree.extend(sub);

        let vis = visible_nodes(&tree);
        // dep + "Classes & Types" hdr + OrderStatistics + "Namespaces" hdr + stats = 5
        assert_eq!(vis.len(), 5, "dep, 2 section headers, 1 type symbol, 1 ns group");
    }

    #[test]
    fn build_api_subtree_flat_items_visible() {
        // Top-level functions (no "::") go to "Free Symbols" section.
        let items = vec![
            make_item("clamp", DocKind::Function),
            make_item("lerp", DocKind::Function),
        ];
        let sub = build_api_subtree(&items, 0, 1, 0);

        let hdrs: Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::SectionHdr)).collect();
        let syms: Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::Symbol)).collect();
        assert_eq!(hdrs.len(), 1, "only Free Symbols section header");
        assert_eq!(syms.len(), 2, "clamp + lerp");
        assert_eq!(hdrs[0].label, "Free Symbols");

        let mut tree = vec![TreeNode {
            label: "dep 0.1".to_string(),
            depth: 0,
            kind: TreeNodeKind::Dep,
            expanded: true,
            item_idx: None,
            dep_idx: Some(0),
            loaded: true,
        }];
        tree.extend(sub);
        let vis = visible_nodes(&tree);
        // dep + "Free Symbols" hdr + clamp + lerp = 4
        assert_eq!(vis.len(), 4, "dep, section header, 2 symbols");
    }

    #[test]
    fn collapsed_dep_hides_children() {
        let items = vec![make_item("foo", DocKind::Function)];
        let sub = build_api_subtree(&items, 0, 1, 0);
        let mut tree = vec![TreeNode {
            label: "dep 0.1".to_string(),
            depth: 0,
            kind: TreeNodeKind::Dep,
            expanded: false, // collapsed
            item_idx: None,
            dep_idx: Some(0),
            loaded: true,
        }];
        tree.extend(sub);
        let vis = visible_nodes(&tree);
        assert_eq!(vis.len(), 1, "only dep itself visible when collapsed");
    }
}
