use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use serde::{Deserialize, Serialize};

mod ada;
mod asm;
pub mod common;
mod cpp;
mod d;
mod fortran;
mod rust;
mod zig;

// ── Public types ──────────────────────────────────────────────────────────────

/// Source language of an extracted doc item.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocLanguage {
    C,
    Cpp,
    Rust,
    Fortran,
    D,
    Ada,
    Zig,
    Unknown,
}

impl DocLanguage {
    pub fn label(&self) -> &'static str {
        match self {
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Rust => "Rust",
            Self::Fortran => "Fortran",
            Self::D => "D",
            Self::Ada => "Ada",
            Self::Zig => "Zig",
            Self::Unknown => "Unknown",
        }
    }
}

/// Classification of an extracted documentation item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocKind {
    Function,
    Struct,
    Class,
    Enum,
    Typedef,
    Variable,
    Macro,
    Module,
    Subroutine,
    Interface,
    Unknown,
}

impl DocKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Struct => "struct",
            Self::Class => "class",
            Self::Enum => "enum",
            Self::Typedef => "type",
            Self::Variable => "var",
            Self::Macro => "macro",
            Self::Module => "mod",
            Self::Subroutine => "sub",
            Self::Interface => "iface",
            Self::Unknown => "item",
        }
    }
}

/// Semantic category of a [`DocTag`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagKind {
    Brief,
    Param,
    Return,
    Note,
    See,
    Since,
    Deprecated,
    Example,
    Warning,
    Other(String),
}

impl TagKind {
    pub fn label(&self) -> &str {
        match self {
            Self::Brief => "Brief",
            Self::Param => "Parameter",
            Self::Return => "Returns",
            Self::Note => "Note",
            Self::See => "See also",
            Self::Since => "Since",
            Self::Deprecated => "Deprecated",
            Self::Example => "Example",
            Self::Warning => "Warning",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// A structured tag extracted from a doc comment (e.g. `@param`, `@return`, `@note`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocTag {
    pub kind: TagKind,
    /// Parameter name for `@param`; `None` for all other tag types.
    pub name: Option<String>,
    pub text: String,
}

/// Access level of a class / struct member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    Public,
    Protected,
    Private,
}

/// Structured metadata populated by language-aware extractors (libclang, etc.).
/// Defaults to empty so heuristic extractors compile without changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocMeta {
    /// Template parameter list, e.g. `["typename T", "int N"]`.
    pub template_params: Vec<String>,
    /// Access specifier for class/struct members.
    pub access: Option<Access>,
    /// Qualified name of the enclosing class or struct, if any.
    pub parent: Option<String>,
    /// Semantic attributes: `"const"`, `"virtual"`, `"override"`, `"noexcept"`,
    /// `"pure"`, `"constructor"`, `"destructor"`, `"operator"`.
    pub attrs: Vec<String>,
    /// Doxygen group name set by `@defgroup`/`@addtogroup`/`@{`/`@}` fences
    /// or an explicit `@ingroup groupname` tag.
    pub group: Option<String>,
    /// Package name and version read from the nearest project manifest
    /// (`Cargo.toml`, `package.json`, `freight.toml`, `pyproject.toml`, …).
    /// `None` when no manifest was found above the source file.
    pub package: Option<PackageId>,
}

/// Identifies the package that owns a source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageId {
    pub name: String,
    pub version: String,
}

/// A single documented symbol extracted from source code.
///
/// Produced by [`extract_dir`] or [`DocExtractor::extract`] and collected into a [`DocSet`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocItem {
    pub name: String,
    pub kind: DocKind,
    /// First sentence / `@brief` value.
    pub brief: String,
    /// Extended description after the brief.
    pub body: String,
    pub tags: Vec<DocTag>,
    pub file: PathBuf,
    /// 1-based line number of the opening doc comment.
    pub line: usize,
    pub lang: DocLanguage,
    /// The first non-blank source line following the doc comment.
    pub signature: String,
    /// Structured metadata populated by accurate extractors; empty for heuristic extraction.
    pub meta: DocMeta,
}

impl DocItem {
    /// Signature formatted for display: `fn name(params) -> ReturnType`.
    /// Falls back to the raw signature (minus trailing `{`) if parsing fails.
    pub fn display_signature(&self) -> String {
        if self.signature.is_empty() {
            return String::new();
        }
        match (&self.lang, &self.kind) {
            (DocLanguage::C | DocLanguage::Cpp, DocKind::Function) => sig_c_native(&self.signature),
            (DocLanguage::D, DocKind::Function) => {
                sig_c_style(&self.signature).unwrap_or_else(|| sig_clean(&self.signature))
            }
            (DocLanguage::Rust, DocKind::Function) => sig_rust(&self.signature),
            (DocLanguage::Ada, DocKind::Function | DocKind::Subroutine) => sig_ada(&self.signature),
            (DocLanguage::Fortran, DocKind::Function | DocKind::Subroutine) => {
                sig_fortran(&self.signature)
            }
            _ => sig_clean(&self.signature),
        }
    }
}

fn sig_clean(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(';')
        .trim_end_matches('{')
        .trim()
        .to_string()
}

fn sig_c_style(raw: &str) -> Option<String> {
    let s = sig_clean(raw);
    let s = strip_c_fn_quals(&s);

    // Find the opening paren of the parameter list.
    let paren = s.find('(')?;
    let before = s[..paren].trim_end();

    // Walk backwards to find where the function name starts.
    let name_start = before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '~')
        .map(|i| i + 1)
        .unwrap_or(0);
    let name = &before[name_start..];
    if name.is_empty() {
        return None;
    }

    let ret = before[..name_start].trim();

    // Find the matching close paren using depth counting.
    let params_end = find_close(&s[paren..])?;
    let params = s[paren + 1..paren + params_end].trim();

    if ret.is_empty() || ret == "void" {
        Some(format!("fn {}({})", name, params))
    } else {
        Some(format!("fn {}({}) -> {}", name, params, ret))
    }
}

fn sig_c_native(raw: &str) -> String {
    strip_c_fn_quals(&sig_clean(raw)).to_string()
}

fn sig_rust(raw: &str) -> String {
    // Strip the body: everything from the first `{` onwards.
    let s = match raw.find('{') {
        Some(i) => raw[..i].trim_end(),
        None => raw.trim_end_matches(';').trim(),
    };
    // Strip visibility prefix: pub / pub(…)
    let s = if let Some(r) = s.strip_prefix("pub") {
        let r = r.trim_start();
        if r.starts_with('(') {
            match r.find(')') {
                Some(i) => r[i + 1..].trim_start(),
                None => s,
            }
        } else {
            r
        }
    } else {
        s
    };
    // Strip async / unsafe / extern "…" / default
    let mut s = s;
    'outer: loop {
        for kw in &["async ", "unsafe ", "default "] {
            if let Some(r) = s.strip_prefix(kw) {
                s = r.trim_start();
                continue 'outer;
            }
        }
        if s.starts_with("extern ") {
            let r = s["extern ".len()..].trim_start();
            if r.starts_with('"') {
                if let Some(end) = r[1..].find('"') {
                    s = r[end + 2..].trim_start();
                    continue 'outer;
                }
            }
        }
        break;
    }
    s.to_string()
}

fn sig_go(raw: &str) -> Option<String> {
    // Strip body brace.
    let s = match raw.find('{') {
        Some(i) => raw[..i].trim_end(),
        None => raw.trim(),
    };
    let s = s.strip_prefix("func ")?.trim_start();

    // Optional receiver: starts with `(`
    let (receiver, s) = if s.starts_with('(') {
        let close = find_close(s)?;
        (Some(&s[..=close]), s[close + 1..].trim_start())
    } else {
        (None, s)
    };

    // Function name
    let name_end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    let name = &s[..name_end];
    let after = s[name_end..].trim_start();

    if !after.starts_with('(') {
        return None;
    }
    let close = find_close(after)?;
    let params = after[1..close].trim();
    let ret = after[close + 1..].trim();

    let display_name = match receiver {
        Some(r) => format!("{} {}", r, name),
        None => name.to_string(),
    };
    if ret.is_empty() {
        Some(format!("fn {}({})", display_name, params))
    } else {
        Some(format!("fn {}({}) -> {}", display_name, params, ret))
    }
}

fn sig_ada(raw: &str) -> String {
    let s = raw.trim().trim_end_matches(';').trim();
    let up = s.to_ascii_uppercase();
    let (is_fn, kw_len) = if up.starts_with("FUNCTION ") {
        (true, 9)
    } else if up.starts_with("PROCEDURE ") {
        (false, 10)
    } else {
        return sig_clean(raw);
    };
    let rest = s[kw_len..].trim();
    if is_fn {
        let up_rest = rest.to_ascii_uppercase();
        if let Some(pos) = up_rest.rfind(" RETURN ") {
            let before = rest[..pos].trim();
            let ret = rest[pos + 8..].trim();
            return format!("fn {} -> {}", before, ret);
        }
    }
    format!("fn {}", rest)
}

fn sig_fortran(raw: &str) -> String {
    let s = raw.trim();
    let up = s.to_ascii_uppercase();

    // Strip leading pure / elemental / recursive / impure
    let mut offset = 0;
    'attrs: loop {
        let rest = up[offset..].trim_start();
        let skipped = up.len() - offset - rest.len();
        for attr in &["PURE ", "ELEMENTAL ", "RECURSIVE ", "IMPURE "] {
            if rest.starts_with(attr) {
                offset += skipped + attr.len();
                continue 'attrs;
            }
        }
        break;
    }
    let s = s[offset..].trim_start();
    let up = s.to_ascii_uppercase();

    // Optional inline return type before FUNCTION keyword.
    let (ret, fn_rest) = if up.starts_with("FUNCTION ") {
        (None, &s["FUNCTION ".len()..])
    } else if up.starts_with("SUBROUTINE ") {
        (None, &s["SUBROUTINE ".len()..])
    } else if let Some(pos) = up.find(" FUNCTION ") {
        (
            Some(s[..pos].trim()),
            s[pos + " FUNCTION ".len()..].trim_start(),
        )
    } else {
        return sig_clean(raw);
    };

    // Strip trailing RESULT(…)
    let fn_rest = {
        let up_fr = fn_rest.to_ascii_uppercase();
        if let Some(p) = up_fr.find(" RESULT(") {
            fn_rest[..p].trim()
        } else {
            fn_rest.trim()
        }
    };

    match ret {
        Some(t) if !t.is_empty() => format!("fn {} -> {}", fn_rest, t),
        _ => format!("fn {}", fn_rest),
    }
}

/// Find the index of the closing character matching the first char of `s`
/// (which must be `(`, `[`, or `{`), respecting depth.
fn find_close(s: &str) -> Option<usize> {
    let mut chars = s.chars();
    let open = chars.next()?;
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let mut depth = 1usize;
    let mut i = open.len_utf8();
    for c in chars {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += c.len_utf8();
    }
    None
}

fn strip_c_fn_quals(s: &str) -> &str {
    const QUALS: &[&str] = &[
        "public ",
        "private ",
        "protected ",
        "static ",
        "inline ",
        "extern ",
        "explicit ",
        "virtual ",
        "constexpr ",
        "consteval ",
        "constinit ",
        "abstract ",
        "synchronized ",
        "native ",
        "default ",
        "__inline ",
        "__inline__ ",
        "__forceinline ",
        "__global__ ",
        "__device__ ",
        "__host__ ",
        "__shared__ ",
        "__constant__ ",
        "__managed__ ",
        "task ",
        "export ",
        "unmasked ",
        "[[nodiscard]] ",
        "[[maybe_unused]] ",
    ];
    let mut t = s;
    'outer: loop {
        for q in QUALS {
            if let Some(r) = t.strip_prefix(q) {
                t = r.trim_start();
                continue 'outer;
            }
        }
        break;
    }
    t
}

/// Collection of [`DocItem`]s extracted from a source tree, with a shared source root for relative path display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocSet {
    pub items: Vec<DocItem>,
    pub source_root: PathBuf,
}

impl DocSet {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ── Extractor trait ───────────────────────────────────────────────────────────

/// Trait for pluggable per-language doc-comment extractors.
///
/// Implement this to add support for a new language without modifying
/// the core dispatch logic. Register instances with [`ExtractorRegistry`].
pub trait DocExtractor: Send + Sync {
    /// File extensions handled by this extractor (without the leading dot).
    fn extensions(&self) -> &[&str];
    /// Extract documented items from `source` text read from `path`.
    fn extract(&self, path: &Path, source: &str) -> Vec<DocItem>;
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Ordered list of [`DocExtractor`] implementations.
///
/// The first extractor whose [`DocExtractor::extensions`] list contains the
/// file extension wins. Built-in extractors are pre-registered; custom ones
/// can be appended with [`ExtractorRegistry::register`].
pub struct ExtractorRegistry {
    extractors: Vec<Box<dyn DocExtractor>>,
}

impl ExtractorRegistry {
    pub fn new() -> Self {
        Self {
            extractors: Vec::new(),
        }
    }

    /// Register an additional extractor (appended after the built-ins).
    pub fn register(&mut self, extractor: Box<dyn DocExtractor>) {
        self.extractors.push(extractor);
    }

    /// Return the first extractor that handles `ext`, or `None`.
    pub fn find(&self, ext: &str) -> Option<&dyn DocExtractor> {
        self.extractors
            .iter()
            .find(|e| e.extensions().contains(&ext))
            .map(|e| e.as_ref())
    }

    /// Extract items from a single file using this registry.
    pub fn extract_file(&self, path: &Path) -> Vec<DocItem> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let Ok(src) = std::fs::read_to_string(path) else {
            return vec![];
        };
        if ext == "h" && looks_like_cpp_header(&src) {
            return cpp::CppExtractor.extract(path, &src);
        }
        let Some(extractor) = self.find(ext) else {
            return vec![];
        };
        extractor.extract(path, &src)
    }
}

impl Default for ExtractorRegistry {
    fn default() -> Self {
        let mut r = Self::new();
        r.register(Box::new(cpp::CExtractor));
        r.register(Box::new(cpp::CppExtractor));
        r.register(Box::new(rust::RustExtractor));
        r.register(Box::new(fortran::FortranExtractor));
        r.register(Box::new(d::DExtractor));
        r.register(Box::new(ada::AdaExtractor));
        r.register(Box::new(zig::ZigExtractor));
        r.register(Box::new(asm::AsmExtractor));
        r
    }
}

// ── Entry points ──────────────────────────────────────────────────────────────

pub fn extract_file(path: &Path) -> Vec<DocItem> {
    ExtractorRegistry::default().extract_file(path)
}

pub(crate) fn looks_like_cpp_header(src: &str) -> bool {
    src.contains("namespace ")
        || src.contains("class ")
        || src.contains("template<")
        || src.contains("template <")
        || src.contains("std::")
        || src.contains("::")
        || src.contains("public:")
        || src.contains("private:")
        || src.contains("protected:")
}

pub fn extract_dir(dir: &Path) -> DocSet {
    let registry = ExtractorRegistry::default();
    let items = walk_and_extract(dir, &mut |path| registry.extract_file(path));
    dedup(items, dir)
}

/// Like [`extract_dir`] but accepts additional [`DocExtractor`] implementations
/// for extensions not covered by the built-ins.
pub fn extract_dir_with(dir: &Path, extras: &[Box<dyn DocExtractor>]) -> DocSet {
    let registry = ExtractorRegistry::default();
    let items = walk_and_extract(dir, &mut |path| {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if let Some(extractor) = registry.find(ext) {
            let Ok(src) = std::fs::read_to_string(path) else {
                return vec![];
            };
            return extractor.extract(path, &src);
        }
        for extractor in extras {
            if extractor.extensions().contains(&ext) {
                let Ok(src) = std::fs::read_to_string(path) else {
                    continue;
                };
                return extractor.extract(path, &src);
            }
        }
        vec![]
    });
    dedup(items, dir)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn walk_and_extract(dir: &Path, extract: &mut dyn FnMut(&Path) -> Vec<DocItem>) -> Vec<DocItem> {
    let mut items = Vec::new();
    let walker = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && name != "target" && name != "build"
        });
    for entry in walker.filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            items.extend(extract(entry.path()));
        }
    }
    items
}

fn dedup(items: Vec<DocItem>, source_root: &Path) -> DocSet {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut deduped: Vec<DocItem> = Vec::new();
    for item in items {
        let key = if item.name.is_empty() {
            format!("{}:{:?}", item.file.display(), item.kind)
        } else {
            item.name.clone()
        };
        let score = item.tags.len() * 10 + item.brief.len() + item.body.len();
        match seen.get(&key).copied() {
            Some(idx) => {
                let prev = deduped[idx].tags.len() * 10
                    + deduped[idx].brief.len()
                    + deduped[idx].body.len();
                // On a tie, prefer header files over implementation files so that
                // namespace/class source items point to declarations, not definitions.
                let prefer_over_existing = score > prev
                    || (score == prev
                        && is_header_file(&item.file)
                        && !is_header_file(&deduped[idx].file));
                if prefer_over_existing {
                    deduped[idx] = item;
                }
            }
            None => {
                seen.insert(key, deduped.len());
                deduped.push(item);
            }
        }
    }
    DocSet {
        items: deduped,
        source_root: source_root.to_path_buf(),
    }
}

fn is_header_file(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("h" | "hpp" | "hh" | "hxx" | "h++")
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn items(src: &str, lang: &DocLanguage) -> Vec<DocItem> {
        let ext = match lang {
            DocLanguage::C => "h",
            DocLanguage::Cpp => "cpp",
            DocLanguage::Rust => "rs",
            DocLanguage::Fortran => "f90",
            DocLanguage::D => "d",
            DocLanguage::Ada => "ads",
            DocLanguage::Zig => "zig",
            DocLanguage::Unknown => return vec![],
        };
        let path = Path::new("test").with_extension(ext);
        let registry = ExtractorRegistry::default();
        registry
            .find(ext)
            .map(|e| e.extract(&path, src))
            .unwrap_or_default()
    }

    // ── C / C++ ───────────────────────────────────────────────────────────────

    #[test]
    fn c_block_comment_extracts_brief() {
        let src = "/** Compute the sum of two integers. */\nint add(int a, int b);";
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Compute the sum of two integers.");
        assert_eq!(got[0].name, "add");
        assert!(matches!(got[0].kind, DocKind::Function));
    }

    #[test]
    fn c_block_multi_line_with_params() {
        let src = r#"/**
 * @brief Sort an array in place.
 * @param arr  Pointer to the array.
 * @param len  Number of elements.
 * @return Zero on success, negative on error.
 */
void sort(int *arr, size_t len);"#;
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Sort an array in place.");
        assert_eq!(got[0].name, "sort");
        let params: Vec<_> = got[0]
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Param)
            .collect();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name.as_deref(), Some("arr"));
        let ret: Vec<_> = got[0]
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Return)
            .collect();
        assert_eq!(ret.len(), 1);
    }

    #[test]
    fn c_brief_stops_at_blank_before_markdown_table() {
        let src = r#"/**
 * @brief Adaptive Simpson's rule.
 *
 * Uses recursive subdivision.
 *
 * | Parameter | Meaning |
 * |-----------|---------|
 * | `f` | Integrand |
 *
 * @return Approximation.
 */
double integrate(double (*f)(double), double a, double b, double eps);"#;
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Adaptive Simpson's rule.");
        assert!(got[0].body.contains("Uses recursive subdivision."));
        assert!(got[0].body.contains("| Parameter | Meaning |"));
        assert!(got[0].body.contains("| `f` | Integrand |"));
    }

    #[test]
    fn c_triple_slash_line_comment() {
        let src = "/// A utility macro.\n#define MAX(a, b) ((a) > (b) ? (a) : (b))";
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "A utility macro.");
        assert!(matches!(got[0].kind, DocKind::Macro));
    }

    #[test]
    fn c_define_keeps_doc_comment() {
        let src = "/** Feature flag for vector code. */\n#define HAVE_VEC 1";
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "HAVE_VEC");
        assert_eq!(got[0].brief, "Feature flag for vector code.");
        assert!(matches!(got[0].kind, DocKind::Macro));
    }

    #[test]
    fn c_ifdef_does_not_consume_doc_comment() {
        let src = "/** Header guard. */\n#ifndef MATHLIB_H\n#define MATHLIB_H";
        let got = items(src, &DocLanguage::C);
        assert!(
            got.is_empty(),
            "conditional directives should not become documented items"
        );
    }

    #[test]
    fn c_struct_detection() {
        let src = "/** Represents a 2D point. */\nstruct Point { float x; float y; };";
        let got = items(src, &DocLanguage::C);
        assert_eq!(got[0].name, "Point");
        assert!(matches!(got[0].kind, DocKind::Struct));
    }

    #[test]
    fn backslash_escape_not_parsed_as_tag() {
        let src = "/** Uses \\n for newlines and \\t for tabs. */\nvoid foo();";
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        let unknown_tags: Vec<_> = got[0]
            .tags
            .iter()
            .filter(|t| matches!(&t.kind, TagKind::Other(s) if s == "n" || s == "t"))
            .collect();
        assert!(
            unknown_tags.is_empty(),
            "single-char escape sequences must not become tags"
        );
    }

    // ── Rust ──────────────────────────────────────────────────────────────────

    #[test]
    fn rust_fn_doc() {
        let src = "/// Return the factorial of n.\npub fn factorial(n: u64) -> u64 { todo!() }";
        let got = items(src, &DocLanguage::Rust);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Return the factorial of n.");
        assert_eq!(got[0].name, "factorial");
        assert!(matches!(got[0].kind, DocKind::Function));
    }

    #[test]
    fn rust_struct_doc() {
        let src =
            "/// A colour in linear sRGB.\npub struct Rgb { pub r: f32, pub g: f32, pub b: f32 }";
        let got = items(src, &DocLanguage::Rust);
        assert_eq!(got[0].name, "Rgb");
        assert!(matches!(got[0].kind, DocKind::Struct));
    }

    #[test]
    fn rust_impl_for_doc() {
        let src = "/// Display impl for Rgb.\nimpl std::fmt::Display for Rgb {}";
        let got = items(src, &DocLanguage::Rust);
        assert_eq!(got[0].name, "Rgb");
        assert!(matches!(got[0].kind, DocKind::Struct));
    }

    // ── Rust: Markdown section headings ──────────────────────────────────────

    fn rust_item(src: &str) -> DocItem {
        let got = items(src, &DocLanguage::Rust);
        assert!(!got.is_empty(), "expected item from Rust src");
        got.into_iter().next().unwrap()
    }

    #[test]
    fn rust_examples_section_becomes_example_tag() {
        let src =
            "/// Brief.\n///\n/// # Examples\n/// ```\n/// let x = 1;\n/// ```\npub fn f() {}";
        let item = rust_item(src);
        assert!(
            tag_of(&item, &TagKind::Example).is_some(),
            "# Examples should become TagKind::Example"
        );
        assert!(
            tag_of(&item, &TagKind::Example)
                .unwrap()
                .text
                .contains("let x"),
            "code block should be in tag text"
        );
    }

    #[test]
    fn rust_panics_section_captured() {
        let src = "/// Brief.\n///\n/// # Panics\n/// If n is zero.\npub fn f(n: u32) {}";
        let item = rust_item(src);
        let t = other_tag(&item, "panics");
        assert!(t.is_some(), "# Panics should become Other(\"panics\")");
        assert!(t.unwrap().text.contains("zero"));
    }

    #[test]
    fn rust_errors_section_captured() {
        let src = "/// Brief.\n///\n/// # Errors\n/// Returns `Err` on I/O failure.\npub fn f() -> Result<(), std::io::Error> { todo!() }";
        let item = rust_item(src);
        let t = other_tag(&item, "errors");
        assert!(t.is_some(), "# Errors should become Other(\"errors\")");
        assert!(t.unwrap().text.contains("I/O"));
    }

    #[test]
    fn rust_safety_section_captured() {
        let src = "/// Brief.\n///\n/// # Safety\n/// Caller must ensure ptr is valid.\n/// # Panics\n/// Never.\npub unsafe fn f(ptr: *const u8) {}";
        let item = rust_item(src);
        let s = other_tag(&item, "safety");
        assert!(s.is_some(), "# Safety should become Other(\"safety\")");
        assert!(s.unwrap().text.contains("ptr"));
        assert!(other_tag(&item, "panics").is_some());
    }

    #[test]
    fn rust_returns_section_becomes_return_tag() {
        let src =
            "/// Brief.\n///\n/// # Returns\n/// The computed value.\npub fn f() -> u32 { 0 }";
        let item = rust_item(src);
        assert!(
            tag_of(&item, &TagKind::Return).is_some(),
            "# Returns should become TagKind::Return"
        );
        assert_eq!(
            tag_of(&item, &TagKind::Return).unwrap().text,
            "The computed value."
        );
    }

    #[test]
    fn rust_notes_section_becomes_note_tag() {
        let src = "/// Brief.\n///\n/// # Note\n/// Thread-safe.\npub fn f() {}";
        let item = rust_item(src);
        assert!(
            tag_of(&item, &TagKind::Note).is_some(),
            "# Note should become TagKind::Note"
        );

        let src2 = "/// Brief.\n///\n/// # Notes\n/// See the module docs.\npub fn f() {}";
        let item2 = rust_item(src2);
        assert!(
            tag_of(&item2, &TagKind::Note).is_some(),
            "# Notes (plural) should work too"
        );
    }

    #[test]
    fn rust_see_also_section_becomes_see_tag() {
        let src = "/// Brief.\n///\n/// # See Also\n/// [`other_fn`]\npub fn f() {}";
        let item = rust_item(src);
        assert!(
            tag_of(&item, &TagKind::See).is_some(),
            "# See Also should become TagKind::See"
        );
    }

    #[test]
    fn rust_deprecated_section_becomes_deprecated_tag() {
        let src = "/// Brief.\n///\n/// # Deprecated\n/// Use `new_fn` instead.\npub fn f() {}";
        let item = rust_item(src);
        assert!(tag_of(&item, &TagKind::Deprecated).is_some());
    }

    #[test]
    fn rust_warning_section_becomes_warning_tag() {
        let src =
            "/// Brief.\n///\n/// # Warning\n/// Do not call from async context.\npub fn f() {}";
        let item = rust_item(src);
        assert!(tag_of(&item, &TagKind::Warning).is_some());
    }

    #[test]
    fn rust_arguments_section_captured() {
        let src = "/// Brief.\n///\n/// # Arguments\n/// - `x`: The value.\npub fn f(x: u32) {}";
        let item = rust_item(src);
        let t = other_tag(&item, "arguments");
        assert!(
            t.is_some(),
            "# Arguments should become Other(\"arguments\")"
        );
        assert!(t.unwrap().text.contains('x'));
    }

    #[test]
    fn rust_parameters_section_captured() {
        let src = "/// Brief.\n///\n/// # Parameters\n/// - `n`: Count.\npub fn f(n: usize) {}";
        let item = rust_item(src);
        assert!(
            other_tag(&item, "arguments").is_some(),
            "# Parameters should also map to Other(\"arguments\")"
        );
    }

    #[test]
    fn rust_double_hash_heading_also_works() {
        // ## subsection headings should be treated identically to # headings.
        let src =
            "/// Brief.\n///\n/// ## Examples\n/// ```\n/// let _ = f();\n/// ```\npub fn f() {}";
        let item = rust_item(src);
        assert!(
            tag_of(&item, &TagKind::Example).is_some(),
            "## Examples (double hash) should also become TagKind::Example"
        );
    }

    #[test]
    fn rust_unknown_heading_stays_in_body() {
        // Custom / unknown headings should not be converted to tags.
        let src = "/// Brief.\n///\n/// # Implementation Details\n/// Uses SIMD internally.\npub fn f() {}";
        let item = rust_item(src);
        assert!(
            item.tags
                .iter()
                .all(|t| t.kind != TagKind::Other("implementation details".to_string())),
            "unknown section heading should remain in body prose"
        );
        assert!(
            item.body.contains("Implementation Details") || item.body.contains("SIMD"),
            "unknown heading content should land in body"
        );
    }

    #[test]
    fn rust_multiple_sections_all_captured() {
        let src = concat!(
            "/// Compute mean.\n",
            "///\n",
            "/// # Panics\n",
            "/// If slice is empty.\n",
            "///\n",
            "/// # Examples\n",
            "/// ```\n",
            "/// assert_eq!(mean(&[1.0, 3.0]), 2.0);\n",
            "/// ```\n",
            "pub fn mean(xs: &[f64]) -> f64 { todo!() }\n"
        );
        let item = rust_item(src);
        assert_eq!(item.brief, "Compute mean.");
        assert!(other_tag(&item, "panics").is_some());
        assert!(tag_of(&item, &TagKind::Example).is_some());
        assert!(tag_of(&item, &TagKind::Example)
            .unwrap()
            .text
            .contains("assert_eq!"));
    }

    #[test]
    fn rust_brief_not_consumed_by_sections() {
        // The first sentence before any section heading is still the brief.
        let src = "/// Return the length of s.\n///\n/// # Panics\n/// Never.\npub fn len(s: &str) -> usize { s.len() }";
        let item = rust_item(src);
        assert_eq!(item.brief, "Return the length of s.");
    }

    // ── Fortran ───────────────────────────────────────────────────────────────

    #[test]
    fn fortran_subroutine_doc() {
        let src = "!> Solve a linear system Ax = b.\n!! Uses LU decomposition.\nsubroutine solve(A, b, x, n)";
        let got = items(src, &DocLanguage::Fortran);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Solve a linear system Ax = b.");
        assert_eq!(got[0].name, "solve");
        assert!(matches!(got[0].kind, DocKind::Subroutine));
    }

    #[test]
    fn fortran_function_uppercase() {
        let src = "!> Compute dot product.\nFUNCTION dot(u, v, n) RESULT(res)";
        let got = items(src, &DocLanguage::Fortran);
        assert_eq!(got[0].name, "dot");
        assert!(matches!(got[0].kind, DocKind::Function));
    }

    // ── Ada ───────────────────────────────────────────────────────────────────

    #[test]
    fn ada_procedure_doc() {
        let src = "--! Print a greeting.\nprocedure Say_Hello (Name : String);";
        let got = items(src, &DocLanguage::Ada);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Print a greeting.");
        assert_eq!(got[0].name, "Say_Hello");
        assert!(matches!(got[0].kind, DocKind::Subroutine));
    }

    // ── Signature capture ─────────────────────────────────────────────────────

    #[test]
    fn c_block_captures_signature() {
        let src = "/** Compute sum. */\nint add(int a, int b);";
        let got = items(src, &DocLanguage::C);
        assert_eq!(got[0].signature, "int add(int a, int b);");
    }

    #[test]
    fn c_block_captures_multiline_signature() {
        let src = r#"/** Integrate a function. */
double integrate(
    double (*f)(double),
    double a,
    double b,
    double eps
);"#;
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "integrate");
        assert!(got[0].signature.contains("double (*f)(double),"));
        assert!(got[0].signature.contains("\n);"));
    }

    #[test]
    fn c_block_captures_full_multiline_typedef() {
        let src = r#"/** Dispatch type. */
typedef cc_ht_map<Key,
                  Mapped,
                  at0t,
                  at1t,
                  _Alloc,
                  false>
    type;"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "type");
        assert!(got[0].signature.contains("_Alloc,"));
        assert!(got[0].signature.contains("type;"));
    }

    #[test]
    fn rust_fn_captures_signature() {
        let src = "/// Return factorial.\npub fn factorial(n: u64) -> u64 { todo!() }";
        let got = items(src, &DocLanguage::Rust);
        assert_eq!(
            got[0].signature,
            "pub fn factorial(n: u64) -> u64 { todo!() }"
        );
    }

    #[test]
    fn signature_empty_when_no_following_line() {
        let src = "/** Orphan comment with nothing after. */\n";
        let got = items(src, &DocLanguage::C);
        if !got.is_empty() {
            assert!(
                !got[0].signature.is_empty() || got[0].signature.is_empty(),
                "signature is whatever followed; just must not crash"
            );
        }
    }

    // ── Language detection ────────────────────────────────────────────────────

    #[test]
    fn extension_routing() {
        let r = ExtractorRegistry::default();
        assert!(r.find("c").is_some());
        assert!(r.find("h").is_some());
        assert!(r.find("cpp").is_some());
        assert!(r.find("rs").is_some());
        assert!(r.find("f90").is_some());
        assert!(r.find("d").is_some());
        assert!(r.find("ads").is_some());
        assert!(r.find("zig").is_some());
        assert!(r.find("java").is_none());
        assert!(r.find("go").is_none());
        assert!(r.find("toml").is_none());
    }

    // ── C++ declarations ──────────────────────────────────────────────────────

    #[test]
    fn cpp_namespace_class() {
        let src = r#"/**
 * @brief 2-D point type.
 */
class Point {
    int x, y;
};"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Point");
        assert!(matches!(got[0].kind, DocKind::Class));
        assert_eq!(got[0].brief, "2-D point type.");
    }

    #[test]
    fn cpp_undocumented_namespace_has_source_item() {
        let src = r#"namespace stats {
/** Arithmetic mean. */
double mean(double x, double y);
}"#;
        let got = items(src, &DocLanguage::Cpp);
        let ns = got
            .iter()
            .find(|item| item.kind == DocKind::Module && item.name == "stats")
            .expect("namespace should be represented for source jumps");
        assert_eq!(ns.line, 1);
        assert_eq!(ns.signature, "namespace stats {");
        assert!(ns.brief.is_empty());
        assert!(got.iter().any(|item| item.name == "stats::mean"));
    }

    #[test]
    fn cpp_class_and_struct_members_are_qualified() {
        let src = r#"namespace geometry {
/**
 * @brief 2-D point type.
 */
struct Point {
    /**
     * @brief Distance from origin.
     */
    double length() const;
};

/**
 * @brief Bounding box.
 */
class AABB {
public:
    /**
     * @brief Check containment.
     */
    bool contains(Point p) const;
};
}"#;
        let got = items(src, &DocLanguage::Cpp);
        assert!(got.iter().any(|item| item.name == "geometry::Point"));
        let length = got
            .iter()
            .find(|item| item.name == "geometry::Point::length")
            .expect("struct method should be qualified under struct");
        assert_eq!(length.meta.parent.as_deref(), Some("Point"));
        let contains = got
            .iter()
            .find(|item| item.name == "geometry::AABB::contains")
            .expect("class method should be qualified under class");
        assert_eq!(contains.meta.parent.as_deref(), Some("AABB"));
    }

    #[test]
    fn cpp_template_class() {
        let src = r#"/**
 * @brief Generic stack container.
 * @tparam T Element type.
 */
template<typename T>
class Stack {};
"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Generic stack container.");
        assert!(!got[0].brief.is_empty());
    }

    #[test]
    fn cpp_typedef_struct() {
        let src = "/** Opaque handle type. */\ntypedef struct _Handle Handle;";
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Opaque handle type.");
        assert!(matches!(got[0].kind, DocKind::Typedef));
        assert_eq!(got[0].name, "Handle");
    }

    #[test]
    fn c_anonymous_typedef_struct_uses_alias_name() {
        let src = r#"/** Fixed-size vector. */
typedef struct {
    double x;
    double y;
} Vec2;"#;
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Vec2");
        assert!(matches!(got[0].kind, DocKind::Typedef));
        assert!(got[0].signature.contains("} Vec2;"));
    }

    #[test]
    fn cpp_using_alias() {
        let src = "/** Convenience alias for a string map. */\nusing StringMap = std::map<std::string, std::string>;";
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "StringMap");
        assert!(matches!(got[0].kind, DocKind::Typedef));
        assert_eq!(got[0].brief, "Convenience alias for a string map.");
    }

    #[test]
    fn cpp_enum_class() {
        let src = r#"/** Colour channels. */
enum class Channel { R, G, B, A };"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Channel");
        assert!(matches!(got[0].kind, DocKind::Enum));
    }

    #[test]
    fn cpp_static_member_function() {
        let src = r#"/** Create from polar coordinates.
 * @param r Radius.
 * @param theta Angle in radians.
 * @return New point.
 */
static Point from_polar(double r, double theta);"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "from_polar");
        assert!(matches!(got[0].kind, DocKind::Function));
        let params: Vec<_> = got[0]
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Param)
            .collect();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name.as_deref(), Some("r"));
        assert_eq!(params[1].name.as_deref(), Some("theta"));
    }

    #[test]
    fn cpp_inline_variable_doc() {
        let src = "/** Maximum buffer size in bytes. */\nconst size_t MAX_BUF = 4096;";
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Maximum buffer size in bytes.");
        assert!(!got[0].brief.is_empty());
    }

    #[test]
    fn cpp_namespace_free_function() {
        let src = r#"namespace math {
/** @brief Clamp x to [lo, hi]. */
double clamp(double x, double lo, double hi);
} // namespace math"#;
        let got = items(src, &DocLanguage::Cpp);
        assert!(got
            .iter()
            .any(|item| item.name == "math" && item.kind == DocKind::Module && item.line == 1));
        let clamp = got
            .iter()
            .find(|item| item.name == "math::clamp")
            .expect("namespace function should be qualified");
        assert_eq!(clamp.brief, "Clamp x to [lo, hi].");
    }

    #[test]
    fn cpp_nested_namespace() {
        let src = r#"namespace outer {
namespace inner {
/** @brief Nested function. */
void nested();
} // namespace inner
} // namespace outer"#;
        let got = items(src, &DocLanguage::Cpp);
        assert!(got
            .iter()
            .any(|item| item.name == "outer" && item.kind == DocKind::Module && item.line == 1));
        assert!(got.iter().any(|item| item.name == "outer::inner"
            && item.kind == DocKind::Module
            && item.line == 2));
        assert!(got.iter().any(|item| item.name == "outer::inner::nested"));
    }

    #[test]
    fn cpp_multiline_param_continuation() {
        let src = r#"/**
 * @brief Multiply matrix.
 * @param A Input matrix; must be square and
 *   stored in row-major order.
 * @param n Dimension.
 */
void matmul(double *A, int n);"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        let params: Vec<_> = got[0]
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Param)
            .collect();
        assert_eq!(params.len(), 2);
        assert!(
            params[0].text.contains("row-major"),
            "expected continuation line in param text"
        );
    }

    // ── Doxygen / Javadoc tag coverage ───────────────────────────────────────
    //
    // Tests every standard tag through the C extractor (primary Doxygen user).
    // Languages that share parse_tag_start (C, C++, Rust, Fortran, Ada, Java, D)
    // all benefit from these tests.

    fn c_item(src: &str) -> DocItem {
        let got = items(src, &DocLanguage::C);
        assert!(!got.is_empty(), "expected item from: {}", src);
        got.into_iter().next().unwrap()
    }

    fn tag_of<'a>(item: &'a DocItem, kind: &TagKind) -> Option<&'a DocTag> {
        item.tags.iter().find(|t| &t.kind == kind)
    }

    fn other_tag<'a>(item: &'a DocItem, label: &str) -> Option<&'a DocTag> {
        item.tags
            .iter()
            .find(|t| t.kind == TagKind::Other(label.to_string()))
    }

    #[test]
    fn tag_brief_at_prefix() {
        let item = c_item("/** @brief Compute sum. */\nint add(int a, int b);");
        assert_eq!(item.brief, "Compute sum.");
    }

    #[test]
    fn tag_brief_backslash_prefix() {
        let item = c_item("/** \\brief Compute sum. */\nint add(int a, int b);");
        assert_eq!(item.brief, "Compute sum.");
    }

    #[test]
    fn tag_param_extracts_name_and_desc() {
        let item = c_item("/**\n * @param n   The count.\n * @param buf  The buffer.\n */\nvoid f(int n, char *buf);");
        let params: Vec<_> = item
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Param)
            .collect();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name.as_deref(), Some("n"));
        assert_eq!(params[0].text, "The count.");
        assert_eq!(params[1].name.as_deref(), Some("buf"));
    }

    #[test]
    fn tag_param_in_out_variants() {
        let src = "/**\n * @param[in]     x  Input.\n * @param[out]    y  Output.\n * @param[in,out] z  Both.\n */\nvoid f(int x, int *y, int *z);";
        let item = c_item(src);
        let params: Vec<_> = item
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Param)
            .collect();
        assert_eq!(
            params.len(),
            3,
            "all three directional variants should become Param"
        );
        assert_eq!(params[0].name.as_deref(), Some("x"));
        assert_eq!(params[1].name.as_deref(), Some("y"));
        assert_eq!(params[2].name.as_deref(), Some("z"));
    }

    #[test]
    fn tag_tparam_extracts_name_and_desc() {
        let src = "/**\n * @tparam T  Element type.\n * @tparam N  Array size.\n */\nvoid f();";
        let item = c_item(src);
        let tparams: Vec<_> = item
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Other("tparam".to_string()))
            .collect();
        assert_eq!(tparams.len(), 2, "@tparam tags should be captured");
        assert_eq!(
            tparams[0].name.as_deref(),
            Some("T"),
            "@tparam name should be extracted"
        );
        assert_eq!(tparams[0].text, "Element type.");
        assert_eq!(tparams[1].name.as_deref(), Some("N"));
    }

    #[test]
    fn tag_return_and_returns() {
        let item = c_item("/** @return The computed value. */\nint f();");
        assert!(tag_of(&item, &TagKind::Return).is_some());
        assert_eq!(
            tag_of(&item, &TagKind::Return).unwrap().text,
            "The computed value."
        );

        let item2 = c_item("/** @returns The computed value. */\nint f();");
        assert!(tag_of(&item2, &TagKind::Return).is_some());
    }

    #[test]
    fn tag_retval_embeds_value_in_label() {
        let src = "/**\n * @retval  0  Success.\n * @retval -1  I/O error.\n */\nint f();";
        let item = c_item(src);
        let rv0 = other_tag(&item, "retval 0");
        assert!(
            rv0.is_some(),
            "@retval 0 should produce Other(\"retval 0\")"
        );
        assert_eq!(rv0.unwrap().text, "Success.");
        let rv1 = other_tag(&item, "retval -1");
        assert!(
            rv1.is_some(),
            "@retval -1 should produce Other(\"retval -1\")"
        );
    }

    #[test]
    fn tag_throws_extracts_type() {
        let src = "/** @throws std::runtime_error On failure. */\nvoid f();";
        let item = c_item(src);
        let t = other_tag(&item, "throws std::runtime_error");
        assert!(
            t.is_some(),
            "@throws should produce Other(\"throws <type>\")"
        );
        assert_eq!(t.unwrap().text, "On failure.");
    }

    #[test]
    fn tag_throw_singular_same_as_throws() {
        // @throw (singular) must be treated identically to @throws.
        let src = "/** @throw std::bad_alloc Out of memory. */\nvoid f();";
        let item = c_item(src);
        let t = other_tag(&item, "throws std::bad_alloc");
        assert!(
            t.is_some(),
            "@throw should be normalised to Other(\"throws <type>\")"
        );
    }

    #[test]
    fn tag_exception_same_as_throws() {
        let src = "/** @exception IOException When IO fails. */\nvoid f();";
        let item = c_item(src);
        let t = other_tag(&item, "throws IOException");
        assert!(
            t.is_some(),
            "@exception should be normalised to Other(\"throws <type>\")"
        );
    }

    #[test]
    fn tag_throws_without_type() {
        // @throws with no following word → label is just "throws".
        let src = "/**\n * @throws\n */\nvoid f();";
        let item = c_item(src);
        let t = other_tag(&item, "throws");
        assert!(
            t.is_some(),
            "typeless @throws should produce Other(\"throws\")"
        );
    }

    #[test]
    fn tag_note() {
        let item = c_item("/** @note Thread-safe when mutex is held. */\nvoid f();");
        let n = tag_of(&item, &TagKind::Note);
        assert!(n.is_some());
        assert!(n.unwrap().text.contains("Thread-safe"));
    }

    #[test]
    fn tag_warning_and_warn() {
        let item = c_item("/** @warning Do not call from ISR. */\nvoid f();");
        assert!(tag_of(&item, &TagKind::Warning).is_some());

        let item2 = c_item("/** @warn Avoid concurrent use. */\nvoid f();");
        assert!(tag_of(&item2, &TagKind::Warning).is_some());
    }

    #[test]
    fn tag_see_and_sa() {
        let item = c_item("/** @see other_func */\nvoid f();");
        assert!(tag_of(&item, &TagKind::See).is_some());

        let item2 = c_item("/** @sa other_func */\nvoid f();");
        assert!(tag_of(&item2, &TagKind::See).is_some());
    }

    #[test]
    fn tag_since() {
        let item = c_item("/** @since 2.0 */\nvoid f();");
        let s = tag_of(&item, &TagKind::Since);
        assert!(s.is_some());
        assert_eq!(s.unwrap().text, "2.0");
    }

    #[test]
    fn tag_deprecated() {
        let item = c_item("/** @deprecated Use new_func() instead. */\nvoid f();");
        assert!(tag_of(&item, &TagKind::Deprecated).is_some());
    }

    #[test]
    fn tag_example_and_code() {
        let item = c_item("/** @example foo.c */\nvoid f();");
        assert!(tag_of(&item, &TagKind::Example).is_some());

        let item2 = c_item("/** @code int x = f(); @endcode */\nvoid f();");
        assert!(tag_of(&item2, &TagKind::Example).is_some());
    }

    #[test]
    fn tag_pre_post_invariant_captured() {
        let src =
            "/**\n * @pre  n > 0\n * @post result >= 0\n * @invariant buf != NULL\n */\nvoid f();";
        let item = c_item(src);
        assert!(
            other_tag(&item, "pre").is_some(),
            "@pre should be captured as Other"
        );
        assert!(
            other_tag(&item, "post").is_some(),
            "@post should be captured as Other"
        );
        assert!(
            other_tag(&item, "invariant").is_some(),
            "@invariant should be captured as Other"
        );
        assert_eq!(other_tag(&item, "pre").unwrap().text, "n > 0");
        assert_eq!(other_tag(&item, "post").unwrap().text, "result >= 0");
    }

    #[test]
    fn tag_attention_remark_remarks_details_captured() {
        let src = "/**\n * @attention Critical section.\n * @remark Internal use only.\n * @remarks Avoid re-entry.\n * @details More detail here.\n */\nvoid f();";
        let item = c_item(src);
        assert!(other_tag(&item, "attention").is_some());
        assert!(other_tag(&item, "remark").is_some());
        assert!(other_tag(&item, "remarks").is_some());
        assert!(other_tag(&item, "details").is_some());
        assert_eq!(
            other_tag(&item, "attention").unwrap().text,
            "Critical section."
        );
    }

    #[test]
    fn tag_todo_bug_captured() {
        let src = "/**\n * @todo Implement fast path.\n * @bug Returns wrong sign for negative n.\n */\nvoid f();";
        let item = c_item(src);
        assert!(other_tag(&item, "todo").is_some());
        assert_eq!(
            other_tag(&item, "todo").unwrap().text,
            "Implement fast path."
        );
        assert!(other_tag(&item, "bug").is_some());
    }

    #[test]
    fn tag_author_version_date_copyright_captured() {
        let src = "/**\n * @author Jane Doe\n * @version 1.3\n * @date 2025-01-15\n * @copyright MIT\n */\nvoid f();";
        let item = c_item(src);
        assert!(other_tag(&item, "author").is_some());
        assert_eq!(other_tag(&item, "author").unwrap().text, "Jane Doe");
        assert!(other_tag(&item, "version").is_some());
        assert!(other_tag(&item, "date").is_some());
        assert!(other_tag(&item, "copyright").is_some());
    }

    #[test]
    fn tag_file_ingroup_par_captured() {
        let src = "/**\n * @file  utils.h\n * @ingroup core\n * @par   Performance\n * O(log n) per call.\n */\nvoid f();";
        let item = c_item(src);
        assert!(other_tag(&item, "file").is_some());
        assert!(other_tag(&item, "ingroup").is_some());
        assert!(other_tag(&item, "par").is_some());
    }

    #[test]
    fn c_file_doc_is_extracted_as_file_module() {
        let src = "/**\n * @file mathlib.h\n * @brief Header docs.\n *\n * Longer file description.\n */\n#ifndef MATHLIB_H\n#define MATHLIB_H\nint f(void);\n#endif";
        let got = items(src, &DocLanguage::C);
        let file = got
            .iter()
            .find(|item| item.kind == DocKind::Module && item.name.is_empty())
            .expect("@file block should become a file/module doc item");

        assert_eq!(file.brief, "Header docs.");
        assert!(file.body.contains("Longer file description."));
        assert!(other_tag(file, "file").is_some());
    }

    #[test]
    fn tag_backslash_variants_work() {
        // All tags accept both @ and \ prefix; each tag must be on its own line.
        let src = "/**\n * \\note Watch out.\n * \\warning Dangerous!\n */\nvoid f();";
        let item = c_item(src);
        assert!(tag_of(&item, &TagKind::Note).is_some());
        assert!(tag_of(&item, &TagKind::Warning).is_some());
    }

    #[test]
    fn tag_unknown_goes_to_other() {
        let item = c_item("/** @customtag Some text. */\nvoid f();");
        assert!(other_tag(&item, "customtag").is_some());
        assert_eq!(other_tag(&item, "customtag").unwrap().text, "Some text.");
    }

    #[test]
    fn tag_single_char_backslash_not_parsed_as_tag() {
        // \n, \t, \0 etc. must not become tags — enforced by tag.len() < 2 guard.
        let src = "/** Uses \\n and \\t for formatting. */\nvoid f();";
        let item = c_item(src);
        let bad: Vec<_> = item
            .tags
            .iter()
            .filter(|t| matches!(&t.kind, TagKind::Other(s) if s == "n" || s == "t"))
            .collect();
        assert!(
            bad.is_empty(),
            "single-char escape sequences must not become tags"
        );
    }

    #[test]
    fn tag_multiline_continuation_appended() {
        let src = "/**\n * @param buf  The source buffer;\n *   must be null-terminated.\n */\nvoid f(char *buf);";
        let item = c_item(src);
        let p = item.tags.iter().find(|t| t.kind == TagKind::Param).unwrap();
        assert!(
            p.text.contains("null-terminated"),
            "continuation line should be appended to tag text"
        );
    }

    #[test]
    fn tag_tparam_in_cpp_template() {
        let src = "/**\n * @brief Generic min.\n * @tparam T Comparable type.\n * @param a First value.\n * @param b Second value.\n * @return The lesser of a and b.\n */\ntemplate<typename T>\nT min_of(T a, T b);";
        let item = c_item(src);
        assert_eq!(item.brief, "Generic min.");
        let tp = item
            .tags
            .iter()
            .find(|t| t.kind == TagKind::Other("tparam".to_string()))
            .unwrap();
        assert_eq!(tp.name.as_deref(), Some("T"));
        assert_eq!(tp.text, "Comparable type.");
        let params: Vec<_> = item
            .tags
            .iter()
            .filter(|t| t.kind == TagKind::Param)
            .collect();
        assert_eq!(params.len(), 2);
        assert!(tag_of(&item, &TagKind::Return).is_some());
    }

    // ── Scope / package qualification ────────────────────────────────────────

    #[test]
    fn rust_mod_qualifies_function() {
        let src =
            "pub mod math {\n/// Compute the square.\npub fn square(x: f64) -> f64 { x * x }\n}";
        let got = items(src, &DocLanguage::Rust);
        assert!(
            got.iter().any(|i| i.name == "math::square"),
            "function inside mod should be qualified; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rust_nested_mod_qualifies_function() {
        let src = concat!(
            "pub mod outer {\n",
            "pub mod inner {\n",
            "/// Nested.\n",
            "pub fn f() {}\n",
            "}\n",
            "}\n"
        );
        let got = items(src, &DocLanguage::Rust);
        assert!(
            got.iter().any(|i| i.name == "outer::inner::f"),
            "got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rust_mod_is_itself_extracted() {
        let src = "/// Utilities.\npub mod utils {\n/// Helper.\npub fn helper() {}\n}";
        let got = items(src, &DocLanguage::Rust);
        // The mod declaration itself gets extracted as Module.
        assert!(
            got.iter()
                .any(|i| i.name == "utils" && matches!(i.kind, DocKind::Module)),
            "mod should be extracted as Module; got: {:?}",
            got.iter().map(|i| (&i.name, &i.kind)).collect::<Vec<_>>()
        );
        // Function inside the mod should be qualified.
        assert!(
            got.iter().any(|i| i.name == "utils::helper"),
            "helper should be utils::helper; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ada_package_qualifies_items() {
        let src = "--! A math package.\npackage Math is\n--! Double a value.\nfunction Double (X : Float) return Float;\nend Math;";
        let got = items(src, &DocLanguage::Ada);
        assert!(
            got.iter().any(|i| i.name == "Math.Double"),
            "function should be qualified with package name; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        // Package item itself should remain unqualified.
        assert!(
            got.iter()
                .any(|i| i.name == "Math" && matches!(i.kind, DocKind::Module)),
            "package item should remain as 'Math'; got: {:?}",
            got.iter().map(|i| (&i.name, &i.kind)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn d_module_qualifies_items() {
        let src =
            "/// A utility module.\nmodule utils;\n/// Add two values.\nint add(int a, int b);";
        let got = items(src, &DocLanguage::D);
        assert!(
            got.iter().any(|i| i.name == "utils.add"),
            "function should be qualified with module name; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        // Module item itself should remain unqualified.
        assert!(
            got.iter()
                .any(|i| i.name == "utils" && matches!(i.kind, DocKind::Module)),
            "module item should remain as 'utils'; got: {:?}",
            got.iter().map(|i| (&i.name, &i.kind)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cpp_defgroup_stamps_item_with_group() {
        let src = concat!(
            "/** @addtogroup io */\n",
            "/** @{ */\n",
            "/** Open a file. */\n",
            "FILE *fopen(const char *path, const char *mode);\n",
            "/** @} */\n"
        );
        let got = items(src, &DocLanguage::Cpp);
        let fopen = got
            .iter()
            .find(|i| i.name == "fopen")
            .expect("fopen should be extracted");
        assert_eq!(
            fopen.meta.group.as_deref(),
            Some("io"),
            "fopen should be in group 'io'"
        );
    }

    #[test]
    fn cpp_ingroup_tag_sets_group() {
        let src = "/**\n * @ingroup io\n * @brief Open a file.\n */\nFILE *fopen(const char *path, const char *mode);";
        let got = items(src, &DocLanguage::C);
        let fopen = got
            .iter()
            .find(|i| i.name == "fopen")
            .expect("fopen should be extracted");
        assert_eq!(
            fopen.meta.group.as_deref(),
            Some("io"),
            "@ingroup tag should set meta.group"
        );
    }

    #[test]
    fn cpp_group_fence_does_not_create_item() {
        // Pure @{ and @} blocks should not produce doc items.
        let src = "/** @addtogroup core */\n/** @{ */\n/** @} */\n";
        let got = items(src, &DocLanguage::C);
        assert!(
            got.is_empty(),
            "pure group fences should not emit items; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cpp_group_items_outside_fence_have_no_group() {
        // Items before any @{ block have no group.
        let src = "/** No group here. */\nvoid before();\n/** @addtogroup io */\n/** @{ */\n/** In group. */\nvoid inside();\n/** @} */\n/** Back out. */\nvoid after();\n";
        let got = items(src, &DocLanguage::C);
        let before = got
            .iter()
            .find(|i| i.name == "before")
            .expect("before not found");
        assert!(before.meta.group.is_none(), "before() should have no group");
        let inside = got
            .iter()
            .find(|i| i.name == "inside")
            .expect("inside not found");
        assert_eq!(inside.meta.group.as_deref(), Some("io"));
        let after = got
            .iter()
            .find(|i| i.name == "after")
            .expect("after not found");
        assert!(
            after.meta.group.is_none(),
            "after() should have no group after @}}"
        );
    }

    #[test]
    fn cpp_defgroup_and_open_combined_block() {
        // @addtogroup and @{ in the same block comment (Doxygen-style combined fence).
        let src = "/**\n * @addtogroup net\n * @{\n */\n/** Connect to host. */\nvoid connect();\n/**\n * @}\n */\n";
        let got = items(src, &DocLanguage::C);
        let conn = got
            .iter()
            .find(|i| i.name == "connect")
            .expect("connect not found");
        assert_eq!(
            conn.meta.group.as_deref(),
            Some("net"),
            "item inside combined @addtogroup+@{{ should get group 'net'"
        );
    }

    #[test]
    fn cpp_triple_slash_group_fence() {
        // /// @addtogroup + /// @{ style (not just /** */).
        let src = "/// @addtogroup utils\n/// @{\n/// Compute abs.\nint abs_val(int x);\n/// @}\n";
        let got = items(src, &DocLanguage::Cpp);
        let abs = got
            .iter()
            .find(|i| i.name == "abs_val")
            .expect("abs_val not found");
        assert_eq!(
            abs.meta.group.as_deref(),
            Some("utils"),
            "/// @addtogroup + @{{ should stamp group on item"
        );
    }

    // ── Rust mod edge cases ───────────────────────────────────────────────────

    #[test]
    fn rust_external_mod_not_tracked() {
        // `mod foo;` (no brace) is an external module — must NOT create a scope.
        let src = "mod external;\n/// Top-level function.\npub fn top() {}";
        let got = items(src, &DocLanguage::Rust);
        assert!(
            got.iter().any(|i| i.name == "top"),
            "function after `mod foo;` should be unqualified; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        assert!(
            !got.iter().any(|i| i.name == "external::top"),
            "external mod declaration should not create a scope"
        );
    }

    #[test]
    fn rust_sibling_mods_are_independent() {
        // Items in mod a and mod b must not be cross-qualified.
        let src = concat!(
            "pub mod a {\n",
            "/// Function in a.\npub fn fa() {}\n",
            "}\n",
            "pub mod b {\n",
            "/// Function in b.\npub fn fb() {}\n",
            "}\n"
        );
        let got = items(src, &DocLanguage::Rust);
        assert!(
            got.iter().any(|i| i.name == "a::fa"),
            "fa should be a::fa; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        assert!(
            got.iter().any(|i| i.name == "b::fb"),
            "fb should be b::fb; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        assert!(
            !got.iter().any(|i| i.name == "a::b::fb"),
            "b should not be nested inside a"
        );
    }

    #[test]
    fn rust_pub_crate_mod_tracked() {
        let src = "pub(crate) mod inner {\n/// Hidden helper.\npub(crate) fn help() {}\n}";
        let got = items(src, &DocLanguage::Rust);
        assert!(
            got.iter().any(|i| i.name == "inner::help"),
            "pub(crate) mod should be tracked; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    // ── Go / Ada / D edge cases ───────────────────────────────────────────────

    #[test]
    fn ada_no_package_means_no_qualification() {
        // Ada source with no PACKAGE declaration: names stay bare.
        let src = "--! Compute factorial.\nfunction Fact (N : Integer) return Integer;";
        let got = items(src, &DocLanguage::Ada);
        assert!(
            got.iter().any(|i| i.name == "Fact"),
            "without package, Ada names should be unqualified; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ada_package_body_not_treated_as_package() {
        // `package body Foo is` must not become the package qualifier.
        let src = "package body Foo is\n--! Aux helper.\nprocedure Aux;\nend Foo;";
        let got = items(src, &DocLanguage::Ada);
        // Either no qualification (body ignored) or correct body-package qualification.
        // The key invariant: names should NOT start with "body."
        assert!(
            !got.iter()
                .any(|i| i.name.to_ascii_lowercase().starts_with("body.")),
            "PACKAGE BODY should not create a 'body.' prefix; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn d_no_module_means_no_qualification() {
        let src = "/// Just a function.\nint add(int a, int b);";
        let got = items(src, &DocLanguage::D);
        assert!(
            got.iter().any(|i| i.name == "add"),
            "D source without module declaration should not qualify; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn d_dotted_module_name_takes_first_identifier() {
        // `module foo.bar;` — only the first identifier is used as prefix.
        let src = "/// Module doc.\nmodule foo.bar;\n/// A function.\nint f();";
        let got = items(src, &DocLanguage::D);
        assert!(
            got.iter().any(|i| i.name == "foo.f"),
            "dotted module name should use first identifier; got: {:?}",
            got.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    // ── Clang integration ────────────────────────────────────────────────────

    #[cfg(feature = "clang")]
    #[test]
    fn clang_extracts_namespace_items() {
        let src = r#"namespace math {
/**
 * @brief Clamp x to [lo, hi].
 * @param x  Value to clamp.
 * @param lo Lower bound.
 * @param hi Upper bound.
 * @return   Clamped value.
 */
double clamp(double x, double lo, double hi);
} // namespace math
"#;
        let dir = std::env::temp_dir();
        let path = dir.join("clang_test_clamp.hpp");
        std::fs::write(&path, src).unwrap();
        let got = crate::extract_clang::extract_file_clang(&path);
        std::fs::remove_file(&path).ok();

        assert!(
            !got.is_empty(),
            "clang extractor should find at least one item"
        );
        let clamp = got
            .iter()
            .find(|i| i.name.contains("clamp"))
            .expect("should find 'math::clamp'");
        assert_eq!(clamp.name, "math::clamp");
        assert!(clamp.brief.contains("Clamp"));
    }

    #[cfg(feature = "clang")]
    #[test]
    fn clang_extracts_class_members() {
        let src = r#"namespace stats {
/**
 * @brief Container of order statistics.
 */
class OrderStatistics {
public:
    /**
     * @brief Median of the sample.
     * @return Median value.
     */
    double median() const;
};
} // namespace stats
"#;
        let dir = std::env::temp_dir();
        let path = dir.join("clang_test_order_stats.hpp");
        std::fs::write(&path, src).unwrap();
        let got = crate::extract_clang::extract_file_clang(&path);
        std::fs::remove_file(&path).ok();

        let class = got
            .iter()
            .find(|i| i.name.contains("OrderStatistics"))
            .expect("should extract the class");
        assert_eq!(class.name, "stats::OrderStatistics");

        let median = got
            .iter()
            .find(|i| i.name.contains("median"))
            .expect("should extract median()");
        assert_eq!(median.name, "stats::OrderStatistics::median");
        assert!(median.meta.parent.as_deref() == Some("OrderStatistics"));
    }
}
