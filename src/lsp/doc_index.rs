//! Symbol documentation index built from the project's source files via docify.
//!
//! The index is rebuilt whenever `freight.toml` is saved (same cadence as
//! `compile_commands.json`). Hover requests for source files look up the symbol
//! at the cursor position and return formatted Markdown documentation before
//! falling back to the passthrough language server (clangd/fortls/asm-lsp).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::doc::{extract_dir, extract_file, DocItem, DocKind, DocLanguage, TagKind};
use crate::manifest::load_manifest;
use crate::manifest::types::Manifest;


// ---------------------------------------------------------------------------
// DocIndex
// ---------------------------------------------------------------------------

/// Symbol documentation index built from docified sources.
///
/// Supports two lookup strategies:
/// - by name (case-insensitive, for word-under-cursor fallback)
/// - by file + line (for precise position-based hover)
pub struct DocIndex {
    items: Vec<DocItem>,
    /// Lower-case symbol name → index into `items`.
    by_name: HashMap<String, usize>,
    /// Canonical file path → (doc-comment start line → index into `items`).
    by_location: HashMap<PathBuf, BTreeMap<usize, usize>>,
}

impl DocIndex {
    pub fn build_freight_packages<'a>(package_dirs: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut items: Vec<DocItem> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut by_location: HashMap<PathBuf, BTreeMap<usize, usize>> = HashMap::new();
        for package_dir in package_dirs {
            let manifest = load_manifest(package_dir).ok();
            for item in extract_pkg_items(package_dir, manifest.as_ref()) {
                if item.brief.is_empty() && item.body.is_empty() {
                    continue;
                }
                let idx = items.len();
                let key = simple_name(&item.name).to_ascii_lowercase();
                // name map: first occurrence wins
                by_name.entry(key).or_insert(idx);
                // location map: keyed by canonical file path + doc-comment start line
                if item.line > 0 {
                    let canonical = item.file.canonicalize().unwrap_or_else(|_| item.file.clone());
                    by_location.entry(canonical).or_default().insert(item.line, idx);
                }
                items.push(item);
            }
        }
        Self { items, by_name, by_location }
    }

    pub fn lookup(&self, name: &str) -> Option<&DocItem> {
        self.by_name.get(&name.to_ascii_lowercase()).and_then(|&i| self.items.get(i))
    }

    /// Find the doc item whose doc-comment is nearest at or before `line` in `file`.
    pub fn lookup_by_location(&self, file: &Path, line: usize) -> Option<&DocItem> {
        let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let tree = self.by_location.get(&canonical)?;
        // Walk backwards from line to find the nearest item at or before the cursor.
        let (_, &idx) = tree.range(..=line + 5).next_back()?;
        self.items.get(idx)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Extract doc items from one Freight package, following `freight doc` TUI rules:
/// prefer public `[lib].hdrs`, then `[lib].srcs`, then `src/`, then package root.
fn extract_pkg_items(dir: &Path, manifest: Option<&Manifest>) -> Vec<DocItem> {
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

    let src_dirs: Vec<PathBuf> = manifest
        .and_then(|m| m.lib.as_ref())
        .map(|lib| {
            lib.srcs
                .iter()
                .map(|s| {
                    let path = dir.join(s);
                    if path.is_dir() {
                        path
                    } else {
                        path.parent()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| dir.to_path_buf())
                    }
                })
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();

    if !src_dirs.is_empty() {
        let mut items = Vec::new();
        for src_dir in &src_dirs {
            items.extend(extract_dir(src_dir).items);
        }
        return items;
    }

    // Scan all conventional source/header directories, deduplicating.
    let candidates = ["src", "include", "inc"];
    let mut items = Vec::new();
    let mut any = false;
    for name in candidates {
        let d = dir.join(name);
        if d.is_dir() {
            items.extend(extract_dir(&d).items);
            any = true;
        }
    }
    if any { items } else { extract_dir(dir).items }
}

// ---------------------------------------------------------------------------
// HeaderIndex
// ---------------------------------------------------------------------------

/// Where a package's headers came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderOrigin {
    /// Workspace member (sibling crate in the same `freight.toml` workspace).
    Workspace,
    /// Path dependency declared in the current project's `[dependencies]`.
    Project,
    /// Installed by `freight fetch` into the project's `.pkgs/` cache.
    Local,
    /// System-installed (found on the compiler's default include path).
    System,
}

pub struct HeaderEntry {
    pub package_name: String,
    pub package_version: Option<String>,
    pub full_path: PathBuf,
    pub origin: HeaderOrigin,
}

/// Maps include-path variants → package origin.
///
/// Both the basename (`zlib.h`) and the relative include path (`zlib/zlib.h`)
/// are indexed so lookups work for both `<zlib.h>` and `<zlib/zlib.h>`.
pub struct HeaderIndex {
    by_path: HashMap<String, HeaderEntry>,
    /// Compiler system include directories (probed once at build time).
    system_dirs: Vec<PathBuf>,
}

/// A package directory together with where it came from.
pub struct HeaderDirSpec<'a> {
    pub path: &'a Path,
    pub origin: HeaderOrigin,
}

impl HeaderIndex {
    /// Build the index from:
    /// - `package_dirs`: workspace members and path deps, tagged with their origin
    /// - `pkgs_dir`: the `.pkgs/` directory (freight-installed packages → `Local`)
    pub fn build(package_dirs: &[HeaderDirSpec<'_>], pkgs_dir: Option<&Path>) -> Self {
        let mut by_path = HashMap::new();

        for spec in package_dirs {
            let dir = spec.path;
            let manifest = load_manifest(dir).ok();
            let pkg_name = manifest
                .as_ref()
                .map(|m| m.package.name.clone())
                .unwrap_or_else(|| {
                    dir.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string()
                });
            let pkg_version = manifest.as_ref().map(|m| m.package.version.clone());

            // Declared public headers
            if let Some(hdrs) = manifest.as_ref().and_then(|m| m.lib.as_ref()).map(|l| &l.hdrs) {
                for hdr in hdrs {
                    let full = dir.join(hdr);
                    if full.is_file() {
                        let entry = HeaderEntry {
                            package_name: pkg_name.clone(),
                            package_version: pkg_version.clone(),
                            full_path: full,
                            origin: spec.origin.clone(),
                        };
                        insert_header(&mut by_path, hdr, entry);
                    }
                }
            }

            // include/ directory
            let include_dir = dir.join("include");
            if include_dir.is_dir() {
                walk_include_dir(
                    &include_dir, &include_dir,
                    &pkg_name, &pkg_version,
                    spec.origin.clone(), &mut by_path,
                );
            }
        }

        // Installed packages in .pkgs/
        if let Some(pkgs) = pkgs_dir.filter(|p| p.is_dir()) {
            for entry in std::fs::read_dir(pkgs).into_iter().flatten().flatten() {
                let pkg_dir = entry.path();
                if !pkg_dir.is_dir() {
                    continue;
                }
                let dir_name = pkg_dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                let (pkg_name, pkg_version) = split_name_version(&dir_name);
                let include_dir = pkg_dir.join("include");
                if include_dir.is_dir() {
                    walk_include_dir(
                        &include_dir, &include_dir,
                        pkg_name, &Some(pkg_version.to_string()),
                        HeaderOrigin::Local, &mut by_path,
                    );
                }
            }
        }

        let system_dirs = probe_system_include_dirs();
        Self { by_path, system_dirs }
    }

    /// Look up a header by its include path (e.g. `"zlib.h"` or `"zlib/zlib.h"`).
    pub fn lookup(&self, header: &str) -> Option<&HeaderEntry> {
        // Exact match first (covers `zlib/zlib.h` style)
        if let Some(e) = self.by_path.get(header) {
            return Some(e);
        }
        // Basename fallback
        let basename = Path::new(header).file_name()?.to_str()?;
        self.by_path.get(basename)
    }

    /// Look up a header in the compiler's system include directories.
    /// Returns a synthetic `System` entry if found, `None` otherwise.
    pub fn lookup_system(&self, header: &str) -> Option<HeaderEntry> {
        for dir in &self.system_dirs {
            let candidate = dir.join(header);
            if candidate.exists() {
                return Some(HeaderEntry {
                    package_name: "stdlib".to_string(),
                    package_version: None,
                    full_path: candidate,
                    origin: HeaderOrigin::System,
                });
            }
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

impl Default for HeaderIndex {
    fn default() -> Self {
        Self { by_path: HashMap::new(), system_dirs: probe_system_include_dirs() }
    }
}

/// Probe the default C++ compiler for its system include search paths.
/// Parses the `#include <...> search starts here:` block from `gcc -v` output.
fn probe_system_include_dirs() -> Vec<PathBuf> {
    let compilers = ["c++", "g++", "clang++", "cc", "gcc", "clang"];
    for compiler in compilers {
        if let Some(dirs) = try_probe_compiler(compiler) {
            if !dirs.is_empty() {
                return dirs;
            }
        }
    }
    Vec::new()
}

fn try_probe_compiler(compiler: &str) -> Option<Vec<PathBuf>> {
    let out = std::process::Command::new(compiler)
        .args(["-xc++", "-E", "-v", "/dev/null"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .ok()?;

    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut dirs = Vec::new();
    let mut in_block = false;

    for line in stderr.lines() {
        if line.contains("#include <...> search starts here") {
            in_block = true;
            continue;
        }
        if in_block {
            if line.starts_with("End of search list") {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                // Strip any trailing " (framework directory)" annotation (macOS clang)
                let path_str = trimmed
                    .split_once(" (")
                    .map(|(p, _)| p)
                    .unwrap_or(trimmed);
                let p = PathBuf::from(path_str);
                if p.is_dir() {
                    dirs.push(p);
                }
            }
        }
    }
    Some(dirs)
}

fn walk_include_dir(
    root: &Path,
    dir: &Path,
    pkg_name: &str,
    pkg_version: &Option<String>,
    origin: HeaderOrigin,
    out: &mut HashMap<String, HeaderEntry>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_include_dir(root, &path, pkg_name, pkg_version, origin.clone(), out);
        } else if is_header(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let entry = HeaderEntry {
                package_name: pkg_name.to_string(),
                package_version: pkg_version.clone(),
                full_path: path.clone(),
                origin: origin.clone(),
            };
            insert_header(out, &rel.to_string_lossy(), entry);
        }
    }
}

fn insert_header(map: &mut HashMap<String, HeaderEntry>, rel_path: &str, entry: HeaderEntry) {
    let normalized = rel_path.replace('\\', "/");
    let basename = Path::new(rel_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    map.entry(normalized).or_insert_with(|| HeaderEntry {
        package_name: entry.package_name.clone(),
        package_version: entry.package_version.clone(),
        full_path: entry.full_path.clone(),
        origin: entry.origin.clone(),
    });
    if !basename.is_empty() {
        map.entry(basename).or_insert(entry);
    }
}

fn is_header(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e, "h" | "hh" | "hpp" | "hxx" | "h++" | "cuh" | "inc"))
        .unwrap_or(false)
}

fn split_name_version(s: &str) -> (&str, &str) {
    // "zlib-1.3.2" → ("zlib", "1.3.2")
    if let Some(pos) = s.rfind('-') {
        let after = &s[pos + 1..];
        if after.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            return (&s[..pos], after);
        }
    }
    (s, "")
}

// ---------------------------------------------------------------------------
// Include hover rendering
// ---------------------------------------------------------------------------

pub fn include_hover_markdown(header: &str, entry: &HeaderEntry) -> String {
    // Build the "$origin/name-version" label
    let name_ver = if let Some(ref ver) = entry.package_version {
        if ver.is_empty() {
            entry.package_name.clone()
        } else {
            format!("{}-{ver}", entry.package_name)
        }
    } else {
        entry.package_name.clone()
    };

    let origin_label = match entry.origin {
        HeaderOrigin::Workspace => format!("$workspace/{name_ver}"),
        HeaderOrigin::Project   => format!("$project/{name_ver}"),
        HeaderOrigin::Local     => format!("$local/{name_ver}"),
        HeaderOrigin::System    => "$system".to_string(),
    };

    let mut out = String::new();
    out.push_str(&format!("**`{header}`**  `{origin_label}`\n\n"));
    let path_str = entry.full_path.to_string_lossy();
    out.push_str(&format!("`{path_str}`"));
    out
}

/// Parse an `#include` or `#import` directive from a line.
/// Returns `(header_path, is_system)` where `is_system` is true for `<…>` includes.
pub fn parse_include_header(line: &str) -> Option<(String, bool)> {
    let line = line.trim();
    let rest = line
        .strip_prefix("#include")
        .or_else(|| line.strip_prefix("#import"))
        .or_else(|| line.strip_prefix("import"))?
        .trim();
    if rest.starts_with('<') {
        let header = rest.strip_prefix('<')?.split('>').next()?.to_string();
        Some((header, true))
    } else if rest.starts_with('"') {
        let header = rest.strip_prefix('"')?.split('"').next()?.to_string();
        Some((header, false))
    } else {
        None
    }
}
// ---------------------------------------------------------------------------
// Markdown rendering 
// ---------------------------------------------------------------------------

/// Short label for an inlay hint: `← pkg-version` or `← stdlib`.
pub fn include_inlay_label(entry: &HeaderEntry) -> String {
    let label = match entry.origin {
        HeaderOrigin::System => "stdlib".to_string(),
        _ => {
            if let Some(ref ver) = entry.package_version {
                if ver.is_empty() {
                    entry.package_name.clone()
                } else {
                    format!("{}-{ver}", entry.package_name)
                }
            } else {
                entry.package_name.clone()
            }
        }
    };
    format!("← {label}")
}

/// Render a `DocItem` to a Markdown string suitable for an LSP hover response.
pub fn item_to_markdown(item: &DocItem) -> String {
    let mut out = String::new();

    // Symbol name heading with kind label and optional parent class.
    let display_name = simple_name(&item.name);
    if !display_name.is_empty() {
        let kind_label = item.kind.label();
        if let Some(parent) = item.meta.parent.as_deref().filter(|p| !p.is_empty()) {
            let parent_simple = simple_name(parent);
            out.push_str(&format!("### `{display_name}`  `{kind_label} in {parent_simple}`\n\n"));
        } else if !matches!(item.kind, DocKind::Unknown) {
            out.push_str(&format!("### `{display_name}`  `{kind_label}`\n\n"));
        } else {
            out.push_str(&format!("### `{display_name}`\n\n"));
        }
    }

    // Fenced code block for the signature.
    if !item.signature.is_empty() {
        let lang = lang_id(item.lang.clone());
        out.push_str(&format!("```{lang}\n{}\n```\n\n", item.signature.trim()));
    }

    // Brief (first paragraph of doc comment).
    if !item.brief.is_empty() {
        out.push_str(item.brief.trim());
        out.push_str("\n\n");
    }

    // Extended body.
    if !item.body.is_empty() {
        out.push_str(item.body.trim());
        out.push_str("\n\n");
    }

    // Structured tags
    let params: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Param).collect();
    let tparams: Vec<&_> = item.tags.iter().filter(|t| {
        matches!(&t.kind, TagKind::Other(s) if s.eq_ignore_ascii_case("tparam"))
    }).collect();
    let returns: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Return).collect();
    let throws: Vec<&_> = item.tags.iter().filter(|t| {
        matches!(&t.kind, TagKind::Other(s) if s.eq_ignore_ascii_case("throws") || s.eq_ignore_ascii_case("exception"))
    }).collect();
    let examples: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Example).collect();
    let sees: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::See).collect();
    let notes: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Note).collect();
    let warnings: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Warning).collect();
    let deprecated: Vec<&_> = item.tags.iter().filter(|t| t.kind == TagKind::Deprecated).collect();

    if !deprecated.is_empty() {
        out.push_str("> ⚠️ **Deprecated**");
        if let Some(text) = deprecated.first().map(|t| t.text.trim()).filter(|t| !t.is_empty()) {
            out.push_str(&format!(": {text}"));
        }
        out.push_str("\n\n");
    }

    if !params.is_empty() || !tparams.is_empty() {
        out.push_str("**Parameters**\n\n");
        for tag in &tparams {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `<{name}>` — {}\n", tag.text.trim()));
        }
        for tag in &params {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `{name}` — {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !returns.is_empty() {
        out.push_str("**Returns**\n\n");
        for tag in &returns {
            out.push_str(&format!("{}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !throws.is_empty() {
        out.push_str("**Throws**\n\n");
        for tag in throws {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str(&format!("- `{name}` — {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !notes.is_empty() {
        out.push_str("**Note**\n\n");
        for tag in &notes {
            out.push_str(&format!("> {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !warnings.is_empty() {
        out.push_str("**Warning**\n\n");
        for tag in &warnings {
            out.push_str(&format!("> ⚠️ {}\n", tag.text.trim()));
        }
        out.push('\n');
    }

    if !examples.is_empty() {
        for tag in &examples {
            out.push_str("**Example**\n\n");
            let text = tag.text.trim();
            if text.starts_with("```") {
                out.push_str(text);
            } else {
                out.push_str(&format!("```\n{text}\n```"));
            }
            out.push_str("\n\n");
        }
    }

    if !sees.is_empty() {
        out.push_str("**See also**: ");
        let refs: Vec<String> = sees.iter().map(|t| format!("`{}`", t.text.trim())).collect();
        out.push_str(&refs.join(", "));
        out.push('\n');
    }

    // File/line footer
    let rel = item.file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !rel.is_empty() && item.line > 0 {
        out.push_str(&format!("\n---\n*defined in `{}:{}`*\n", rel, item.line));
    }

    out
}

// ---------------------------------------------------------------------------
// Word extraction
// ---------------------------------------------------------------------------

/// Extract the identifier word at `(line, character)` from `text`.
pub fn word_at(text: &str, line: usize, character: usize) -> Option<String> {
    let line_text = text.lines().nth(line)?;
    let char_idx = character.min(line_text.len());
    let before = &line_text[..char_idx];
    let after = &line_text[char_idx..];
    let start = before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '~')
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let end = char_idx
        + after
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(after.len());
    if start >= end {
        return None;
    }
    let word = &line_text[start..end];
    if word.is_empty() || word.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(word.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn simple_name(name: &str) -> &str {
    let name = name.trim_start_matches('~');
    name.rsplit("::").next().unwrap_or(name)
}

fn lang_id(lang: DocLanguage) -> &'static str {
    match lang {
        DocLanguage::C => "c",
        DocLanguage::Cpp => "cpp",
        DocLanguage::Fortran => "fortran",
        DocLanguage::Rust => "rust",
        DocLanguage::Ada => "ada",
        DocLanguage::D => "d",
        _ => "text",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freight_package_index_prefers_public_headers() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("include")).unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            r#"
[package]
name = "core"
version = "0.1.0"

[language.c]
std = "c17"

[lib]
type = "static"
srcs = ["src/private.c"]
hdrs = ["include/core.h"]
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("include/core.h"), "/// Public API.\nint core_public(void);").unwrap();
        std::fs::write(tmp.path().join("src/private.c"), "/// Private implementation.\nint core_private(void) { return 1; }").unwrap();

        let index = DocIndex::build_freight_packages([tmp.path()]);

        assert!(index.lookup("core_public").is_some());
        assert!(index.lookup("core_private").is_none());
    }

    #[test]
    fn parse_include_header_angle_brackets() {
        assert_eq!(parse_include_header("#include <zlib.h>"), Some(("zlib.h".into(), true)));
        assert_eq!(parse_include_header("  #include  <foo/bar.h>"), Some(("foo/bar.h".into(), true)));
    }

    #[test]
    fn parse_include_header_quotes() {
        assert_eq!(parse_include_header(r#"#include "myheader.h""#), Some(("myheader.h".into(), false)));
    }



    #[test]
    fn header_index_lookup_by_basename() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("include/mylib")).unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            "[package]\nname = \"mylib\"\nversion = \"1.0.0\"\n\n[language.c]\nstd = \"c17\"\n\n[lib]\ntype = \"static\"\nsrcs = []\nhdrs = [\"include/mylib/api.h\"]\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("include/mylib/api.h"), "// header").unwrap();

        let specs = [HeaderDirSpec { path: tmp.path(), origin: HeaderOrigin::Project }];
        let index = HeaderIndex::build(&specs, None);
        assert!(index.lookup("api.h").is_some());
        assert!(index.lookup("mylib/api.h").is_some());
        assert_eq!(index.lookup("api.h").unwrap().package_name, "mylib");
        assert_eq!(index.lookup("api.h").unwrap().origin, HeaderOrigin::Project);
    }
}
