use std::collections::{HashMap, HashSet};
use std::io;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::doc::{DocItem, DocKind, DocLanguage, TagKind};

use crate::doc::latex::render_math_lines;
use crate::doc::stdlib::StdlibMsg;

// ── Colors ────────────────────────────────────────────────────────────────────

const COLOR_PKG: Color = Color::Rgb(220, 200, 120);
const COLOR_SECTION: Color = Color::Rgb(140, 140, 180);
const COLOR_SYMBOL: Color = Color::Rgb(200, 220, 200);
const COLOR_BRIEF: Color = Color::Rgb(180, 180, 180);
const COLOR_BORDER: Color = Color::Rgb(80, 80, 100);
const COLOR_ACTIVE: Color = Color::Rgb(120, 160, 220);
const COLOR_HINT: Color = Color::DarkGray;
const COLOR_KIND: Color = Color::Rgb(130, 170, 130);

// ── Public data type ──────────────────────────────────────────────────────────

/// All documentation items belonging to one freight package.
pub struct PackageDoc {
    pub name: String,
    pub version: String,
    pub items: Vec<DocItem>,
    pub readme: Option<String>,
}

// ── Tree classification ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    ClassesTypes,
    Namespaces,
    FreeSymbols,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::ClassesTypes => "Classes & Types",
            Section::Namespaces => "Namespaces",
            Section::FreeSymbols => "Free Symbols",
        }
    }
    const ALL: [Section; 3] = [
        Section::ClassesTypes,
        Section::Namespaces,
        Section::FreeSymbols,
    ];
}

fn item_section(item: &DocItem) -> Option<Section> {
    // Class members are shown on the class detail page, not in any section.
    if item.meta.parent.is_some() {
        return None;
    }
    Some(match item.kind {
        DocKind::Class
        | DocKind::Struct
        | DocKind::Interface
        | DocKind::Enum
        | DocKind::Typedef => Section::ClassesTypes,
        DocKind::Module => Section::Namespaces,
        _ => Section::FreeSymbols,
    })
}

/// Items whose `meta.parent` simple name matches `class_simple_name`.
fn class_members<'a>(items: &'a [DocItem], class_simple_name: &str) -> Vec<&'a DocItem> {
    items
        .iter()
        .filter(|i| i.meta.parent.as_deref() == Some(class_simple_name))
        .collect()
}

/// Derive the set of unique namespace / scope prefixes from all item names.
///
/// Splits on `::` (C++/Rust/D) and `.` (Python/Go/Lua) and collects every
/// non-leaf prefix. Items whose kind is already `Module` contribute their name
/// directly. Result is sorted and deduplicated.
fn derive_namespaces(items: &[DocItem]) -> Vec<String> {
    // Collect the simple names of all documented classes so we can exclude
    // them from the namespace list (a class is not a namespace).
    let class_names: std::collections::HashSet<&str> = items
        .iter()
        .filter(|i| {
            matches!(
                i.kind,
                DocKind::Class | DocKind::Struct | DocKind::Interface
            )
        })
        .map(|i| {
            i.name
                .rsplit("::")
                .next()
                .or_else(|| i.name.rsplit('.').next())
                .unwrap_or(&i.name)
        })
        .collect();

    let mut set = std::collections::BTreeSet::new();
    for item in items {
        // Class members' qualified names would otherwise pollute the namespace
        // list — skip them; they're reachable via the class detail page.
        if item.meta.parent.is_some() {
            continue;
        }
        if item.kind == DocKind::Module && !item.name.is_empty() {
            set.insert(item.name.clone());
            continue;
        }
        for sep in ["::", "."] {
            let parts: Vec<&str> = item.name.split(sep).collect();
            if parts.len() > 1 {
                for i in 1..parts.len() {
                    let ns = parts[..i].join(sep);
                    // Don't turn a class name into a namespace entry.
                    let simple = ns
                        .rsplit("::")
                        .next()
                        .or_else(|| ns.rsplit('.').next())
                        .unwrap_or(&ns);
                    if !ns.is_empty() && !class_names.contains(simple) {
                        set.insert(ns);
                    }
                }
                break;
            }
        }
    }
    set.into_iter().collect()
}

/// All items that belong to namespace `ns` — their name starts with `ns::` or `ns.`.
fn items_in_ns<'a>(items: &'a [DocItem], ns: &str) -> Vec<&'a DocItem> {
    let prefix_cc = format!("{ns}::");
    let prefix_dot = format!("{ns}.");
    items
        .iter()
        .filter(|item| {
            item.kind != DocKind::Module
            // Class members belong to their class, not to the enclosing namespace.
            && item.meta.parent.is_none()
            && (item.name.starts_with(&prefix_cc) || item.name.starts_with(&prefix_dot))
        })
        .collect()
}

fn kind_label(kind: &DocKind) -> &'static str {
    match kind {
        DocKind::Function | DocKind::Subroutine => "fn",
        DocKind::Class => "class",
        DocKind::Struct => "struct",
        DocKind::Interface => "iface",
        DocKind::Enum => "enum",
        DocKind::Typedef => "type",
        DocKind::Module => "ns",
        DocKind::Variable => "var",
        DocKind::Macro => "macro",
        DocKind::Unknown => "?",
    }
}

// ── Tree rows ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TreeRow {
    depth: usize,
    label: String,
    count: usize, // item count — shown on pkg/section rows
    kind: RowKind,
}

#[derive(Clone)]
enum RowKind {
    Package {
        pkg_idx: usize,
    },
    /// Expandable namespace — children are its member items.
    Namespace {
        pkg_idx: usize,
        name: String,
    },
    /// Expandable class / struct / typedef — children are its member items.
    Group {
        pkg_idx: usize,
        item_idx: usize,
    },
    /// Leaf symbol with no members.
    Symbol {
        pkg_idx: usize,
        item_idx: usize,
    },
    /// Child of a Group or Namespace — shows full documentation when selected.
    Member {
        pkg_idx: usize,
        item_idx: usize,
    },
}

// ── App ───────────────────────────────────────────────────────────────────────

struct App {
    /// Visible packages — shown in the sidebar tree.
    packages: Vec<PackageDoc>,
    /// Hidden packages (stdlib, etc.) — in sym_index but not in the tree.
    hidden: Vec<PackageDoc>,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
    list_state: ListState,
    scroll: u16,
    focus: Focus,
    query: String,
    tree_area: Rect,
    filter_area: Rect,
    detail_area: Rect,
    /// (pkg_idx, item_idx) of a hidden item being shown in the detail pane.
    /// pkg_idx is an index into `hidden`.
    external_item: Option<(usize, usize)>,
    sym_index: HashMap<String, (usize, usize, bool)>, // name → (pkg_idx, item_idx, is_hidden)
    detail_links: Vec<DocLink>,
    /// When true, detail pane shows source file instead of doc text.
    show_source: bool,
    /// Cached source file: (path, lines). Invalidated when the path changes.
    source_cache: Option<(std::path::PathBuf, Vec<String>)>,
    /// Loading indicator while the background stdlib thread is running.
    stdlib_status: Option<(usize, usize, String)>, // (done, total, label)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Filter,
    Tree,
    Detail,
}

fn group_or_leaf_rows(
    items: &[DocItem],
    pi: usize,
    ii: usize,
    depth: usize,
    expanded: &HashSet<String>,
) -> Vec<TreeRow> {
    let item = &items[ii];
    if has_members(items, item) {
        let gexp = expanded.contains(&grp_key(pi, ii));
        let members = class_members(items, &item_display_name(item));
        let count = members.len();
        let mut rows = vec![TreeRow {
            depth,
            label: item_display_name(item),
            count,
            kind: RowKind::Group {
                pkg_idx: pi,
                item_idx: ii,
            },
        }];
        if gexp {
            for m in members {
                let mi = items.iter().position(|i| std::ptr::eq(i, m)).unwrap();
                rows.push(TreeRow {
                    depth: depth + 1,
                    label: item_display_name(m),
                    count: 0,
                    kind: RowKind::Member {
                        pkg_idx: pi,
                        item_idx: mi,
                    },
                });
            }
        }
        rows
    } else {
        vec![TreeRow {
            depth,
            label: item_display_name(item),
            count: 0,
            kind: RowKind::Symbol {
                pkg_idx: pi,
                item_idx: ii,
            },
        }]
    }
}

// ── Cross-reference links ─────────────────────────────────────────────────────

#[derive(Clone)]
struct DocLink {
    line: usize,
    start_col: usize,
    end_col: usize,
    pkg_idx: usize,
    item_idx: usize,
    is_hidden: bool,
}

/// Build a name → (pkg_idx, item_idx, is_hidden) index for all documented symbols.
fn build_sym_index(
    packages: &[PackageDoc],
    hidden: &[PackageDoc],
) -> HashMap<String, (usize, usize, bool)> {
    let mut map = HashMap::new();
    let insert = |map: &mut HashMap<String, (usize, usize, bool)>, key: String, pi, ii, h| {
        map.entry(key).or_insert((pi, ii, h));
    };
    for (is_hidden, pkgs) in [(false, packages), (true, hidden)] {
        for (pi, pkg) in pkgs.iter().enumerate() {
            for (ii, item) in pkg.items.iter().enumerate() {
                let display = item_display_name(item);
                insert(&mut map, display, pi, ii, is_hidden);
                insert(&mut map, item.name.clone(), pi, ii, is_hidden);
                for sep in ["::", "."] {
                    let mut remaining = item.name.as_str();
                    while let Some((_, tail)) = remaining.split_once(sep) {
                        insert(&mut map, tail.to_string(), pi, ii, is_hidden);
                        remaining = tail;
                    }
                }
            }
        }
    }
    map
}

/// Scan the text of a rendered line and record clickable symbol references.
/// Also updates the spans in-place to add underline + blue styling for matches.
/// `current` is the (pkg_idx, item_idx) of the symbol being displayed — skip self-links.
fn annotate_links(
    lines: &mut Vec<Line<'static>>,
    sym_index: &HashMap<String, (usize, usize, bool)>,
    links: &mut Vec<DocLink>,
    current: Option<(usize, usize)>,
) {
    for (line_no, line) in lines.iter_mut().enumerate() {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let found = find_sym_refs(&text, sym_index);
        for (start_col, end_col, pi, ii, hidden) in found {
            if !hidden && current == Some((pi, ii)) {
                continue;
            }
            links.push(DocLink {
                line: line_no,
                start_col,
                end_col,
                pkg_idx: pi,
                item_idx: ii,
                is_hidden: hidden,
            });
        }
    }
}

/// Find all symbol references in text, returning (start_col, end_col, pkg_idx, item_idx, is_hidden).
fn find_sym_refs(
    text: &str,
    sym_index: &HashMap<String, (usize, usize, bool)>,
) -> Vec<(usize, usize, usize, usize, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let mut results = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            // Collect identifier, possibly qualified with :: or .
            while i < chars.len() {
                if chars[i].is_alphanumeric() || chars[i] == '_' {
                    i += 1;
                } else if chars.get(i..i + 2) == Some(&[':', ':']) {
                    i += 2;
                } else {
                    break;
                }
            }
            let token: String = chars[start..i].iter().collect();
            // Require at least 3 chars to avoid linking common words like "a", "fn".
            if token.len() >= 3 {
                if let Some(&(pi, ii, hidden)) = sym_index.get(&token) {
                    results.push((start, i, pi, ii, hidden));
                }
            }
        } else {
            i += 1;
        }
    }
    results
}

fn pkg_key(pkg_idx: usize) -> String {
    format!("p:{pkg_idx}")
}
fn grp_key(pkg_idx: usize, item_idx: usize) -> String {
    format!("g:{pkg_idx}:{item_idx}")
}
fn ns_key(pkg_idx: usize, name: &str) -> String {
    format!("n:{pkg_idx}:{name}")
}

fn has_members(items: &[DocItem], item: &DocItem) -> bool {
    let simple = item_display_name(item);
    items
        .iter()
        .any(|i| i.meta.parent.as_deref() == Some(simple.as_str()))
}

fn is_in_namespace(item: &DocItem, namespaces: &[String]) -> bool {
    for sep in ["::", "."] {
        if let Some((prefix, _)) = item.name.rsplit_once(sep) {
            if namespaces.iter().any(|ns| ns == prefix) {
                return true;
            }
        }
    }
    false
}

fn is_type_item(kind: &DocKind) -> bool {
    matches!(
        kind,
        DocKind::Class | DocKind::Struct | DocKind::Interface | DocKind::Typedef | DocKind::Enum
    )
}

impl App {
    fn new(packages: Vec<PackageDoc>, hidden: Vec<PackageDoc>) -> Self {
        let sym_index = build_sym_index(&packages, &hidden);
        let mut app = Self {
            packages,
            hidden,
            expanded: HashSet::new(),
            rows: Vec::new(),
            list_state: ListState::default(),
            scroll: 0,
            focus: Focus::Tree,
            query: String::new(),
            tree_area: Rect::default(),
            filter_area: Rect::default(),
            detail_area: Rect::default(),
            external_item: None,
            sym_index,
            detail_links: Vec::new(),
            show_source: false,
            source_cache: None,
            stdlib_status: Some((0, 1, "starting…".to_string())),
        };
        // Expand first package by default.
        if !app.packages.is_empty() {
            app.expanded.insert(pkg_key(0));
        }
        app.rebuild_rows();
        if !app.rows.is_empty() {
            app.list_state.select(Some(0));
        }
        app
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        if !self.query.is_empty() {
            self.rebuild_filtered_rows();
            return;
        }
        for (pi, pkg) in self.packages.iter().enumerate() {
            let pexp = self.expanded.contains(&pkg_key(pi));
            self.rows.push(TreeRow {
                depth: 0,
                label: format!("{} {}", pkg.name, pkg.version),
                count: pkg.items.len(),
                kind: RowKind::Package { pkg_idx: pi },
            });
            if !pexp {
                continue;
            }

            let namespaces = derive_namespaces(&pkg.items);

            // 1. Type items at root (not in any namespace, not a class member).
            let type_items: Vec<usize> = pkg
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| {
                    it.meta.parent.is_none()
                        && is_type_item(&it.kind)
                        && !is_in_namespace(it, &namespaces)
                })
                .map(|(i, _)| i)
                .collect();

            for ii in type_items {
                let extra = group_or_leaf_rows(&pkg.items, pi, ii, 1, &self.expanded);
                self.rows.extend(extra);
            }

            // 2. Namespaces — expandable, show [namespace] badge.
            for ns in &namespaces {
                let ns_items = items_in_ns(&pkg.items, ns);
                let nexp = self.expanded.contains(&ns_key(pi, ns));
                self.rows.push(TreeRow {
                    depth: 1,
                    label: ns.clone(),
                    count: ns_items.len(),
                    kind: RowKind::Namespace {
                        pkg_idx: pi,
                        name: ns.clone(),
                    },
                });
                if !nexp {
                    continue;
                }
                for item in &ns_items {
                    let ii = pkg
                        .items
                        .iter()
                        .position(|i| std::ptr::eq(i, *item))
                        .unwrap();
                    if is_type_item(&item.kind) && has_members(&pkg.items, item) {
                        let extra = group_or_leaf_rows(&pkg.items, pi, ii, 2, &self.expanded);
                        self.rows.extend(extra);
                    } else {
                        self.rows.push(TreeRow {
                            depth: 2,
                            label: item_display_name(item),
                            count: 0,
                            kind: RowKind::Member {
                                pkg_idx: pi,
                                item_idx: ii,
                            },
                        });
                    }
                }
            }

            // 3. Free symbols — not a class member, not in any namespace, not a type item.
            let free_items: Vec<usize> = pkg
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| {
                    it.meta.parent.is_none()
                        && !is_type_item(&it.kind)
                        && it.kind != DocKind::Module
                        && !is_in_namespace(it, &namespaces)
                })
                .map(|(i, _)| i)
                .collect();

            for ii in free_items {
                let item = &pkg.items[ii];
                self.rows.push(TreeRow {
                    depth: 1,
                    label: item_display_name(item),
                    count: 0,
                    kind: RowKind::Symbol {
                        pkg_idx: pi,
                        item_idx: ii,
                    },
                });
            }
        }
    }

    fn rebuild_filtered_rows(&mut self) {
        let q = self.query.to_ascii_lowercase();
        for (pi, pkg) in self.packages.iter().enumerate() {
            let mut matches: Vec<TreeRow> = Vec::new();
            // Items whose name or display name contains the query.
            for (ii, item) in pkg.items.iter().enumerate() {
                let display = item_display_name(item);
                if display.to_ascii_lowercase().contains(&q)
                    || item.name.to_ascii_lowercase().contains(&q)
                {
                    let kind = if is_type_item(&item.kind) {
                        RowKind::Group {
                            pkg_idx: pi,
                            item_idx: ii,
                        }
                    } else if item.meta.parent.is_some() {
                        RowKind::Member {
                            pkg_idx: pi,
                            item_idx: ii,
                        }
                    } else {
                        RowKind::Symbol {
                            pkg_idx: pi,
                            item_idx: ii,
                        }
                    };
                    matches.push(TreeRow {
                        depth: 1,
                        label: display,
                        count: 0,
                        kind,
                    });
                }
            }
            // Namespace names.
            let namespaces = derive_namespaces(&pkg.items);
            for ns in &namespaces {
                if ns.to_ascii_lowercase().contains(&q) {
                    let count = items_in_ns(&pkg.items, ns).len();
                    matches.push(TreeRow {
                        depth: 1,
                        label: ns.clone(),
                        count,
                        kind: RowKind::Namespace {
                            pkg_idx: pi,
                            name: ns.clone(),
                        },
                    });
                }
            }
            if matches.is_empty() {
                continue;
            }
            self.rows.push(TreeRow {
                depth: 0,
                label: format!("{} {}", pkg.name, pkg.version),
                count: matches.len(),
                kind: RowKind::Package { pkg_idx: pi },
            });
            self.rows.extend(matches);
        }
    }

    fn selected_row(&self) -> Option<&TreeRow> {
        self.rows.get(self.list_state.selected()?)
    }

    fn toggle_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        match row.kind.clone() {
            RowKind::Package { pkg_idx } => {
                let k = pkg_key(pkg_idx);
                if self.expanded.contains(&k) {
                    self.expanded.remove(&k);
                } else {
                    self.expanded.insert(k);
                }
                self.rebuild_rows();
            }
            RowKind::Namespace { pkg_idx, name } => {
                let k = ns_key(pkg_idx, &name);
                if self.expanded.contains(&k) {
                    self.expanded.remove(&k);
                } else {
                    self.expanded.insert(k);
                }
                self.rebuild_rows();
            }
            RowKind::Group { pkg_idx, item_idx } => {
                let k = grp_key(pkg_idx, item_idx);
                if self.expanded.contains(&k) {
                    self.expanded.remove(&k);
                } else {
                    self.expanded.insert(k);
                }
                self.rebuild_rows();
            }
            RowKind::Symbol { .. } | RowKind::Member { .. } => {}
        }
        // Keep selection in bounds after rebuild.
        let sel = self.list_state.selected().unwrap_or(0);
        if sel >= self.rows.len() {
            self.list_state
                .select(Some(self.rows.len().saturating_sub(1)));
        }
    }

    fn move_sel(&mut self, delta: i32) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, n as i32 - 1) as usize;
        self.list_state.select(Some(next));
        self.external_item = None;
        self.scroll = 0;
    }

    fn contains(area: Rect, x: u16, y: u16) -> bool {
        x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
    }

    fn selected_source_item(&self) -> Option<&DocItem> {
        if let Some((pi, ii)) = self.external_item {
            return self.hidden.get(pi)?.items.get(ii);
        }
        match self.selected_row()?.kind {
            RowKind::Group { pkg_idx, item_idx }
            | RowKind::Symbol { pkg_idx, item_idx }
            | RowKind::Member { pkg_idx, item_idx } => {
                self.packages.get(pkg_idx)?.items.get(item_idx)
            }
            _ => None,
        }
    }

    fn ensure_source_cache(&mut self, path: &std::path::Path) {
        if self.source_cache.as_ref().is_some_and(|(p, _)| p == path) {
            return;
        }
        let lines = std::fs::read_to_string(path)
            .map(|s| s.lines().map(String::from).collect::<Vec<_>>())
            .unwrap_or_default();
        self.source_cache = Some((path.to_path_buf(), lines));
    }

    fn scroll_detail(&mut self, delta: i32) {
        let inner_h = self.detail_area.height.saturating_sub(2) as usize;
        let max = if self.show_source {
            // Virtual scroll: total = header(2) + all source lines.
            let path = self.selected_source_item().map(|i| i.file.clone());
            let total = if let Some(path) = path {
                self.ensure_source_cache(&path);
                self.source_cache
                    .as_ref()
                    .map(|(_, l)| l.len())
                    .unwrap_or(0)
                    + 2
            } else {
                0
            };
            total.saturating_sub(inner_h) as i32
        } else {
            let inner_w = self.detail_area.width.saturating_sub(2) as usize;
            let lines = detail_lines(self);
            visual_row_count(&lines, inner_w).saturating_sub(inner_h) as i32
        };
        self.scroll = (self.scroll as i32 + delta).clamp(0, max) as u16;
    }

    fn activate_detail_link(&mut self, x: u16, y: u16) -> bool {
        if !App::contains(self.detail_area, x, y) {
            return false;
        }
        let inner_w = self.detail_area.width.saturating_sub(2) as usize;
        let visual_row = self.scroll as usize + (y.saturating_sub(self.detail_area.y + 1)) as usize;
        let lines = detail_lines(self);
        let line_idx = visual_to_logical(&lines, visual_row, inner_w);
        let col = x.saturating_sub(self.detail_area.x + 1) as usize;
        let Some(link) = self
            .detail_links
            .iter()
            .find(|l| l.line == line_idx && col >= l.start_col && col < l.end_col)
            .cloned()
        else {
            return false;
        };
        if link.is_hidden {
            self.external_item = Some((link.pkg_idx, link.item_idx));
            self.scroll = 0;
        } else {
            self.external_item = None;
            self.navigate_to(link.pkg_idx, link.item_idx);
        }
        true
    }

    fn navigate_to(&mut self, pkg_idx: usize, item_idx: usize) {
        self.external_item = None;
        self.expanded.insert(pkg_key(pkg_idx));
        let pkg = &self.packages[pkg_idx];
        if let Some(parent_name) = pkg.items[item_idx].meta.parent.clone() {
            if let Some(pi) = pkg
                .items
                .iter()
                .position(|i| item_display_name(i) == parent_name)
            {
                self.expanded.insert(grp_key(pkg_idx, pi));
            }
        }
        self.rebuild_rows();
        self.scroll = 0;
        if let Some(pos) = self.rows.iter().position(|r| match &r.kind {
            RowKind::Group {
                pkg_idx: pi,
                item_idx: ii,
            }
            | RowKind::Symbol {
                pkg_idx: pi,
                item_idx: ii,
            }
            | RowKind::Member {
                pkg_idx: pi,
                item_idx: ii,
            } => *pi == pkg_idx && *ii == item_idx,
            _ => false,
        }) {
            self.list_state.select(Some(pos));
        }
    }

    fn selected_hidden_item(&self) -> Option<&DocItem> {
        let (pi, ii) = self.external_item?;
        self.hidden.get(pi)?.items.get(ii)
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(app: &mut App, f: &mut Frame) {
    let root = f.area();

    // Reserve 1 line at the very bottom for the hint bar.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(root);
    let main_area = rows[0];
    let hint_area = rows[1];

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(main_area);

    // Split left column: filter bar (3 lines) + tree.
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(panes[0]);

    app.filter_area = left[0];
    app.tree_area = left[1];
    app.detail_area = panes[1];

    render_filter(app, f);
    render_tree(app, f);
    render_detail(app, f);

    let hint = " / filter  ↑↓ navigate  Enter expand  ←→ panes  Tab source/docs  PgUp/PgDn scroll  q quit ";
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(COLOR_HINT),
        ))),
        hint_area,
    );
}

fn render_filter(app: &App, f: &mut Frame) {
    let active = app.focus == Focus::Filter;
    let border_style = Style::default().fg(if active { COLOR_ACTIVE } else { COLOR_BORDER });

    // Show stdlib loading progress inside the filter block when loading.
    let title = if let Some((done, total, ref label)) = app.stdlib_status {
        let pct = if total > 0 { done * 100 / total } else { 0 };
        Span::styled(
            format!(" ⟳ stdlib {pct}% {label:.12} "),
            Style::default().fg(Color::Rgb(160, 160, 80)),
        )
    } else if app.query.is_empty() && !active {
        Span::styled(" filter… ", Style::default().fg(COLOR_HINT))
    } else {
        Span::styled(
            format!(" {} ", app.query),
            Style::default().fg(Color::White),
        )
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    // If loading, render a progress bar inside the block.
    if let Some((done, total, _)) = app.stdlib_status {
        let inner = block.inner(app.filter_area);
        f.render_widget(block, app.filter_area);
        if inner.width > 2 && inner.height > 0 {
            let pct = if total > 0 { done * 100 / total } else { 0 };
            let w = inner.width as usize;
            let filled = w * pct / 100;
            let bar = format!(
                "{}{}",
                "█".repeat(filled),
                "░".repeat(w.saturating_sub(filled))
            );
            f.render_widget(
                Paragraph::new(Span::styled(
                    bar,
                    Style::default().fg(Color::Rgb(80, 120, 80)),
                )),
                inner,
            );
        }
    } else {
        f.render_widget(block, app.filter_area);
    }
}

fn render_tree(app: &mut App, f: &mut Frame) {
    let border_style = Style::default().fg(if app.focus == Focus::Tree {
        COLOR_ACTIVE
    } else {
        COLOR_BORDER
    });
    let block = Block::default()
        .title(" Packages ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            let line = match &row.kind {
                RowKind::Package { pkg_idx } => {
                    let arrow = if app.expanded.contains(&pkg_key(*pkg_idx)) {
                        "▼"
                    } else {
                        "▶"
                    };
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled(format!("{arrow} "), Style::default().fg(COLOR_PKG)),
                        Span::styled(
                            row.label.clone(),
                            Style::default().fg(COLOR_PKG).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!(" ({})", row.count), Style::default().fg(COLOR_HINT)),
                    ])
                }
                RowKind::Namespace { pkg_idx, name } => {
                    let arrow = if app.expanded.contains(&ns_key(*pkg_idx, name)) {
                        "▼"
                    } else {
                        "▶"
                    };
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled(format!("{arrow} "), Style::default().fg(COLOR_SECTION)),
                        Span::styled(row.label.clone(), Style::default().fg(COLOR_SECTION)),
                        Span::styled(" [namespace]".to_string(), Style::default().fg(COLOR_KIND)),
                        Span::styled(format!(" ({})", row.count), Style::default().fg(COLOR_HINT)),
                    ])
                }
                RowKind::Group { pkg_idx, item_idx } => {
                    let item = &app.packages[*pkg_idx].items[*item_idx];
                    let kl = kind_label(&item.kind);
                    let arrow = if app.expanded.contains(&grp_key(*pkg_idx, *item_idx)) {
                        "▼"
                    } else {
                        "▶"
                    };
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled(format!("{arrow} "), Style::default().fg(COLOR_SYMBOL)),
                        Span::styled(
                            row.label.clone(),
                            Style::default()
                                .fg(COLOR_SYMBOL)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!(" [{kl}]"), Style::default().fg(COLOR_KIND)),
                        Span::styled(format!(" ({})", row.count), Style::default().fg(COLOR_HINT)),
                    ])
                }
                RowKind::Symbol { pkg_idx, item_idx } => {
                    let item = &app.packages[*pkg_idx].items[*item_idx];
                    let kl = kind_label(&item.kind);
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled(row.label.clone(), Style::default().fg(COLOR_SYMBOL)),
                        Span::styled(format!(" [{kl}]"), Style::default().fg(COLOR_KIND)),
                    ])
                }
                RowKind::Member { pkg_idx, item_idx } => {
                    let item = &app.packages[*pkg_idx].items[*item_idx];
                    let kl = kind_label(&item.kind);
                    Line::from(vec![
                        Span::raw(indent),
                        Span::styled("• ".to_string(), Style::default().fg(COLOR_HINT)),
                        Span::styled(row.label.clone(), Style::default().fg(COLOR_SYMBOL)),
                        Span::styled(format!(" [{kl}]"), Style::default().fg(COLOR_KIND)),
                    ])
                }
            };
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::Rgb(40, 50, 70))
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(list, app.tree_area, &mut app.list_state);
}

/// Estimated total visual rows for `lines` when wrapped to `inner_width` columns.
fn visual_row_count(lines: &[Line<'static>], inner_width: usize) -> usize {
    if inner_width == 0 {
        return lines.len();
    }
    lines
        .iter()
        .map(|line| {
            let chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            chars.div_ceil(inner_width).max(1)
        })
        .sum()
}

/// Map a visual row index to the logical line index that contains it.
fn visual_to_logical(lines: &[Line<'static>], visual_row: usize, inner_width: usize) -> usize {
    if inner_width == 0 || lines.is_empty() {
        return visual_row.min(lines.len().saturating_sub(1));
    }
    let mut v = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        let rows = chars.div_ceil(inner_width).max(1);
        v += rows;
        if v > visual_row {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

fn render_detail(app: &mut App, f: &mut Frame) {
    let border_style = Style::default().fg(if app.focus == Focus::Detail {
        COLOR_ACTIVE
    } else {
        COLOR_BORDER
    });

    let title = detail_title(app);
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(app.detail_area);
    f.render_widget(block, app.detail_area);

    // Source view: virtual window — only render visible lines, no Paragraph::scroll.
    if app.show_source {
        if let Some(item) = app.selected_source_item() {
            let path = item.file.clone();
            let decl_line = item.line.saturating_sub(1);
            let lang = item.lang.clone();
            let file_label = path.display().to_string();
            app.ensure_source_cache(&path);
            if let Some((_, cached)) = &app.source_cache {
                let lines = source_lines_windowed(
                    cached,
                    decl_line,
                    &lang,
                    &file_label,
                    app.scroll as usize,
                    inner.height as usize,
                );
                f.render_widget(Paragraph::new(lines), inner);
            }
        }
        return;
    }

    let mut lines = detail_lines(app);
    app.detail_links.clear();
    let current = if app.external_item.is_some() {
        None
    } else {
        app.selected_row().and_then(|r| match &r.kind {
            RowKind::Group { pkg_idx, item_idx }
            | RowKind::Symbol { pkg_idx, item_idx }
            | RowKind::Member { pkg_idx, item_idx } => Some((*pkg_idx, *item_idx)),
            _ => None,
        })
    };
    annotate_links(&mut lines, &app.sym_index, &mut app.detail_links, current);
    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, inner);
}

fn detail_title(app: &App) -> String {
    if let Some(item) = app.selected_hidden_item() {
        return format!("{} [{}]", item_display_name(item), kind_label(&item.kind));
    }
    match app.selected_row().map(|r| &r.kind) {
        Some(RowKind::Package { pkg_idx }) => {
            format!("{} — overview", app.packages[*pkg_idx].name)
        }
        Some(RowKind::Namespace { pkg_idx, name }) => {
            format!("{} — namespace {name}", app.packages[*pkg_idx].name)
        }
        Some(
            RowKind::Group { pkg_idx, item_idx }
            | RowKind::Symbol { pkg_idx, item_idx }
            | RowKind::Member { pkg_idx, item_idx },
        ) => {
            let item = &app.packages[*pkg_idx].items[*item_idx];
            let view = if app.show_source {
                "source"
            } else {
                kind_label(&item.kind)
            };
            format!("{} [{}]", item_display_name(item), view)
        }
        None => "doc".to_string(),
    }
}

/// Render a window of source lines from the cache.
/// `scroll` and `viewport_h` are in "virtual rows" where row 0 = header, row 1 = blank,
/// rows 2.. = code lines.
fn source_lines_windowed(
    cached_lines: &[String],
    decl_line: usize,
    lang: &DocLanguage,
    file_label: &str,
    scroll: usize,
    viewport_h: usize,
) -> Vec<Line<'static>> {
    let fence = lang_fence(lang);
    let mut out = Vec::new();
    for vrow in scroll..scroll + viewport_h {
        match vrow {
            0 => out.push(Line::styled(
                format!(" {file_label} : line {}", decl_line + 1),
                Style::default().fg(COLOR_HINT),
            )),
            1 => out.push(Line::raw("")),
            n => {
                let abs = n - 2; // 0-indexed source line
                if abs >= cached_lines.len() {
                    break;
                }
                let line = &cached_lines[abs];
                let is_decl = abs == decl_line;
                let num_style = if is_decl {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let marker = if is_decl { "▶ " } else { "  " };
                let mut spans = vec![
                    Span::styled(format!("{:>5} ", abs + 1), num_style),
                    Span::styled(marker.to_string(), Style::default().fg(Color::Yellow)),
                ];
                let md = format!("```{fence}\n{line}\n```\n");
                let rendered = tui_markdown::from_str(&md);
                let code_spans: Vec<Span<'static>> = rendered
                    .lines
                    .into_iter()
                    .find(|l| {
                        let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                        !t.trim().starts_with("```")
                    })
                    .map(|l| {
                        l.spans
                            .into_iter()
                            .map(|s| Span::styled(s.content.into_owned(), s.style))
                            .collect()
                    })
                    .unwrap_or_else(|| vec![Span::raw(line.clone())]);
                spans.extend(code_spans);
                out.push(Line::from(spans));
            }
        }
    }
    out
}

fn detail_lines(app: &App) -> Vec<Line<'static>> {
    let width = app.detail_area.width.saturating_sub(4) as usize;
    let width = width.max(20);
    // Hidden stdlib item shown via link click.
    if let Some((pi, ii)) = app.external_item {
        if let Some(pkg) = app.hidden.get(pi) {
            if let Some(item) = pkg.items.get(ii) {
                return symbol_lines(item, &pkg.items, width);
            }
        }
    }
    match app.selected_row().map(|r| r.kind.clone()) {
        Some(RowKind::Package { pkg_idx }) => pkg_overview_lines(&app.packages[pkg_idx], width),
        Some(RowKind::Namespace { pkg_idx, name }) => {
            namespace_lines(&app.packages[pkg_idx], &name, width)
        }
        Some(
            RowKind::Group { pkg_idx, item_idx }
            | RowKind::Symbol { pkg_idx, item_idx }
            | RowKind::Member { pkg_idx, item_idx },
        ) => symbol_lines(
            &app.packages[pkg_idx].items[item_idx],
            &app.packages[pkg_idx].items,
            width,
        ),
        None => vec![Line::raw("")],
    }
}

// ── Detail content renderers ──────────────────────────────────────────────────

fn namespace_lines(pkg: &PackageDoc, ns: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    out.push(Line::from(vec![
        Span::styled("namespace ".to_string(), Style::default().fg(COLOR_HINT)),
        Span::styled(
            ns.to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    out.push(Line::from(Span::styled(
        "─".repeat(width.min(60)),
        Style::default().fg(COLOR_BORDER),
    )));
    out.push(Line::raw(""));

    let members = items_in_ns(&pkg.items, ns);
    if members.is_empty() {
        out.push(Line::styled(
            "No documented members.".to_string(),
            Style::default().fg(COLOR_HINT),
        ));
        return out;
    }

    // Group by section within the namespace.
    for sec in Section::ALL {
        let group: Vec<_> = members
            .iter()
            .filter(|i| item_section(i) == Some(sec))
            .collect();
        if group.is_empty() {
            continue;
        }
        out.push(Line::styled(
            sec.label().to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        for item in group {
            let kl = kind_label(&item.kind);
            let simple = item_display_name(item);
            out.push(Line::from(vec![
                Span::styled(
                    format!("  {simple}"),
                    Style::default()
                        .fg(COLOR_SYMBOL)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  [{kl}]"), Style::default().fg(COLOR_KIND)),
            ]));
            if !item.brief.is_empty() {
                let brief = render_math_lines(&item.brief);
                for line in word_wrap(&brief, width.saturating_sub(6)) {
                    out.push(Line::styled(
                        format!("    {line}"),
                        Style::default().fg(COLOR_BRIEF),
                    ));
                }
            }
        }
        out.push(Line::raw(""));
    }
    out
}

fn pkg_overview_lines(pkg: &PackageDoc, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();

    out.push(Line::from(vec![
        Span::styled(
            pkg.name.clone(),
            Style::default().fg(COLOR_PKG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", pkg.version),
            Style::default().fg(COLOR_HINT),
        ),
    ]));
    out.push(Line::raw(""));

    // Section counts table
    for sec in Section::ALL {
        let count = pkg
            .items
            .iter()
            .filter(|i| item_section(i) == Some(sec))
            .count();
        if count > 0 {
            out.push(Line::from(vec![
                Span::styled(
                    format!("  {:.<24}", sec.label()),
                    Style::default().fg(COLOR_SECTION),
                ),
                Span::styled(format!(" {count}"), Style::default().fg(COLOR_SYMBOL)),
            ]));
        }
    }
    out.push(Line::raw(""));

    if let Some(readme) = &pkg.readme {
        out.push(Line::styled(
            "README".to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        out.push(Line::from(Span::styled(
            "─".repeat(width.min(60)),
            Style::default().fg(COLOR_BORDER),
        )));
        out.push(Line::raw(""));
        push_markdown_body(&mut out, readme);
    }
    out
}

fn symbol_lines(item: &DocItem, all_items: &[DocItem], width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();

    // Name + kind badge
    out.push(Line::from(vec![
        Span::styled(
            item_display_name(item),
            Style::default()
                .fg(COLOR_SYMBOL)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  [{}]", kind_label(&item.kind)),
            Style::default().fg(COLOR_KIND),
        ),
    ]));

    // Signature with syntax highlighting
    if !item.signature.is_empty() {
        out.push(Line::raw(""));
        push_highlighted_code(&mut out, &make_prototype(&item.signature), &item.lang);
    }

    out.push(Line::from(Span::styled(
        "─".repeat(width.min(60)),
        Style::default().fg(COLOR_BORDER),
    )));

    // Brief (always single line — plain text is fine)
    if !item.brief.is_empty() {
        let brief = render_math_lines(&item.brief);
        for line in word_wrap(&brief, width) {
            out.push(Line::styled(line, Style::default().fg(Color::White)));
        }
        out.push(Line::raw(""));
    }

    // Body — full markdown: tables, bold, lists, etc.
    if !item.body.is_empty() {
        push_markdown_body(&mut out, &item.body);
        out.push(Line::raw(""));
    }

    // Params / returns / other tags
    let params: Vec<_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Param)
        .collect();
    let returns: Vec<_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::Return)
        .collect();
    let see: Vec<_> = item
        .tags
        .iter()
        .filter(|t| t.kind == TagKind::See)
        .collect();

    if !params.is_empty() {
        out.push(Line::styled(
            "Parameters".to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        for p in &params {
            let name = p.name.as_deref().unwrap_or("?");
            let desc = render_math_lines(&p.text);
            out.push(Line::from(vec![
                Span::styled(format!("  {name:<16}"), Style::default().fg(COLOR_SYMBOL)),
                Span::styled(desc, Style::default().fg(COLOR_BRIEF)),
            ]));
        }
        out.push(Line::raw(""));
    }

    if !returns.is_empty() {
        out.push(Line::styled(
            "Returns".to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        for r in &returns {
            push_markdown_body(&mut out, &r.text);
        }
        out.push(Line::raw(""));
    }

    if !see.is_empty() {
        out.push(Line::styled(
            "See also".to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        let combined = see
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        push_markdown_body(&mut out, &combined);
        out.push(Line::raw(""));
    }

    // Member table for class / struct / interface items.
    if matches!(
        item.kind,
        DocKind::Class | DocKind::Struct | DocKind::Interface | DocKind::Typedef
    ) {
        let simple = item_display_name(item);
        let members = class_members(all_items, &simple);
        if !members.is_empty() {
            render_member_table(&mut out, &members, width);
        }
    }

    out
}

fn lang_fence(lang: &DocLanguage) -> &'static str {
    // Only languages whose syntax is bundled in syntect's default set get a tag.
    // Unsupported ones return "" so the code block falls back to generic styling
    // rather than syntect trying and failing to find the syntax definition.
    match lang {
        DocLanguage::C => "c",
        DocLanguage::Cpp => "cpp",
        DocLanguage::Rust => "rust",
        DocLanguage::D => "d",
        // No bundled syntect syntax — use generic code block.
        DocLanguage::Fortran | DocLanguage::Ada | DocLanguage::Zig | DocLanguage::Unknown => "",
    }
}

/// Convert a raw signature into a one-line prototype declaration.
/// Strips bodies/modifiers that aren't part of the declaration and adds `;`.
fn make_prototype(sig: &str) -> String {
    // Take everything up to the first `{` (body start) or `;` (already a decl).
    let decl = sig.split('{').next().unwrap_or(sig).trim();
    let decl = decl.trim_end_matches(';').trim();
    format!("{decl};")
}

/// Push syntax-highlighted code lines into `out` using tui_markdown + syntect.
/// Fence marker lines that tui_markdown emits as decoration are dropped.
fn push_highlighted_code(out: &mut Vec<Line<'static>>, code: &str, lang: &DocLanguage) {
    let fence = lang_fence(lang);
    let md = format!("```{fence}\n{code}\n```\n");
    let text = tui_markdown::from_str(&md);
    for line in text.lines {
        // Collect the raw text of the line to detect fence markers.
        let raw: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let trimmed = raw.trim();
        // Skip lines that are purely fence markers (` ``` ` with optional lang tag).
        if trimmed.starts_with("```") {
            continue;
        }
        out.push(Line::from(
            line.spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style))
                .collect::<Vec<_>>(),
        ));
    }
}

fn render_member_table(out: &mut Vec<Line<'static>>, members: &[&DocItem], width: usize) {
    use crate::doc::Access;

    // Group by access specifier bucket.
    let buckets: &[(&str, Option<Access>)] = &[
        ("Public Functions", Some(Access::Public)),
        ("Protected Functions", Some(Access::Protected)),
        ("Public Variables", None), // variables without explicit access
        ("Private Members", Some(Access::Private)),
    ];

    // Separate functions/subroutines from variables.
    let fns: Vec<&&DocItem> = members
        .iter()
        .filter(|m| matches!(m.kind, DocKind::Function | DocKind::Subroutine))
        .collect();
    let vars: Vec<&&DocItem> = members
        .iter()
        .filter(|m| !matches!(m.kind, DocKind::Function | DocKind::Subroutine))
        .collect();

    let mut emitted = false;

    // Public and protected functions.
    for (label, access) in [
        ("Public Functions", Some(Access::Public)),
        ("Protected Functions", Some(Access::Protected)),
        ("Private Functions", Some(Access::Private)),
    ] {
        let group: Vec<_> = fns
            .iter()
            .filter(|m| {
                m.meta.access == access
                    || (access == Some(Access::Public) && m.meta.access.is_none())
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        if !emitted {
            out.push(Line::from(Span::styled(
                "─".repeat(width.min(60)),
                Style::default().fg(COLOR_BORDER),
            )));
        }
        emitted = true;
        out.push(Line::styled(
            label.to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        for m in group {
            render_member_row(out, m, width);
        }
        out.push(Line::raw(""));
    }

    // Variables / fields.
    if !vars.is_empty() {
        if !emitted {
            out.push(Line::from(Span::styled(
                "─".repeat(width.min(60)),
                Style::default().fg(COLOR_BORDER),
            )));
        }
        out.push(Line::styled(
            "Data Members".to_string(),
            Style::default()
                .fg(COLOR_SECTION)
                .add_modifier(Modifier::BOLD),
        ));
        for m in &vars {
            render_member_row(out, m, width);
        }
        out.push(Line::raw(""));
    }
    let _ = buckets; // suppress unused warning
}

fn render_member_row(out: &mut Vec<Line<'static>>, m: &DocItem, width: usize) {
    let simple = item_display_name(m);
    let kl = kind_label(&m.kind);
    let attrs = m.meta.attrs.join(" ");
    let sig = if !m.signature.is_empty() {
        make_prototype(&m.signature)
    } else if !attrs.is_empty() {
        format!("{attrs} {};", simple)
    } else {
        format!("{};", simple)
    };
    // Indent + kind badge header
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("[{kl}]"), Style::default().fg(COLOR_KIND)),
    ]));
    // Highlighted signature indented under the badge
    let base_len = out.len();
    push_highlighted_code(out, &sig, &m.lang);
    // Indent the code lines that were appended
    for line in out.iter_mut().skip(base_len) {
        line.spans.insert(0, Span::raw("    "));
    }
    if !m.brief.is_empty() {
        let brief = render_math_lines(&m.brief);
        for line in word_wrap(&brief, width.saturating_sub(6)) {
            out.push(Line::styled(
                format!("    {line}"),
                Style::default().fg(COLOR_BRIEF),
            ));
        }
    }
}

// ── Markdown table rendering (ported from docify) ─────────────────────────────

#[derive(Debug, Clone)]
struct MarkdownTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn parse_markdown_table(lines: &[&str], start: usize) -> Option<(MarkdownTable, usize)> {
    let header = split_markdown_table_row(*lines.get(start)?)?;
    let sep = split_markdown_table_row(*lines.get(start + 1)?)?;
    if header.is_empty() || !sep.iter().all(|c| is_markdown_separator_cell(c)) {
        return None;
    }
    let mut rows = Vec::new();
    let mut i = start + 2;
    while let Some(line) = lines.get(i) {
        let Some(cells) = split_markdown_table_row(line) else {
            break;
        };
        if cells.is_empty() {
            break;
        }
        rows.push(cells);
        i += 1;
    }
    Some((
        MarkdownTable {
            headers: header,
            rows,
        },
        i - start,
    ))
}

fn split_markdown_table_row(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.starts_with('|') || !t.ends_with('|') || t.matches('|').count() < 2 {
        return None;
    }
    Some(
        t.trim_matches('|')
            .split('|')
            .map(|c| clean_markdown_table_cell(c.trim()))
            .collect(),
    )
}

fn is_markdown_separator_cell(cell: &str) -> bool {
    let t = cell.trim_matches(':').trim();
    !t.is_empty() && t.chars().all(|c| c == '-')
}

fn clean_markdown_table_cell(cell: &str) -> String {
    cell.replace('`', "")
        .replace("**", "")
        .replace('*', "")
        .trim()
        .to_string()
}

fn push_markdown_table(out: &mut Vec<Line<'static>>, table: &MarkdownTable, width: usize) {
    if table.headers.len() != 2 {
        // Fallback: plain text
        let header = table.headers.join("  ");
        for w in word_wrap(&header, width) {
            out.push(Line::styled(
                w,
                Style::default()
                    .fg(COLOR_SECTION)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        for row in &table.rows {
            for w in word_wrap(&row.join("  "), width) {
                out.push(Line::styled(w, Style::default().fg(COLOR_BRIEF)));
            }
        }
        return;
    }

    let name_w = table
        .rows
        .iter()
        .filter_map(|r| r.first())
        .map(|c| c.chars().count())
        .chain(table.headers.first().map(|c| c.chars().count()))
        .max()
        .unwrap_or(8)
        .clamp(4, 20);
    let avail = width.saturating_sub(name_w + 8).max(16);
    let desc_content = table
        .rows
        .iter()
        .filter_map(|r| r.get(1))
        .map(|c| c.chars().count())
        .chain(table.headers.get(1).map(|c| c.chars().count()))
        .max()
        .unwrap_or(16);
    let desc_w = desc_content.clamp(16, avail.min(56));

    out.push(table_rule(name_w, desc_w, "┌", "┬", "┐"));
    out.push(table_row(
        table.headers.first().map(String::as_str).unwrap_or(""),
        table.headers.get(1).map(String::as_str).unwrap_or(""),
        name_w,
        desc_w,
        true,
        true,
    ));
    out.push(table_rule(name_w, desc_w, "├", "┼", "┤"));
    for row in &table.rows {
        push_wrapped_table_row(
            out,
            row.first().map(String::as_str).unwrap_or(""),
            row.get(1).map(String::as_str).unwrap_or(""),
            name_w,
            desc_w,
        );
    }
    out.push(table_rule(name_w, desc_w, "└", "┴", "┘"));
}

fn push_wrapped_table_row(
    out: &mut Vec<Line<'static>>,
    name: &str,
    text: &str,
    name_w: usize,
    desc_w: usize,
) {
    let wrapped = word_wrap(text, desc_w);
    if wrapped.is_empty() {
        out.push(table_row(name, "", name_w, desc_w, false, true));
        return;
    }
    for (idx, desc) in wrapped.iter().enumerate() {
        let label = if idx == 0 { name } else { "" };
        out.push(table_row(label, desc, name_w, desc_w, false, idx == 0));
    }
}

fn table_row(
    name: &str,
    text: &str,
    name_w: usize,
    desc_w: usize,
    header: bool,
    show_label: bool,
) -> Line<'static> {
    let name_style = if header {
        Style::default()
            .fg(COLOR_SECTION)
            .add_modifier(Modifier::BOLD)
    } else if show_label {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let text_style = if header {
        Style::default()
            .fg(COLOR_SECTION)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_BRIEF)
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled("│ ".to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{name:<name_w$}"), name_style),
        Span::styled(" │ ".to_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{text:<desc_w$}"), text_style),
        Span::styled(" │".to_string(), Style::default().fg(Color::DarkGray)),
    ])
}

fn table_rule(
    name_w: usize,
    desc_w: usize,
    l: &'static str,
    m: &'static str,
    r: &'static str,
) -> Line<'static> {
    Line::styled(
        format!(
            "  {l}{}{m}{}{r}",
            "─".repeat(name_w + 2),
            "─".repeat(desc_w + 2)
        ),
        Style::default().fg(Color::DarkGray),
    )
}

/// Render markdown body text — tables with box-drawing, everything else via tui_markdown.
/// LaTeX is pre-processed to Unicode before rendering.
// ── One Dark Pro markdown stylesheet ─────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
struct OneDarkPro;

impl tui_markdown::StyleSheet for OneDarkPro {
    fn heading(&self, level: u8) -> ratatui::style::Style {
        use ratatui::style::{Color, Modifier, Style};
        match level {
            // Yellow — brightest, most prominent
            1 => Style::default()
                .fg(Color::Rgb(229, 192, 123))
                .add_modifier(Modifier::BOLD),
            // Green
            2 => Style::default()
                .fg(Color::Rgb(152, 195, 121))
                .add_modifier(Modifier::BOLD),
            // Blue
            3 => Style::default()
                .fg(Color::Rgb(97, 175, 239))
                .add_modifier(Modifier::BOLD),
            // Purple
            4 => Style::default()
                .fg(Color::Rgb(198, 120, 221))
                .add_modifier(Modifier::BOLD),
            // Cyan
            5 => Style::default()
                .fg(Color::Rgb(86, 182, 194))
                .add_modifier(Modifier::ITALIC),
            // Red/orange — lowest level
            _ => Style::default()
                .fg(Color::Rgb(224, 108, 117))
                .add_modifier(Modifier::ITALIC),
        }
    }
    fn code(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(ratatui::style::Color::Rgb(152, 195, 121))
    }
    fn link(&self) -> ratatui::style::Style {
        ratatui::style::Style::default()
            .fg(ratatui::style::Color::Rgb(97, 175, 239))
            .add_modifier(ratatui::style::Modifier::UNDERLINED)
    }
    fn blockquote(&self) -> ratatui::style::Style {
        ratatui::style::Style::default()
            .fg(ratatui::style::Color::Rgb(92, 99, 112))
            .add_modifier(ratatui::style::Modifier::ITALIC)
    }
    fn heading_meta(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::DIM)
    }
    fn metadata_block(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(ratatui::style::Color::Rgb(92, 99, 112))
    }
}

/// Parse a heading line — returns `(level, text)` for `# …` through `#### …`.
fn parse_heading_line(line: &str) -> Option<(u8, &str)> {
    for (prefix, level) in [("#### ", 4u8), ("### ", 3), ("## ", 2), ("# ", 1)] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((level, rest));
        }
    }
    None
}

/// Build a styled heading `Line` with half-circle bookends and a per-level background.
fn styled_heading_line(level: u8, text: &str) -> Line<'static> {
    let (fg, bg) = match level {
        1 => (Color::Rgb(229, 192, 123), Color::Rgb(50, 44, 18)),
        2 => (Color::Rgb(152, 195, 121), Color::Rgb(24, 44, 20)),
        3 => (Color::Rgb(97, 175, 239), Color::Rgb(18, 33, 50)),
        _ => (Color::Rgb(198, 120, 221), Color::Rgb(34, 18, 44)),
    };
    let cap_style = Style::default().fg(bg);
    let body_style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled("\u{E0B6}", cap_style),
        Span::styled(format!(" {text} "), body_style),
        Span::styled("\u{E0B4}", cap_style),
    ])
}

/// Render a fenced code block as a box with the language tag in the top border.
///
/// ```text
/// ┌─── rust ───────────────────────────────────────┐
/// │  fn main() { println!("hello"); }              │
/// └────────────────────────────────────────────────┘
/// ```
fn push_fenced_code(out: &mut Vec<Line<'static>>, fence_lang: &str, code: &str, width: usize) {
    let border = Style::default().fg(COLOR_BORDER);
    let w = width.max(10);

    // Top border: ┌─── lang ───…───┐
    let lang_part = if fence_lang.is_empty() {
        String::new()
    } else {
        format!(" {} ", fence_lang)
    };
    let left_dashes = 3usize;
    let right_dashes = w.saturating_sub(left_dashes + lang_part.chars().count() + 2);
    out.push(Line::styled(
        format!(
            "┌{}{}{}┐",
            "─".repeat(left_dashes),
            lang_part,
            "─".repeat(right_dashes)
        ),
        border,
    ));

    // Code lines — syntax highlighted, padded to width and wrapped in │ … │
    let inner_w = w.saturating_sub(4); // "│ " + content + " │"
    let md_src = format!("```{fence_lang}\n{code}\n```\n");
    let rendered = tui_markdown::from_str(&md_src);
    for line in rendered.lines {
        let raw: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        if raw.trim().starts_with("```") {
            continue;
        }
        let text_len: usize = raw.chars().count();
        let padding = " ".repeat(inner_w.saturating_sub(text_len));
        let mut spans: Vec<Span<'static>> = vec![Span::styled("│ ", border)];
        spans.extend(
            line.spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style)),
        );
        spans.push(Span::raw(padding));
        spans.push(Span::styled(" │", border));
        out.push(Line::from(spans));
    }

    // Bottom border: └───…───┘
    out.push(Line::styled(
        format!("└{}┘", "─".repeat(w.saturating_sub(2))),
        border,
    ));
    out.push(Line::raw(""));
}

fn push_markdown_body(out: &mut Vec<Line<'static>>, text: &str) {
    let math_processed = render_math_lines(text);
    let raw: Vec<&str> = math_processed.lines().collect();
    let opts = tui_markdown::Options::new(OneDarkPro);
    let mut i = 0;
    while i < raw.len() {
        // Heading line — render with half-circle decoration and background color.
        if let Some((level, heading_text)) = parse_heading_line(raw[i]) {
            out.push(Line::raw(""));
            out.push(styled_heading_line(level, heading_text));
            out.push(Line::raw(""));
            i += 1;
            continue;
        }
        // Fenced code block — render as a boxed block with language in the top border.
        if raw[i].starts_with("```") {
            let fence_lang = raw[i].trim_start_matches('`').trim();
            let code_start = i + 1;
            let mut code_end = code_start;
            while code_end < raw.len() && !raw[code_end].starts_with("```") {
                code_end += 1;
            }
            let code = raw[code_start..code_end].join("\n");
            push_fenced_code(out, fence_lang, &code, 72);
            i = code_end + 1;
            continue;
        }
        if let Some((table, consumed)) = parse_markdown_table(&raw, i) {
            push_markdown_table(out, &table, 72);
            i += consumed;
            continue;
        }
        // Accumulate non-heading, non-table, non-fence lines for tui_markdown.
        let start = i;
        while i < raw.len()
            && parse_markdown_table(&raw, i).is_none()
            && parse_heading_line(raw[i]).is_none()
            && !raw[i].starts_with("```")
        {
            i += 1;
        }
        let block = raw[start..i].join("\n");
        let md = tui_markdown::from_str_with_options(&block, &opts);
        out.extend(md.lines.into_iter().map(|l| {
            Line::from(
                l.spans
                    .into_iter()
                    .map(|s| Span::styled(s.content.into_owned(), s.style))
                    .collect::<Vec<_>>(),
            )
        }));
    }
}

// ── Text helpers ──────────────────────────────────────────────────────────────

/// Strip C-style keyword prefixes from type names.
///
/// Docify's C extractor sometimes stores the full specifier as the name:
/// `"typedef struct vec2"` → `"vec2"`, `"struct Point"` → `"Point"`.
fn clean_item_name(name: &str) -> &str {
    for prefix in [
        "typedef struct ",
        "typedef enum ",
        "typedef union ",
        "struct ",
        "enum ",
        "union ",
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    name
}

/// The simple (unqualified) display name for an item, with keyword prefixes stripped.
fn item_display_name(item: &DocItem) -> String {
    let base = clean_item_name(&item.name);
    // Take only the leaf name after :: or .
    base.rsplit("::")
        .next()
        .or_else(|| base.rsplit('.').next())
        .unwrap_or(base)
        .to_string()
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in raw_line.split_whitespace() {
            if line.is_empty() {
                line.push_str(word);
            } else if line.chars().count() + 1 + word.chars().count() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(line.clone());
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

// ── Event loop ────────────────────────────────────────────────────────────────

pub fn browse(
    packages: Vec<PackageDoc>,
    stdlib_rx: std::sync::mpsc::Receiver<StdlibMsg>,
) -> anyhow::Result<()> {
    let hidden = Vec::new();
    if packages.is_empty() {
        return Err(anyhow::anyhow!("no documented packages to display"));
    }

    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(packages, hidden);
    let result = run_loop(&mut terminal, &mut app, stdlib_rx);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    app: &mut App,
    stdlib_rx: std::sync::mpsc::Receiver<StdlibMsg>,
) -> anyhow::Result<()> {
    loop {
        // Drain all pending stdlib messages (non-blocking).
        loop {
            match stdlib_rx.try_recv() {
                Ok(StdlibMsg::Progress { done, total, label }) => {
                    app.stdlib_status = Some((done, total, label));
                }
                Ok(StdlibMsg::Done(hidden)) => {
                    app.hidden = hidden;
                    app.sym_index = build_sym_index(&app.packages, &app.hidden);
                    app.stdlib_status = None;
                }
                Err(_) => break,
            }
        }

        terminal.draw(|f| render(app, f))?;

        match event::read()? {
            Event::Key(key) => {
                // Quit only when not in filter mode.
                if app.focus != Focus::Filter
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    && key.modifiers == KeyModifiers::NONE
                {
                    break;
                }

                match app.focus {
                    Focus::Filter => match key.code {
                        KeyCode::Esc => {
                            app.query.clear();
                            app.focus = Focus::Tree;
                            app.rebuild_rows();
                            if !app.rows.is_empty() {
                                app.list_state.select(Some(0));
                            }
                        }
                        KeyCode::Enter | KeyCode::Right => {
                            app.focus = Focus::Tree;
                        }
                        KeyCode::Backspace => {
                            app.query.pop();
                            app.rebuild_rows();
                            if !app.rows.is_empty() {
                                app.list_state.select(Some(0));
                            }
                        }
                        KeyCode::Char(c)
                            if key.modifiers == KeyModifiers::NONE
                                || key.modifiers == KeyModifiers::SHIFT =>
                        {
                            app.query.push(c);
                            app.rebuild_rows();
                            if !app.rows.is_empty() {
                                app.list_state.select(Some(0));
                            }
                        }
                        _ => {}
                    },
                    Focus::Tree => match key.code {
                        KeyCode::Char('/') => {
                            app.focus = Focus::Filter;
                        }
                        KeyCode::Right => {
                            app.focus = Focus::Detail;
                            app.show_source = false;
                        }
                        KeyCode::Left => {
                            app.focus = Focus::Filter;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.move_sel(-1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.move_sel(1);
                        }
                        KeyCode::PageUp => {
                            app.move_sel(-10);
                        }
                        KeyCode::PageDown => {
                            app.move_sel(10);
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            app.scroll = 0;
                            app.toggle_selected();
                        }
                        _ => {}
                    },
                    Focus::Detail => match key.code {
                        KeyCode::Left => {
                            app.focus = Focus::Tree;
                        }
                        // Tab toggles source ↔ docs view.
                        KeyCode::Tab => {
                            app.show_source = !app.show_source;
                            app.scroll = 0;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.scroll_detail(-1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.scroll_detail(1);
                        }
                        KeyCode::PageUp => {
                            app.scroll_detail(-10);
                        }
                        KeyCode::PageDown => {
                            app.scroll_detail(10);
                        }
                        _ => {}
                    },
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if App::contains(app.tree_area, m.column, m.row) {
                        app.focus = Focus::Tree;
                        let inner_y = app.tree_area.y + 1; // inside border
                        if m.row >= inner_y {
                            let clicked = (m.row - inner_y) as usize;
                            let offset = app.list_state.offset();
                            let idx = offset + clicked;
                            if idx < app.rows.len() {
                                if app.list_state.selected() == Some(idx) {
                                    app.scroll = 0;
                                    app.toggle_selected();
                                } else {
                                    app.list_state.select(Some(idx));
                                    app.external_item = None;
                                    app.scroll = 0;
                                }
                            }
                        }
                    } else if App::contains(app.filter_area, m.column, m.row) {
                        app.focus = Focus::Filter;
                    } else if App::contains(app.detail_area, m.column, m.row) {
                        app.focus = Focus::Detail;
                        app.activate_detail_link(m.column, m.row);
                    }
                }
                MouseEventKind::ScrollUp => {
                    if App::contains(app.tree_area, m.column, m.row) {
                        app.move_sel(-3);
                    } else {
                        app.scroll_detail(-3);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if App::contains(app.tree_area, m.column, m.row) {
                        app.move_sel(3);
                    } else {
                        app.scroll_detail(3);
                    }
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
    Ok(())
}
