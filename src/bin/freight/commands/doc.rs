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

use freight_core::manifest::types::{Dependency, Manifest};
use freight_core::manifest::{find_manifest_dir, load_manifest};
use freight_core::toolchain::freight_home;
use docify::extract::{extract_dir, DocItem, DocSet};
use docify::render;

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
            let dir = project_dir.join(".deps").join(name);
            (
                "registry".to_string(),
                version.clone(),
                "freight.dev".to_string(),
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if freight_core::manifest::types::is_platform_dep(name) => {
            (
                "platform".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                name.to_string(),
                None,
            )
        }
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
            let dir = project_dir.join(".deps").join(name);
            let source = d.git.clone().unwrap_or_default();
            (
                "git".to_string(),
                git_ref(d),
                source,
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) if d.url.is_some() => {
            let dir = project_dir.join(".deps").join(name);
            let source = d.url.clone().unwrap_or_default();
            (
                "url".to_string(),
                d.version.clone().unwrap_or_else(|| "*".into()),
                source,
                dir.exists().then_some(dir),
            )
        }
        Dependency::Detailed(d) => {
            let dir = project_dir.join(".deps").join(name);
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
enum TreeNodeKind { Dep, Group, Symbol, Readme }

/// One node in the unified package/API tree.
struct TreeNode {
    label:    String,
    depth:    usize,
    kind:     TreeNodeKind,
    expanded: bool,
    item_idx: Option<usize>, // for Symbol nodes: index into doc_items
    dep_idx:  Option<usize>, // for Dep nodes: index into deps
    loaded:   bool,          // for Dep nodes: true once items have been loaded
}

/// Which panel currently holds keyboard focus.
#[derive(Clone, Copy, PartialEq)]
enum Focus { Left, Content, Meta }

struct DocApp<'a> {
    deps: &'a [DocDependency],
    ctx:  RenderCtx,
    focus: Focus,

    // Unified tree (deps as roots, namespaces + symbols as children)
    tree:         Vec<TreeNode>,
    tree_cursor:  usize,
    tree_offset:  usize,
    tree_visible: usize,
    doc_items:    Vec<DocItem>, // flat: all loaded items from all deps (append-only)

    // Per-dep item ranges — populated on first load; index matches deps[].
    dep_item_ranges: Vec<Option<(usize, usize)>>, // (offset, count) into doc_items

    // Which dep's content is currently shown in the centre panel.
    content_dep_idx: Option<usize>,

    // Content (centre)
    blocks:        Vec<ContentBlock>,
    total_lines:   usize,
    scroll:        usize,
    content_links: Vec<(usize, String)>, // virtual_line → link target name
    content_area:  Rect,                 // updated each frame for click hit-testing
    item_vlines:   Vec<usize>,           // sorted virtual line of each item section (for Tab)
    item_line_map: std::collections::HashMap<usize, usize>, // item_idx → first virtual line

    // Metadata (right panel)
    meta_lines:   Vec<Line<'static>>,
    meta_scroll:  usize,
    meta_visible: usize,
}

impl<'a> DocApp<'a> {
    fn new(deps: &'a [DocDependency], ctx: RenderCtx) -> Self {
        let tree = deps.iter().enumerate().map(|(i, dep)| TreeNode {
            label:    format!("{} {}", dep.name, dep.version),
            depth:    0,
            kind:     TreeNodeKind::Dep,
            expanded: false,
            item_idx: None,
            dep_idx:  Some(i),
            loaded:   false,
        }).collect();
        let n = deps.len();
        Self {
            deps, ctx,
            focus: Focus::Left,
            tree, tree_cursor: 0, tree_offset: 0, tree_visible: 0,
            doc_items: Vec::new(),
            dep_item_ranges: vec![None; n],
            content_dep_idx: None,
            blocks: Vec::new(), total_lines: 0, scroll: 0,
            content_links: Vec::new(), content_area: Rect::default(),
            item_vlines: Vec::new(), item_line_map: std::collections::HashMap::new(),
            meta_lines: Vec::new(), meta_scroll: 0, meta_visible: 0,
        }
    }

    /// Toggle a Dep node: extract items + build content on first open; just toggle on re-open.
    fn open_dep_node(&mut self, tree_idx: usize) {
        let dep_idx = self.tree[tree_idx].dep_idx.unwrap();

        self.meta_lines  = render_pkg_meta(&self.deps[dep_idx]);
        self.meta_scroll = 0;

        if !self.tree[tree_idx].loaded {
            // Extract items and record their range in doc_items.
            let item_offset = self.doc_items.len();
            let new_items   = extract_dep_items(&self.deps[dep_idx]);
            let item_count  = new_items.len();
            self.doc_items.extend(new_items);
            self.dep_item_ranges[dep_idx] = Some((item_offset, item_count));

            // Build API sub-tree.
            let sub = build_api_subtree(&self.doc_items[item_offset..item_offset + item_count], item_offset, 1);

            // Insert README node (depth 1) then API nodes.
            let has_readme = readme_exists(&self.deps[dep_idx]);
            let mut ins = tree_idx + 1;
            if has_readme {
                self.tree.insert(ins, TreeNode {
                    label:    "README".to_string(),
                    depth:    1,
                    kind:     TreeNodeKind::Readme,
                    expanded: false,
                    item_idx: None,
                    dep_idx:  Some(dep_idx),
                    loaded:   false,
                });
                ins += 1;
            }
            for (i, node) in sub.into_iter().enumerate() {
                self.tree.insert(ins + i, node);
            }
            self.tree[tree_idx].loaded = true;
        }

        // Build (or re-build if switching deps) the centre-panel content.
        self.render_dep_content(dep_idx);
        self.scroll = 0;
        self.tree[tree_idx].expanded = !self.tree[tree_idx].expanded;
        self.focus = Focus::Content;
    }

    /// Populate `blocks` / `links` / `vlines` / `line_map` for `dep_idx`.
    /// Always prepends the README (if present) before the API items.
    fn render_dep_content(&mut self, dep_idx: usize) {
        self.blocks.clear();
        self.content_links.clear();
        self.item_vlines.clear();
        self.item_line_map.clear();

        let dep = &self.deps[dep_idx];

        // 1. README section (if it exists).
        if readme_exists(dep) {
            let (rb, rl) = load_readme_content(dep, &self.ctx);
            self.blocks.extend(rb);
            self.content_links.extend(rl);
            // Separator before API items.
            self.blocks.push(ContentBlock::Lines(vec![
                Line::raw(""),
                Line::styled("─".repeat(78), Style::default().fg(Color::DarkGray)),
                Line::raw(""),
            ]));
        }

        // 2. API items (if any).
        if let Some((offset, count)) = self.dep_item_ranges[dep_idx] {
            if count > 0 {
                let items = &self.doc_items[offset..offset + count];
                load_all_items(items, offset, &self.ctx,
                    &mut self.blocks, &mut self.content_links,
                    &mut self.item_vlines, &mut self.item_line_map);
            }
        }

        self.total_lines    = self.blocks.iter().map(ContentBlock::line_count).sum();
        self.content_dep_idx = Some(dep_idx);
    }

    fn open_tree_item(&mut self) {
        let vis = visible_nodes(&self.tree);
        let Some(&idx) = vis.get(self.tree_cursor) else { return; };

        match self.tree[idx].kind {
            TreeNodeKind::Dep => {
                self.open_dep_node(idx);
                let new_len = visible_nodes(&self.tree).len();
                self.tree_cursor = self.tree_cursor.min(new_len.saturating_sub(1));
            }
            TreeNodeKind::Group => {
                self.tree[idx].expanded = !self.tree[idx].expanded;
                let new_len = visible_nodes(&self.tree).len();
                self.tree_cursor = self.tree_cursor.min(new_len.saturating_sub(1));
            }
            TreeNodeKind::Symbol => {
                if let Some(item_idx) = self.tree[idx].item_idx {
                    self.jump_to_item(item_idx);
                }
            }
            TreeNodeKind::Readme => {
                let dep_idx = self.tree[idx].dep_idx.unwrap();
                // If this dep's content isn't showing, load it.
                if self.content_dep_idx != Some(dep_idx) {
                    // Ensure items are extracted first (they may not be if the dep
                    // node was expanded via a different path).
                    if self.dep_item_ranges[dep_idx].is_none() {
                        let offset = self.doc_items.len();
                        let items  = extract_dep_items(&self.deps[dep_idx]);
                        let count  = items.len();
                        self.doc_items.extend(items);
                        self.dep_item_ranges[dep_idx] = Some((offset, count));
                    }
                    self.render_dep_content(dep_idx);
                }
                self.scroll = 0;
                self.focus  = Focus::Content;
            }
        }
    }

    fn jump_to_item(&mut self, item_idx: usize) {
        if let Some(&vline) = self.item_line_map.get(&item_idx) {
            self.scroll = vline;
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
            // Highlight the tree node
            let vis = visible_nodes(&self.tree);
            if let Some(cursor) = vis.iter().position(|&ti| self.tree[ti].item_idx == Some(item_idx)) {
                self.tree_cursor = cursor;
            } else {
                if let Some(ti) = self.tree.iter().position(|n| n.item_idx == Some(item_idx)) {
                    let depth = self.tree[ti].depth;
                    if depth > 0 {
                        for pi in (0..ti).rev() {
                            if self.tree[pi].depth < depth
                                && matches!(self.tree[pi].kind, TreeNodeKind::Group | TreeNodeKind::Dep)
                            {
                                self.tree[pi].expanded = true;
                                break;
                            }
                        }
                    }
                    let vis2 = visible_nodes(&self.tree);
                    if let Some(cursor) = vis2.iter().position(|&x| x == ti) {
                        self.tree_cursor = cursor;
                    }
                }
            }
            self.jump_to_item(item_idx);
        }
    }

    /// Advance scroll to the next function section (Tab in content panel).
    fn next_declaration(&mut self) {
        if self.item_vlines.is_empty() { return; }
        let next = self.item_vlines.iter()
            .find(|&&vl| vl > self.scroll)
            .copied()
            .unwrap_or(self.item_vlines[0]); // wrap around
        self.scroll = next;
    }

    /// Scroll back to the previous function section (Shift-Tab in content panel).
    fn prev_declaration(&mut self) {
        if self.item_vlines.is_empty() { return; }
        let prev = self.item_vlines.iter().rev()
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
                if handle_key(&mut app, key) { break; }
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
        KeyEvent { code: KeyCode::Char('q'), .. }
        | KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL, .. } => return true,

        // Left arrow → tree panel; Right arrow → content panel.
        KeyEvent { code: KeyCode::Left, .. } => { app.focus = Focus::Left; }
        KeyEvent { code: KeyCode::Right, .. } => { app.focus = Focus::Content; }

        // Tab: Left→Content (focus switch); Content→next declaration; Meta→Left.
        KeyEvent { code: KeyCode::Tab, modifiers: KeyModifiers::NONE, .. } => {
            match app.focus {
                Focus::Left    => { app.focus = Focus::Content; }
                Focus::Content => { app.next_declaration(); }
                Focus::Meta    => { app.focus = Focus::Left; }
            }
        }
        KeyEvent { code: KeyCode::BackTab, .. } => {
            match app.focus {
                Focus::Content => { app.prev_declaration(); }
                _              => { app.focus = Focus::Left; }
            }
        }

        KeyEvent { code: KeyCode::Esc, .. } | KeyEvent { code: KeyCode::Backspace, .. } => {
            app.focus = Focus::Left;
        }

        _ => match app.focus {
            Focus::Left    => handle_left_key(app, key),
            Focus::Content => handle_content_key(app, key),
            Focus::Meta    => handle_meta_key(app, key),
        },
    }
    false
}

fn handle_left_key(app: &mut DocApp<'_>, key: KeyEvent) {
    match key {
        KeyEvent { code: KeyCode::Up, .. } | KeyEvent { code: KeyCode::Char('k'), .. } => {
            app.tree_cursor = app.tree_cursor.saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Down, .. } | KeyEvent { code: KeyCode::Char('j'), .. } => {
            let vis_len = visible_nodes(&app.tree).len();
            app.tree_cursor = (app.tree_cursor + 1).min(vis_len.saturating_sub(1));
        }
        KeyEvent { code: KeyCode::PageUp, .. } => {
            let v = app.tree_visible.max(1);
            app.tree_cursor = app.tree_cursor.saturating_sub(v);
        }
        KeyEvent { code: KeyCode::PageDown, .. } => {
            let v = app.tree_visible.max(1);
            let n = visible_nodes(&app.tree).len().saturating_sub(1);
            app.tree_cursor = (app.tree_cursor + v).min(n);
        }
        KeyEvent { code: KeyCode::Home, .. } | KeyEvent { code: KeyCode::Char('g'), .. } => {
            app.tree_cursor = 0;
        }
        KeyEvent { code: KeyCode::End, .. } | KeyEvent { code: KeyCode::Char('G'), .. } => {
            app.tree_cursor = visible_nodes(&app.tree).len().saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Enter, .. } => {
            app.open_tree_item();
        }
        _ => {}
    }
}

fn handle_content_key(app: &mut DocApp<'_>, key: KeyEvent) {
    let total = app.total_lines;
    match key {
        KeyEvent { code: KeyCode::Up, .. } | KeyEvent { code: KeyCode::Char('k'), .. } => {
            app.scroll = app.scroll.saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Down, .. } | KeyEvent { code: KeyCode::Char('j'), .. } => {
            app.scroll = (app.scroll + 1).min(total.saturating_sub(1));
        }
        KeyEvent { code: KeyCode::PageUp, .. } => {
            app.scroll = app.scroll.saturating_sub(20);
        }
        KeyEvent { code: KeyCode::PageDown, .. } | KeyEvent { code: KeyCode::Char(' '), .. } => {
            app.scroll = (app.scroll + 20).min(total.saturating_sub(1));
        }
        KeyEvent { code: KeyCode::Home, .. } | KeyEvent { code: KeyCode::Char('g'), .. } => {
            app.scroll = 0;
        }
        KeyEvent { code: KeyCode::End, .. } | KeyEvent { code: KeyCode::Char('G'), .. } => {
            app.scroll = total.saturating_sub(1);
        }
        _ => {}
    }
}

fn handle_meta_key(app: &mut DocApp<'_>, key: KeyEvent) {
    let total = app.meta_lines.len();
    match key {
        KeyEvent { code: KeyCode::Up, .. } | KeyEvent { code: KeyCode::Char('k'), .. } => {
            app.meta_scroll = app.meta_scroll.saturating_sub(1);
        }
        KeyEvent { code: KeyCode::Down, .. } | KeyEvent { code: KeyCode::Char('j'), .. } => {
            app.meta_scroll = (app.meta_scroll + 1).min(total.saturating_sub(1));
        }
        KeyEvent { code: KeyCode::Home, .. } => { app.meta_scroll = 0; }
        KeyEvent { code: KeyCode::End, .. }  => { app.meta_scroll = total.saturating_sub(1); }
        _ => {}
    }
}

fn handle_mouse_click(app: &mut DocApp<'_>, col: u16, row: u16) {
    let a = app.content_area;
    if a.width == 0 { return; }
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
        .constraints([Constraint::Percentage(20), Constraint::Min(10), Constraint::Percentage(20)])
        .split(rows[0]);

    draw_tree(frame, app, cols[0]);
    draw_content(frame, app, cols[1]);
    draw_meta(frame, app, cols[2]);

    draw_help_bar(frame, app, rows[1]);
}

fn draw_help_bar(frame: &mut ratatui::Frame, app: &DocApp<'_>, area: Rect) {
    let text = match app.focus {
        Focus::Left    => "↑/↓  Enter expand/open  →/← focus  q quit",
        Focus::Content => "↑/↓  PgUp/PgDn  Tab next decl  Shift-Tab prev decl  click link  ← tree  q quit",
        Focus::Meta    => "↑/↓  Tab/← focus  q quit",
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(Color::DarkGray)), area);
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
        .border_style(if focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = inner.height as usize;
    app.tree_visible = visible;

    if app.tree_cursor < app.tree_offset {
        app.tree_offset = app.tree_cursor;
    } else if visible > 0 && app.tree_cursor >= app.tree_offset + visible {
        app.tree_offset = app.tree_cursor + 1 - visible;
    }

    let items: Vec<ListItem> = vis.iter().skip(app.tree_offset).take(visible).map(|&ti| {
        let node = &app.tree[ti];
        let pad  = "  ".repeat(node.depth);
        match node.kind {
            TreeNodeKind::Dep => {
                let scope_color = node.dep_idx
                    .and_then(|i| app.deps.get(i))
                    .map(|d| match d.scope {
                        "local"     => Color::Cyan,
                        "local-dev" => Color::Blue,
                        "global"    => Color::DarkGray,
                        _           => Color::Reset,
                    })
                    .unwrap_or(Color::Reset);
                let arrow = if node.expanded { "▾ " } else { "▸ " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{pad}{arrow}"), Style::default().fg(scope_color)),
                    Span::styled(node.label.clone(), Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD)),
                ]))
            }
            TreeNodeKind::Group => {
                let arrow = if node.expanded { "▾ " } else { "▸ " };
                ListItem::new(Line::from(vec![
                    Span::raw(pad),
                    Span::styled(format!("{arrow}{}", node.label), Style::default().fg(Color::Yellow)),
                ]))
            }
            TreeNodeKind::Symbol => {
                let color = node.item_idx
                    .and_then(|ii| app.doc_items.get(ii))
                    .map(|it| match it.kind.label() {
                        "fn" | "sub" | "func" => Color::LightBlue,
                        "struct" | "class"    => Color::LightGreen,
                        "enum"                => Color::LightMagenta,
                        _                     => Color::Reset,
                    })
                    .unwrap_or(Color::Reset);
                ListItem::new(Line::from(vec![
                    Span::raw(pad),
                    Span::styled(format!("  {}", node.label), Style::default().fg(color)),
                ]))
            }
            TreeNodeKind::Readme => {
                ListItem::new(Line::from(vec![
                    Span::raw(pad),
                    Span::styled("  README", Style::default().fg(Color::Cyan)),
                ]))
            }
        }
    }).collect();

    let sel_in_view = app.tree_cursor.saturating_sub(app.tree_offset);
    let mut state = ListState::default().with_offset(0);
    state.select(Some(sel_in_view));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::default())
            .highlight_symbol(""),
        inner, &mut state,
    );
}

fn draw_content(frame: &mut ratatui::Frame, app: &mut DocApp<'_>, area: Rect) {
    let focused = app.focus == Focus::Content;
    let outer = Block::default()
        .title("docs")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused { Style::default() } else { Style::default().fg(Color::DarkGray) });
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

    let scroll    = app.scroll;
    let visible_h = inner.height as usize;
    let mut vy: usize = 0;

    for block in app.blocks.iter_mut() {
        let bh    = block.line_count();
        let bstart = vy;
        let bend   = vy + bh;
        vy = bend;

        if bend <= scroll || bstart >= scroll + visible_h { continue; }

        let screen_top = bstart.saturating_sub(scroll);
        let skip       = scroll.saturating_sub(bstart);

        match block {
            ContentBlock::Lines(lines) => {
                let take = (bh - skip).min(visible_h - screen_top);
                let rect = Rect {
                    x: inner.x, y: inner.y + screen_top as u16,
                    width: inner.width, height: take as u16,
                };
                frame.render_widget(Paragraph::new(lines[skip..skip + take].to_vec()), rect);
            }
            #[cfg(feature = "rich-math")]
            ContentBlock::MathImage { state, height_lines } => {
                let rect = Rect {
                    x: inner.x, y: inner.y + screen_top as u16,
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
        .border_style(if focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) });
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
    app.meta_scroll  = app.meta_scroll.min(app.meta_lines.len().saturating_sub(1));

    let vis: Vec<Line<'static>> = app.meta_lines.iter()
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
        &mathjax_svg_rs::Options { font_size: 32.0, ..Default::default() },
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
    Some(ContentBlock::MathImage { state, height_lines })
}

// ── Doc content loading ───────────────────────────────────────────────────────

/// Render all `items` into a single content area, building per-item offset tables.
///
/// Link virtual lines from each `markdown_to_blocks` call are offset by the running
/// total so they point into the combined block list.
fn load_all_items(
    items: &[DocItem],
    item_offset: usize,
    ctx: &RenderCtx,
    out_blocks:   &mut Vec<ContentBlock>,
    out_links:    &mut Vec<(usize, String)>,
    out_vlines:   &mut Vec<usize>,
    out_line_map: &mut std::collections::HashMap<usize, usize>,
) {
    for (i, item) in items.iter().enumerate() {
        let item_idx = item_offset + i;
        let start_vline: usize = out_blocks.iter().map(|b| b.line_count()).sum();
        out_vlines.push(start_vline);
        out_line_map.insert(item_idx, start_vline);

        let md = docify::render_tui::items_to_markdown(std::slice::from_ref(item));
        let (item_blocks, item_links) = markdown_to_blocks(&md, 80, ctx);
        for (vl, target) in item_links {
            out_links.push((start_vline + vl, target));
        }
        out_blocks.extend(item_blocks);
    }
}

/// Return `true` if `dep` has any readable README / doc file.
fn readme_exists(dep: &DocDependency) -> bool {
    if dep.docs.iter().any(|p| !p.extension().map_or(false, |e| e == "html")) {
        return true;
    }
    dep.path.as_ref().map_or(false, |root| {
        ["README.md", "readme.md", "README"].iter().any(|n| root.join(n).exists())
    })
}

/// Load the README / markdown docs for the centre panel when a dep is first opened.
fn load_readme_content(dep: &DocDependency, ctx: &RenderCtx) -> (Vec<ContentBlock>, Vec<(usize, String)>) {
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
    (vec![ContentBlock::Lines(vec![
        Line::raw(format!("No README found for '{}'.", dep.name)),
        Line::raw(""),
        Line::raw("Use [2] Files to browse the API tree."),
    ])], Vec::new())
}

/// Extract all doc items from a dependency's source tree.
fn extract_dep_items(dep: &DocDependency) -> Vec<DocItem> {
    let Some(dep_dir) = &dep.path else { return Vec::new(); };
    let src_dir  = dep_dir.join("src");
    let scan_dir = if src_dir.is_dir() { src_dir } else { dep_dir.clone() };
    docify::extract::extract_dir(&scan_dir).items
}

/// Build API sub-tree nodes for one dependency's items.
///
/// `item_offset` is the position of `items[0]` in the global `doc_items` vec.
/// `base_depth` is the depth of the generated nodes (1 for dep children).
/// Items with `::` or `.` in the name are grouped under expandable namespace nodes.
fn build_api_subtree(items: &[DocItem], item_offset: usize, base_depth: usize) -> Vec<TreeNode> {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut roots:  Vec<usize>                   = Vec::new();

    for (i, item) in items.iter().enumerate() {
        if item.name.is_empty() { continue; } // skip file-level/anonymous items
        let global = item_offset + i;
        let sep = item.name.rfind("::").or_else(|| item.name.rfind('.'));
        if let Some(p) = sep {
            groups.entry(item.name[..p].to_string()).or_default().push(global);
        } else {
            roots.push(global);
        }
    }

    let mut tree = Vec::new();

    for gi in roots {
        tree.push(TreeNode {
            label:    items[gi - item_offset].name.clone(),
            depth:    base_depth,
            kind:     TreeNodeKind::Symbol,
            expanded: false,
            item_idx: Some(gi),
            dep_idx:  None,
            loaded:   false,
        });
    }

    for (ns, members) in &groups {
        tree.push(TreeNode {
            label:    ns.clone(),
            depth:    base_depth,
            kind:     TreeNodeKind::Group,
            expanded: true,
            item_idx: None,
            dep_idx:  None,
            loaded:   false,
        });
        for &gi in members {
            let item = &items[gi - item_offset];
            let local = item.name.rfind("::").or_else(|| item.name.rfind('.'))
                .map_or(item.name.as_str(), |p| &item.name[p + 2..]);
            tree.push(TreeNode {
                label:    local.to_owned(),
                depth:    base_depth + 1,
                kind:     TreeNodeKind::Symbol,
                expanded: false,
                item_idx: Some(gi),
                dep_idx:  None,
                loaded:   false,
            });
        }
    }

    tree
}

/// Return indices of currently-visible tree nodes, respecting collapsed groups and deps.
fn visible_nodes(tree: &[TreeNode]) -> Vec<usize> {
    let mut vis  = Vec::new();
    let mut skip: Option<usize> = None;
    for (i, node) in tree.iter().enumerate() {
        if let Some(d) = skip {
            if node.depth > d { continue; }
            skip = None;
        }
        vis.push(i);
        if matches!(node.kind, TreeNodeKind::Group | TreeNodeKind::Dep) && !node.expanded {
            skip = Some(node.depth);
        }
    }
    vis
}

/// Render package metadata from the dep's freight.toml as styled lines.
fn render_pkg_meta(dep: &DocDependency) -> Vec<Line<'static>> {
    let key_sty  = Style::default().fg(Color::DarkGray);
    let link_sty = Style::default().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
    let bold     = Style::default().add_modifier(Modifier::BOLD);

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

    kv!("name",    &dep.name);
    kv!("version", &dep.version);
    kv!("kind",    &dep.kind);
    kv!("source",  &dep.source);

    if let Some(root) = &dep.path {
        if let Ok(manifest) = load_manifest(root) {
            let pkg = &manifest.package;

            if !pkg.license.is_empty() { kv!("license", &pkg.license); }

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
    let mut out  = Vec::new();
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
    if !line.is_empty() { out.push(line); }
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
            if let ContentBlock::MathImage { state, height_lines } = img_block {
                builder.push_image(state, height_lines);
                builder.push(Line::raw(""));
                return;
            }
        }
    }
    let rendered = docify::util::latex::render_math_block(latex);
    builder.push(Line::from(vec![Span::raw("    ".to_owned()), Span::raw(rendered)]));
    builder.push(Line::raw(""));
}

/// Render a fenced or indented code block as a rounded-corner bordered box.
fn render_code_block(lang: &str, code: &str, width: usize) -> Vec<Line<'static>> {
    let bdr      = Style::default().fg(Color::DarkGray);
    let code_sty = Style::default().fg(Color::LightGreen);
    let inner    = width.saturating_sub(4);
    let mut out  = Vec::new();

    let top = if lang.is_empty() {
        format!("  ╭{}", "─".repeat(inner + 2))
    } else {
        let label = format!(" {lang} ");
        let llen  = label.chars().count();
        let bars  = (inner + 2).saturating_sub(llen + 1);
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
    out.push(Line::from(Span::styled(format!("  ╰{}", "─".repeat(inner + 2)), bdr)));
    out
}

/// Render a markdown table with box-drawing borders.
fn render_md_table(header: &[String], rows: &[Vec<String>]) -> Vec<Line<'static>> {
    let bdr      = Style::default().fg(Color::DarkGray);
    let hdr_sty  = Style::default().add_modifier(Modifier::BOLD);
    let code_sty = Style::default().fg(Color::LightGreen);

    let ncols = header.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    if ncols == 0 { return vec![]; }

    let col_widths: Vec<usize> = (0..ncols).map(|c| {
        let hw = header.get(c).map_or(0, |s| s.chars().count());
        let rw = rows.iter().map(|r| r.get(c).map_or(0, |s| s.chars().count())).max().unwrap_or(0);
        hw.max(rw).max(3)
    }).collect();

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
            let padded: String = format!(" {content:<w$} ", w = w).chars().take(w + 2).collect();
            spans.push(Span::styled(padded, sty));
            spans.push(Span::styled("│".to_owned(), bdr));
        }
        Line::from(spans)
    };

    let is_separator = |row: &[String]| {
        !row.is_empty() && row.iter().all(|s| s.trim().chars().all(|c| c == '─' || c == '-' || c == ' '))
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
    let mut cont  = String::new();
    for _ in 0..bq { first.push_str("▌ "); cont.push_str("▌ "); }
    let depth = stack.len();
    for (d, (ordered, num)) in stack.iter().enumerate() {
        if d + 1 < depth {
            first.push_str("  "); cont.push_str("  ");
        } else if item_first {
            if *ordered {
                let b = format!("{num}. ");
                let p = " ".repeat(b.len());
                first.push_str(&b); cont.push_str(&p);
            } else {
                first.push_str("• "); cont.push_str("  ");
            }
        } else {
            let pad = " ".repeat(if *ordered { format!("{num}. ").len() } else { 2 });
            first.push_str(&pad); cont.push_str(&pad);
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
    let indent = match level { HL::H1 => 0, HL::H2 => 1, HL::H3 => 2, HL::H4 => 3, HL::H5 => 4, HL::H6 => 5 };
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
                _      => Color::White,
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
    let bg_plain  = Style::default().bg(bg).fg(Color::White);
    let bg_ret    = Style::default().bg(bg).fg(Color::LightBlue);
    let bg_name   = Style::default().bg(bg).fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let bg_type   = Style::default().bg(bg).fg(Color::LightCyan);
    let bg_punct  = Style::default().bg(bg).fg(Color::DarkGray);

    // Find the opening paren of the parameter list.
    let paren = sig.find('(');
    if paren.is_none() {
        // Not a function — struct/class/typedef/variable: highlight kind keyword.
        let t = sig.trim();
        let kind_end = t.find(' ').unwrap_or(t.len());
        let (kw, rest) = t.split_at(kind_end);
        let kw_sty = match kw {
            "struct" | "class" | "enum" => Style::default().bg(bg).fg(Color::LightGreen).add_modifier(Modifier::BOLD),
            "typedef" | "using"         => Style::default().bg(bg).fg(Color::LightMagenta).add_modifier(Modifier::BOLD),
            "const" | "static"          => Style::default().bg(bg).fg(Color::LightCyan).add_modifier(Modifier::BOLD),
            _                           => bg_name,
        };
        out.push(Span::styled(format!("{kw}"), kw_sty));
        out.push(Span::styled(format!("{rest} "), bg_plain));
        return;
    }
    let paren = paren.unwrap();
    let before = sig[..paren].trim_end();
    let params  = &sig[paren..]; // "(int a, int b)"

    // Split return-type from function name.
    // The last token before `(` is the name (may have * prefix for pointer-returning fns).
    let last_space = before.rfind(|c: char| c.is_ascii_whitespace() || c == '*');
    let (ret_part, name_part) = if let Some(p) = last_space {
        let split_at = if before.as_bytes().get(p) == Some(&b'*') { p } else { p + 1 };
        (&before[..split_at], &before[split_at..])
    } else {
        ("", before)
    };

    if !ret_part.is_empty() {
        out.push(Span::styled(format!("{} ", ret_part.trim_end()), bg_ret));
    }
    let name_clean = name_part.trim_start_matches('*');
    let ptr_stars  = &name_part[..name_part.len() - name_clean.len()];
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
        if tok.is_empty() { return; }
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
                '*' | '&'             => Style::default().bg(bg).fg(Color::LightMagenta),
                ' '                   => { out.push(Span::styled(" ", plain_sty)); continue; }
                _                     => plain_sty,
            };
            out.push(Span::styled(c.to_string(), sty));
        }
    }
    flush(&mut tok, &mut last_was_type, out);
}

fn is_c_type_keyword(s: &str) -> bool {
    matches!(s,
        "int" | "long" | "short" | "char" | "void" | "float" | "double" | "bool"
        | "unsigned" | "signed" | "const" | "volatile" | "restrict" | "static"
        | "inline" | "extern" | "register" | "auto" | "struct" | "class" | "enum"
        | "union" | "typename" | "template" | "size_t" | "ssize_t"
        | "uint8_t" | "uint16_t" | "uint32_t" | "uint64_t" | "uintptr_t"
        | "int8_t"  | "int16_t"  | "int32_t"  | "int64_t"  | "intptr_t"
        | "ptrdiff_t" | "nullptr_t" | "string" | "vector" | "map" | "set"
    )
}




// ── Block builder ─────────────────────────────────────────────────────────────

struct DocBlockBuilder {
    blocks: Vec<ContentBlock>,
    pending: Vec<Line<'static>>,
}

impl DocBlockBuilder {
    fn new() -> Self { Self { blocks: Vec::new(), pending: Vec::new() } }

    fn push(&mut self, line: Line<'static>) {
        self.pending.push(line);
    }

    fn virtual_line(&self) -> usize {
        self.blocks.iter().map(|b| b.line_count()).sum::<usize>() + self.pending.len()
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.blocks.push(ContentBlock::Lines(std::mem::take(&mut self.pending)));
        }
    }

    #[cfg(feature = "rich-math")]
    fn push_image(&mut self, state: ratatui_image::protocol::StatefulProtocol, height_lines: u16) {
        self.flush();
        self.blocks.push(ContentBlock::MathImage { state, height_lines });
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
    let mut links:   Vec<(usize, String)> = Vec::new();

    // Each word carries (text, style, optional_link_dest).
    let mut words: Vec<(String, Style, Option<String>)> = Vec::new();
    let mut bold         = false;
    let mut italic       = false;
    let mut strike       = false;
    let mut current_link: Option<String> = None;
    let code_sty         = Style::default().fg(Color::LightGreen);
    let link_sty         = Style::default().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);

    let mut in_code   = false;
    let mut code_lang = String::new();
    let mut code_text = String::new();

    let mut hd_level: Option<HeadingLevel> = None;
    let mut hd_text:  String               = String::new();

    let mut tbl_header: Vec<String>      = Vec::new();
    let mut tbl_rows:   Vec<Vec<String>> = Vec::new();
    let mut tbl_row:    Vec<String>      = Vec::new();
    let mut tbl_cell:   String           = String::new();
    let mut in_thead                     = false;
    let mut in_cell                      = false;

    let mut list_stack: Vec<(bool, u64)> = Vec::new();
    let mut item_first                   = false;

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
                let mut cur: Vec<Span<'static>>   = vec![Span::raw(fp)];
                let mut cur_link: Option<String>  = None;
                let mut len: usize = 0;
                let mut fst = true;
                for (word, sty, ldest) in words.drain(..) {
                    if let Some(ref d) = ldest { cur_link = Some(d.clone()); }
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
                    if let Some(ref d) = cur_link { links.push((builder.virtual_line(), d.clone())); }
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
            if bold   { s = s.add_modifier(Modifier::BOLD); }
            if italic { s = s.add_modifier(Modifier::ITALIC); }
            if strike { s = s.add_modifier(Modifier::CROSSED_OUT); }
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
                if !dest.is_empty() { current_link = Some(dest); }
            }
            Event::End(TagEnd::Link) => { current_link = None; }

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
                    CodeBlockKind::Indented     => String::new(),
                };
                code_text.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                for l in render_code_block(&code_lang, &code_text, width) { builder.push(l); }
                builder.push(Line::raw(""));
                in_code = false;
                code_lang.clear();
                code_text.clear();
            }

            Event::Start(Tag::BlockQuote(_)) => { bq += 1; }
            Event::End(TagEnd::BlockQuote(_)) => { bq = bq.saturating_sub(1); }

            Event::Start(Tag::List(n)) => {
                list_stack.push((n.is_some(), n.map(|v| v.saturating_sub(1)).unwrap_or(0)));
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                if list_stack.is_empty() { builder.push(Line::raw("")); }
            }
            Event::Start(Tag::Item) => {
                if let Some(last) = list_stack.last_mut() {
                    if last.0 { last.1 += 1; }
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

            Event::Start(Tag::Table(_)) => { tbl_header.clear(); tbl_rows.clear(); }
            Event::End(TagEnd::Table) => {
                for l in render_md_table(&tbl_header, &tbl_rows) { builder.push(l); }
                builder.push(Line::raw(""));
            }
            Event::Start(Tag::TableHead) => { in_thead = true; tbl_row.clear(); }
            Event::End(TagEnd::TableHead) => {
                tbl_header = std::mem::take(&mut tbl_row);
                in_thead = false;
            }
            Event::Start(Tag::TableRow) => { tbl_row.clear(); }
            Event::End(TagEnd::TableRow) => {
                if !in_thead { tbl_rows.push(std::mem::take(&mut tbl_row)); }
            }
            Event::Start(Tag::TableCell) => { tbl_cell.clear(); in_cell = true; }
            Event::End(TagEnd::TableCell) => {
                tbl_row.push(std::mem::take(&mut tbl_cell));
                in_cell = false;
            }

            Event::Rule => {
                builder.push(Line::styled("─".repeat(width), Style::default().fg(Color::DarkGray)));
                builder.push(Line::raw(""));
            }

            Event::Start(Tag::Strong)        => { bold   = true; }
            Event::End(TagEnd::Strong)        => { bold   = false; }
            Event::Start(Tag::Emphasis)       => { italic = true; }
            Event::End(TagEnd::Emphasis)      => { italic = false; }
            Event::Start(Tag::Strikethrough)  => { strike = true; }
            Event::End(TagEnd::Strikethrough) => { strike = false; }

            Event::Text(t) => {
                if in_code {
                    code_text.push_str(&t);
                } else if hd_level.is_some() {
                    hd_text.push_str(&t);
                } else if in_cell {
                    tbl_cell.push_str(&t);
                } else {
                    let sty = if current_link.is_some() { link_sty } else { isty!() };
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
                    let sty = if current_link.is_some() { link_sty } else { code_sty };
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
        return (vec![ContentBlock::Lines(vec![Line::raw("(no content)")])], Vec::new());
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
            name:      name.to_string(),
            kind,
            brief:     "A brief.".to_string(),
            body:      String::new(),
            tags:      Vec::new(),
            file:      PathBuf::from("test.cpp"),
            line:      1,
            lang:      DocLanguage::Cpp,
            signature: String::new(),
            meta:      DocMeta::default(),
        }
    }

    #[test]
    fn build_api_subtree_namespaced_items_visible() {
        // Items with "ns::name" form should be grouped and visible once dep expands.
        let items = vec![
            make_item("stats::mean",     DocKind::Function),
            make_item("stats::variance", DocKind::Function),
            make_item("stats::OrderStatistics", DocKind::Class),
            make_item("",                DocKind::Unknown), // @file block — should be skipped
        ];
        let sub = build_api_subtree(&items, 0, 1);

        // Should have: Group "stats" + 3 Symbol children (empty-named item skipped).
        let groups: Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::Group)).collect();
        let syms:   Vec<_> = sub.iter().filter(|n| matches!(n.kind, TreeNodeKind::Symbol)).collect();
        assert_eq!(groups.len(), 1, "expected one 'stats' group");
        assert_eq!(syms.len(),   3, "expected 3 symbols");
        assert_eq!(groups[0].label, "stats");
        assert!(groups[0].expanded, "group should start expanded");

        // Build a minimal tree with a Dep + the subtree.
        let mut tree = vec![TreeNode {
            label:    "dep 0.1".to_string(),
            depth:    0,
            kind:     TreeNodeKind::Dep,
            expanded: true,
            item_idx: None,
            dep_idx:  Some(0),
            loaded:   true,
        }];
        tree.extend(sub);

        let vis = visible_nodes(&tree);
        // All nodes should be visible: dep + group + 3 symbols = 5.
        assert_eq!(vis.len(), 5, "dep, group, and all 3 symbols should be visible");
    }

    #[test]
    fn build_api_subtree_flat_items_visible() {
        // Items without "::" go to roots and are directly visible.
        let items = vec![
            make_item("clamp", DocKind::Function),
            make_item("lerp",  DocKind::Function),
        ];
        let sub = build_api_subtree(&items, 0, 1);
        assert_eq!(sub.len(), 2);
        assert!(sub.iter().all(|n| matches!(n.kind, TreeNodeKind::Symbol)));

        let mut tree = vec![TreeNode {
            label:    "dep 0.1".to_string(),
            depth:    0,
            kind:     TreeNodeKind::Dep,
            expanded: true,
            item_idx: None,
            dep_idx:  Some(0),
            loaded:   true,
        }];
        tree.extend(sub);
        let vis = visible_nodes(&tree);
        assert_eq!(vis.len(), 3, "dep + 2 symbols");
    }

    #[test]
    fn collapsed_dep_hides_children() {
        let items = vec![make_item("foo", DocKind::Function)];
        let sub   = build_api_subtree(&items, 0, 1);
        let mut tree = vec![TreeNode {
            label:    "dep 0.1".to_string(),
            depth:    0,
            kind:     TreeNodeKind::Dep,
            expanded: false, // collapsed
            item_idx: None,
            dep_idx:  Some(0),
            loaded:   true,
        }];
        tree.extend(sub);
        let vis = visible_nodes(&tree);
        assert_eq!(vis.len(), 1, "only dep itself visible when collapsed");
    }
}
