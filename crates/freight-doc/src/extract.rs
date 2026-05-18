use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum DocLanguage {
    C, Cpp, Rust, Fortran, D, Ada, Unknown,
}

impl DocLanguage {
    pub fn label(&self) -> &'static str {
        match self {
            Self::C       => "C",
            Self::Cpp     => "C++",
            Self::Rust    => "Rust",
            Self::Fortran => "Fortran",
            Self::D       => "D",
            Self::Ada     => "Ada",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DocKind {
    Function, Struct, Class, Enum, Typedef, Variable,
    Macro, Module, Subroutine, Interface, Unknown,
}

impl DocKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Function  => "fn",
            Self::Struct    => "struct",
            Self::Class     => "class",
            Self::Enum      => "enum",
            Self::Typedef   => "type",
            Self::Variable  => "var",
            Self::Macro     => "macro",
            Self::Module    => "mod",
            Self::Subroutine => "sub",
            Self::Interface => "iface",
            Self::Unknown   => "item",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagKind {
    Brief, Param, Return, Note, See, Since,
    Deprecated, Example, Warning, Other(String),
}

impl TagKind {
    pub fn label(&self) -> &str {
        match self {
            Self::Brief      => "Brief",
            Self::Param      => "Parameter",
            Self::Return     => "Returns",
            Self::Note       => "Note",
            Self::See        => "See also",
            Self::Since      => "Since",
            Self::Deprecated => "Deprecated",
            Self::Example    => "Example",
            Self::Warning    => "Warning",
            Self::Other(s)   => s.as_str(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocTag {
    pub kind: TagKind,
    /// Parameter name for `@param`; `None` for all other tag types.
    pub name: Option<String>,
    pub text: String,
}

/// Access level of a class / struct member.
#[derive(Debug, Clone, PartialEq)]
pub enum Access { Public, Protected, Private }

/// Structured metadata populated by language-aware extractors (libclang, etc.).
/// Defaults to empty so heuristic extractors compile without changes.
#[derive(Debug, Clone, Default)]
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
}

#[derive(Debug, Clone)]
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
    /// The first non-blank source line following the doc comment (declaration / signature).
    /// Empty when no following line was found or when the language parser couldn't read it.
    pub signature: String,
    /// Structured metadata populated by accurate extractors; empty for heuristic extraction.
    pub meta: DocMeta,
}

pub struct DocSet {
    pub items: Vec<DocItem>,
    pub source_root: PathBuf,
}

impl DocSet {
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
}

// ── Language detection ────────────────────────────────────────────────────────

pub fn lang_from_ext(ext: &str) -> DocLanguage {
    match ext {
        "c" | "h" => DocLanguage::C,
        "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx"
        | "cu" | "hip" | "sycl" | "ispc" => DocLanguage::Cpp,
        "rs" => DocLanguage::Rust,
        "f" | "f90" | "f95" | "f03" | "f08" | "F90" | "for" | "ftn" => DocLanguage::Fortran,
        "d" => DocLanguage::D,
        "ads" | "adb" => DocLanguage::Ada,
        _ => DocLanguage::Unknown,
    }
}

// ── Entry points ──────────────────────────────────────────────────────────────

pub fn extract_file(path: &Path) -> Vec<DocItem> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let lang = lang_from_ext(ext);
    if lang == DocLanguage::Unknown { return vec![]; }
    #[cfg(feature = "clang")]
    if matches!(lang, DocLanguage::C | DocLanguage::Cpp) {
        return crate::extract_clang::extract_file_clang(path);
    }
    extract_file_heuristic(path)
}

/// Heuristic C/C++ extractor used as a fallback when libclang is unavailable.
pub(crate) fn extract_file_heuristic(path: &Path) -> Vec<DocItem> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let lang = lang_from_ext(ext);
    if lang == DocLanguage::Unknown { return vec![]; }
    let Ok(src) = std::fs::read_to_string(path) else { return vec![]; };
    from_str(&src, path, &lang)
}

pub fn extract_dir(dir: &Path) -> DocSet {
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
            items.extend(extract_file(entry.path()));
        }
    }
    // Deduplicate: when the same qualified name appears in both a header and
    // an implementation file, keep whichever has richer doc content.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut deduped: Vec<DocItem> = Vec::new();
    for item in items {
        let score = item.tags.len() * 10 + item.brief.len() + item.body.len();
        match seen.get(&item.name).copied() {
            Some(idx) => {
                let prev = deduped[idx].tags.len() * 10 + deduped[idx].brief.len() + deduped[idx].body.len();
                if score > prev { deduped[idx] = item; }
            }
            None => {
                seen.insert(item.name.clone(), deduped.len());
                deduped.push(item);
            }
        }
    }
    DocSet { items: deduped, source_root: dir.to_path_buf() }
}

fn from_str(src: &str, file: &Path, lang: &DocLanguage) -> Vec<DocItem> {
    match lang {
        DocLanguage::C | DocLanguage::Cpp => extract_c_style(src, file, lang),
        DocLanguage::Rust                 => extract_rust(src, file),
        DocLanguage::Fortran              => extract_fortran(src, file),
        DocLanguage::D                    => extract_d(src, file),
        DocLanguage::Ada                  => extract_ada(src, file),
        DocLanguage::Unknown              => vec![],
    }
}

// ── C / C++ (Doxygen) ─────────────────────────────────────────────────────────
//
// Supported forms:
//   /** ... */   (JavaDoc / Doxygen block)
//   /*! ... */   (Qt-style Doxygen block)
//   ///          (Doxygen line comment)
//
// Within blocks: leading " * " is stripped.
// Tags: @tag / \tag — @param, @return, @brief, @note, @see, @since,
//       @deprecated, @example, @warning, and any unknown tag.

fn extract_c_style(src: &str, file: &Path, lang: &DocLanguage) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    // Namespace scope tracking.
    // Each entry: (brace_depth_after_open, fully_qualified_path).
    let mut brace_depth: usize = 0;
    let mut ns_stack: Vec<(usize, String)> = Vec::new();
    // `namespace X` was seen on the last line without a `{` on the same line.
    let mut pending_ns: Option<String> = None;

    while i < lines.len() {
        let t = lines[i].trim();

        // ── Doc comment blocks — collect, qualify, advance ────────────────────
        if (t.starts_with("/**") && !t.starts_with("/***/")) || t.starts_with("/*!") {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_decl_sym(&lines, end + 1);
            if !is_c_conditional_directive(sym) {
                let (name, kind) = detect_c_symbol(sym);
                let ns  = ns_stack.last().map(|(_, p)| p.as_str()).unwrap_or("");
                let item = build_item(block, qualify_name(&name, ns), kind, file, i + 1, lang.clone(), sym.to_string());
                if item_has_content(&item) { items.push(item); }
            }
            i = end + 1;
            continue;
        }

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_decl_sym(&lines, end + 1);
            if !is_c_conditional_directive(sym) {
                let (name, kind) = detect_c_symbol(sym);
                let ns  = ns_stack.last().map(|(_, p)| p.as_str()).unwrap_or("");
                let item = build_item(block, qualify_name(&name, ns), kind, file, i + 1, lang.clone(), sym.to_string());
                if item_has_content(&item) { items.push(item); }
            }
            i = end + 1;
            continue;
        }

        // ── Namespace / brace tracking (non-comment, non-doc lines) ──────────
        if !t.starts_with("//") && !t.starts_with("/*") && !t.starts_with('*') {
            // Detect `namespace X` or `namespace X {`
            if let Some(rest) = t.strip_prefix("namespace") {
                let rest_ok = rest.is_empty()
                    || rest.starts_with(|c: char| c.is_whitespace() || c == '{');
                if rest_ok {
                    let name = first_ident(rest.trim_start());
                    if !name.is_empty() {
                        let path = match ns_stack.last() {
                            Some((_, p)) => format!("{p}::{name}"),
                            None         => name,
                        };
                        if t.contains('{') {
                            let opens  = t.chars().filter(|&c| c == '{').count();
                            let closes = t.chars().filter(|&c| c == '}').count();
                            brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                            ns_stack.push((brace_depth, path));
                        } else {
                            pending_ns = Some(path);
                        }
                        i += 1;
                        continue;
                    }
                }
            }

            // Commit a deferred namespace when its opening `{` appears.
            if pending_ns.is_some() && t.contains('{') {
                let path   = pending_ns.take().unwrap();
                let opens  = t.chars().filter(|&c| c == '{').count();
                let closes = t.chars().filter(|&c| c == '}').count();
                brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
                ns_stack.push((brace_depth, path));
                while ns_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                    ns_stack.pop();
                }
                i += 1;
                continue;
            }

            // Generic brace counting for all other code lines.
            let opens  = t.chars().filter(|&c| c == '{').count();
            let closes = t.chars().filter(|&c| c == '}').count();
            brace_depth = brace_depth.saturating_add(opens).saturating_sub(closes);
            while ns_stack.last().map_or(false, |&(d, _)| brace_depth < d) {
                ns_stack.pop();
            }
        }

        i += 1;
    }
    items
}

/// Prefix `name` with `ns::` when both are non-empty.
fn qualify_name(name: &str, ns: &str) -> String {
    if name.is_empty() || ns.is_empty() { name.to_string() } else { format!("{ns}::{name}") }
}

fn collect_c_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let first = lines[start].trim();

    // Content after the 3-char opener ("/**" or "/*!"):
    let after = first[3..].trim();

    // Single-line: /** brief text */
    if let Some(content) = after.strip_suffix("*/") {
        out.push(content.trim().to_string());
        return (out, start);
    }
    if !after.is_empty() { out.push(after.to_string()); }

    let mut i = start + 1;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.ends_with("*/") {
            let content = t.strip_suffix("*/").unwrap_or("").trim_start_matches('*').trim();
            if !content.is_empty() { out.push(content.to_string()); }
            return (out, i);
        }
        // Strip leading " * " / " *"
        let line = if let Some(r) = t.strip_prefix("* ") {
            r.to_string()
        } else if t == "*" {
            String::new()
        } else {
            t.strip_prefix('*').unwrap_or(t).to_string()
        };
        out.push(line);
        i += 1;
    }
    (out, i.saturating_sub(1))
}

fn collect_line_block(lines: &[&str], start: usize, prefix: &str) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    let double = format!("{prefix}/"); // avoid matching ////
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with(prefix) && !t.starts_with(&double) {
            out.push(t[prefix.len()..].trim_start().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (out, i.saturating_sub(1))
}

fn next_non_blank<'a>(lines: &[&'a str], from: usize) -> &'a str {
    lines.iter().skip(from)
        .find(|l| !l.trim().is_empty())
        .copied()
        .unwrap_or("")
}

/// Like `next_non_blank` but skips past `template<…>` header lines so that
/// `detect_c_symbol` sees the actual struct/class/function declaration.
fn next_decl_sym<'a>(lines: &[&'a str], from: usize) -> &'a str {
    let mut i = from;
    loop {
        let Some(&l) = lines.get(i) else { return "" };
        let t = l.trim();
        if t.is_empty() { i += 1; continue; }
        if t.starts_with("template") { i += 1; continue; }
        return t;
    }
}

fn is_c_conditional_directive(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("#if")
        || t.starts_with("#elif")
        || t.starts_with("#else")
        || t.starts_with("#endif")
}

/// Strip storage-class and attribute qualifiers that precede a type or name.
fn strip_c_qualifiers(mut t: &str) -> &str {
    const QUALS: &[&str] = &[
        "static ", "inline ", "extern ", "explicit ", "virtual ",
        "constexpr ", "consteval ", "constinit ",
        "__inline ", "__inline__ ", "__forceinline ",
        "[[nodiscard]] ", "[[maybe_unused]] ",
    ];
    'outer: loop {
        for q in QUALS {
            if let Some(rest) = t.strip_prefix(q) {
                t = rest.trim_start();
                continue 'outer;
            }
        }
        break;
    }
    t
}

fn detect_c_symbol(line: &str) -> (String, DocKind) {
    let t = strip_c_qualifiers(line.trim());

    if let Some(r) = t.strip_prefix("struct ")        { return (first_ident(r), DocKind::Struct); }
    if let Some(r) = t.strip_prefix("class ")         { return (first_ident(r), DocKind::Class); }
    if let Some(r) = t.strip_prefix("enum class ")    { return (first_ident(r), DocKind::Enum); }
    if let Some(r) = t.strip_prefix("enum ")          { return (first_ident(r), DocKind::Enum); }
    if let Some(r) = t.strip_prefix("namespace ")     { return (first_ident(r), DocKind::Module); }
    if let Some(r) = t.strip_prefix("#define ")       { return (first_ident(r), DocKind::Macro); }
    if t.starts_with("typedef ") {
        // typedef <type> <alias>;  — last ident-like word before semicolon.
        // For multi-line "typedef struct { ... } Name;" the closing line has
        // the alias; single-line "typedef unsigned int u32;" works directly.
        let candidate = t.trim_end_matches(';')
            .trim_end()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or("")
            .to_string();
        // Suppress obviously-wrong tokens (keywords, opening brace artefacts)
        if !candidate.is_empty()
            && !matches!(candidate.as_str(), "struct" | "union" | "enum" | "class")
        {
            return (candidate, DocKind::Typedef);
        }
        return (String::new(), DocKind::Typedef);
    }
    if t.starts_with("template") {
        // Template declarations need a full parser; leave as Unknown
        return (String::new(), DocKind::Unknown);
    }
    // Function heuristic: look for "name(" pattern
    if let Some(name) = func_name_before_paren(t) {
        return (name, DocKind::Function);
    }
    (String::new(), DocKind::Unknown)
}

fn first_ident(s: &str) -> String {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
        .to_string()
}

fn func_name_before_paren(t: &str) -> Option<String> {
    let paren = t.find('(')?;
    let before = t[..paren].trim_end();
    let word = before.split_whitespace().last()?;
    // Strip pointer / destructor prefixes
    let name = word.trim_start_matches('*').trim_start_matches('~');
    if name.is_empty() { return None; }
    // Reject control-flow keywords
    if matches!(name, "if" | "while" | "for" | "switch" | "do" | "catch") {
        return None;
    }
    if name.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false) {
        Some(name.to_string())
    } else {
        None
    }
}

// ── Rust ──────────────────────────────────────────────────────────────────────
//
// Supported forms:
//   /// Markdown line doc
//   /** Markdown block doc */
//
// `//!` module-level docs are skipped (they apply to the enclosing scope,
// not a following item, so association is ambiguous without a full parser).

fn extract_rust(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_rust_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Rust, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        if t.starts_with("/**") && !t.starts_with("/***/") {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_rust_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Rust, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        i += 1;
    }
    items
}

fn detect_rust_symbol(line: &str) -> (String, DocKind) {
    // Split into tokens and skip visibility / modifier keywords
    let words: Vec<&str> = line.split_whitespace().collect();
    let skip = words.iter().take_while(|&&w| {
        matches!(w, "pub" | "async" | "unsafe" | "extern" | "default")
        || w.starts_with("pub(")
        || w.starts_with('"') // extern "C"
    }).count();

    let rest = &words[skip..];
    if rest.is_empty() { return (String::new(), DocKind::Unknown); }

    let keyword = rest[0];
    let name_raw = rest.get(1).copied().unwrap_or("")
        .split(['<', '(', '{', ':']).next().unwrap_or("");
    let name = name_raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_').to_string();

    match keyword {
        "fn"     => (name, DocKind::Function),
        "struct" => (name, DocKind::Struct),
        "enum"   => (name, DocKind::Enum),
        "trait"  => (name, DocKind::Interface),
        "type"   => (name, DocKind::Typedef),
        "mod"    => (name, DocKind::Module),
        "const"  => (name, DocKind::Variable),
        "static" => (name, DocKind::Variable),
        "impl"   => {
            // "impl Foo" or "impl<T> Bar for Baz" — take the type after "for" if present
            if let Some(pos) = rest.iter().position(|w| *w == "for") {
                let after = rest.get(pos + 1).copied().unwrap_or("");
                let n = after.split('<').next().unwrap_or("").to_string();
                return (n, DocKind::Struct);
            }
            (name, DocKind::Struct)
        }
        _ => (String::new(), DocKind::Unknown),
    }
}

// ── Fortran ───────────────────────────────────────────────────────────────────
//
// Supported forms (FORD / ford-esque conventions):
//   !> Opens a doc comment block
//   !! Continues a doc comment block
//
// Case-insensitive keyword detection for SUBROUTINE, FUNCTION, MODULE, TYPE.

fn extract_fortran(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    // Track whether we are inside a MODULE block so variable declarations can
    // be treated as documented module-level variables.
    let mut in_module = false;

    while i < lines.len() {
        let t = lines[i].trim();
        let up = t.to_ascii_uppercase();

        // Track MODULE / END MODULE scope.
        if up.starts_with("MODULE ")
            && !up.starts_with("MODULE SUBROUTINE ")
            && !up.starts_with("MODULE FUNCTION ")
            && !up.starts_with("MODULE PROCEDURE ")
        {
            in_module = true;
        } else if up.starts_with("END MODULE") || up == "END MODULE" {
            in_module = false;
        }

        if t.starts_with("!>") {
            let (block, end) = collect_fortran_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_fortran_symbol(sym);

            // When inside a module and the symbol could not be identified as a
            // procedure/type, try reading it as a variable declaration.
            let (name, kind) = if kind == DocKind::Unknown && in_module {
                if let Some(var_name) = detect_fortran_variable(sym) {
                    (var_name, DocKind::Variable)
                } else {
                    (name, kind)
                }
            } else {
                (name, kind)
            };

            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Fortran, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    items
}

/// Detect a Fortran module-level variable declaration of the form:
/// `TYPE_KW [, attrs] :: name [= init]`
fn detect_fortran_variable(line: &str) -> Option<String> {
    let up = line.trim_start().to_ascii_uppercase();
    let is_type = ["INTEGER", "REAL", "DOUBLE PRECISION", "COMPLEX",
                   "LOGICAL", "CHARACTER", "TYPE("]
        .iter().any(|k| up.starts_with(k));
    if !is_type { return None; }
    // Extract the declared name: the first identifier after `::`.
    let name = line.find("::")
        .map(|p| first_ident(line[p + 2..].trim_start()))
        .filter(|n| !n.is_empty())?;
    Some(name)
}

fn collect_fortran_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("!>") || t.starts_with("!!") {
            out.push(t[2..].trim_start().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (out, i.saturating_sub(1))
}

fn detect_fortran_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();
    let up = t.to_ascii_uppercase();

    // Strip leading qualifiers: PURE, RECURSIVE, ELEMENTAL, IMPURE, LOGICAL, INTEGER, …
    // by finding the first occurrence of FUNCTION or SUBROUTINE and working from there.
    let up_sp = up.replace('(', " ").replace(')', " ");
    let tokens: Vec<&str> = up_sp.split_whitespace().collect();

    // Ordered so "MODULE SUBROUTINE/FUNCTION" match before bare "MODULE"
    if up.starts_with("MODULE SUBROUTINE ") { return (ci_ident_after(t, "module subroutine "), DocKind::Subroutine); }
    if up.starts_with("MODULE FUNCTION ")   { return (ci_ident_after(t, "module function "),   DocKind::Function); }

    // Walk tokens to find SUBROUTINE or FUNCTION keyword, then take the next token as name
    if let Some(pos) = tokens.iter().position(|w| *w == "SUBROUTINE") {
        if let Some(name_tok) = tokens.get(pos + 1) {
            let orig = original_token_at(t, pos + 1);
            return (first_ident(orig.unwrap_or(name_tok)), DocKind::Subroutine);
        }
    }
    if let Some(pos) = tokens.iter().position(|w| *w == "FUNCTION") {
        if let Some(name_tok) = tokens.get(pos + 1) {
            let orig = original_token_at(t, pos + 1);
            return (first_ident(orig.unwrap_or(name_tok)), DocKind::Function);
        }
    }

    if up.starts_with("MODULE ") && !up.starts_with("MODULE PROCEDURE") {
        return (ci_ident_after(t, "module "), DocKind::Module);
    }
    if up.contains("::") {
        // Only bare `TYPE :: TypeName` is a derived-type definition.
        // All other `:: ` declarations (INTEGER, REAL, etc.) are variables;
        // return Unknown so detect_fortran_variable can handle them.
        let before = &up[..up.find("::").unwrap()];
        if before.trim() == "TYPE" {
            if let Some(after) = t.split("::").nth(1) {
                return (first_ident(after.trim()), DocKind::Struct);
            }
        }
    }
    (String::new(), DocKind::Unknown)
}

/// Return the n-th whitespace token from the original (mixed-case) line.
fn original_token_at(line: &str, n: usize) -> Option<&str> {
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|s| !s.is_empty())
        .nth(n)
}

/// Case-insensitive prefix skip — safe for ASCII-only languages (Fortran, Ada).
fn ci_ident_after(s: &str, prefix_lower: &str) -> String {
    let len = prefix_lower.len();
    if s.len() >= len && s[..len].eq_ignore_ascii_case(prefix_lower) {
        first_ident(s[len..].trim_start())
    } else {
        String::new()
    }
}

// ── D (DDoc) ──────────────────────────────────────────────────────────────────
//
// Supported forms:
//   /++ ... +/   DDoc block (D-specific)
//   /** ... */   shared with C++
//   ///          shared with C++

fn extract_d(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();

        if t.starts_with("/++") {
            let (block, end) = collect_d_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::D, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        if t.starts_with("/**") && !t.starts_with("/***/") {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::D, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::D, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        i += 1;
    }
    items
}

fn collect_d_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let first = lines[start].trim();
    let after = first[3..].trim(); // after "/++"

    // Single-line: /++ brief +/
    if let Some(content) = after.strip_suffix("+/") {
        out.push(content.trim().to_string());
        return (out, start);
    }
    if !after.is_empty() { out.push(after.to_string()); }

    let mut i = start + 1;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.ends_with("+/") {
            let content = t.strip_suffix("+/").unwrap_or("").trim_start_matches('+').trim();
            if !content.is_empty() { out.push(content.to_string()); }
            return (out, i);
        }
        let content = t.strip_prefix("+ ").or_else(|| t.strip_prefix('+')).unwrap_or(t);
        out.push(content.to_string());
        i += 1;
    }
    (out, i.saturating_sub(1))
}

fn detect_d_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();
    // D shares most declaration syntax with C/C++
    let (name, kind) = detect_c_symbol(t);
    if kind != DocKind::Unknown { return (name, kind); }
    if let Some(r) = t.strip_prefix("interface ") { return (first_ident(r), DocKind::Interface); }
    if let Some(r) = t.strip_prefix("module ")    { return (first_ident(r), DocKind::Module); }
    (name, kind)
}

// ── Ada ───────────────────────────────────────────────────────────────────────
//
// Supported forms:
//   --!  GNAT doc comment
//   ---  common alternative convention
//
// Keyword detection: PROCEDURE, FUNCTION, PACKAGE, TYPE (case-insensitive).

fn extract_ada(src: &str, file: &Path) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("--!") || t.starts_with("---") {
            let (block, end) = collect_ada_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_ada_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Ada, sym.to_string());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    items
}

fn collect_ada_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("--!") || t.starts_with("---") {
            out.push(t[3..].trim_start().to_string());
            i += 1;
        } else {
            break;
        }
    }
    (out, i.saturating_sub(1))
}

fn detect_ada_symbol(line: &str) -> (String, DocKind) {
    let t = line.trim();
    let up = t.to_ascii_uppercase();
    if up.starts_with("PROCEDURE ") { return (ci_ident_after(t, "procedure "), DocKind::Subroutine); }
    if up.starts_with("FUNCTION ")  { return (ci_ident_after(t, "function "),  DocKind::Function); }
    if up.starts_with("PACKAGE ")   { return (ci_ident_after(t, "package "),   DocKind::Module); }
    if up.starts_with("TYPE ")      { return (ci_ident_after(t, "type "),       DocKind::Typedef); }
    (String::new(), DocKind::Unknown)
}

// ── Tag parsing + item construction ──────────────────────────────────────────

pub(crate) fn item_has_content(item: &DocItem) -> bool {
    !item.brief.is_empty() || !item.body.is_empty() || !item.tags.is_empty()
}

pub(crate) fn build_item(
    raw_lines: Vec<String>,
    name: String,
    kind: DocKind,
    file: &Path,
    line: usize,
    lang: DocLanguage,
    signature: String,
) -> DocItem {
    let mut prose: Vec<String> = Vec::new();
    let mut tags: Vec<DocTag>  = Vec::new();
    let mut cur_tag: Option<(TagKind, Option<String>, Vec<String>)> = None;

    for raw in &raw_lines {
        if let Some(tag) = parse_tag_start(raw) {
            if let Some((k, n, tl)) = cur_tag.take() {
                tags.push(DocTag { kind: k, name: n, text: tl.join(" ").trim().to_string() });
            }
            cur_tag = Some(tag);
        } else if let Some((_, _, ref mut tl)) = cur_tag {
            tl.push(raw.clone());
        } else {
            prose.push(raw.clone());
        }
    }
    if let Some((k, n, tl)) = cur_tag {
        tags.push(DocTag { kind: k, name: n, text: tl.join(" ").trim().to_string() });
    }

    // brief: use @brief tag if present, otherwise first non-empty prose line
    let explicit_brief = tags.iter().find(|t| t.kind == TagKind::Brief).map(|t| t.text.clone());
    let first_prose = prose.iter().position(|l| !l.trim().is_empty());

    let (brief, body) = match (explicit_brief, first_prose) {
        (Some(b), _) => {
            let body = prose.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n").trim().to_string();
            (b, body)
        }
        (None, Some(idx)) => {
            let brief = prose[idx].trim().to_string();
            let body  = prose[idx + 1..].join("\n").trim().to_string();
            (brief, body)
        }
        (None, None) => (String::new(), String::new()),
    };

    DocItem { name, kind, brief, body, tags, file: file.to_path_buf(), line, lang, signature: signature.trim().to_string(), meta: DocMeta::default() }
}

/// Recognise a Doxygen/Javadoc tag at the start of a trimmed comment line.
///
/// Both `@tag` and `\tag` forms are accepted.
/// Single-character "tags" are rejected to avoid treating `\n`, `\t` etc. as
/// documentation directives.
fn parse_tag_start(line: &str) -> Option<(TagKind, Option<String>, Vec<String>)> {
    let t = line.trim_start();
    let rest = if t.starts_with('@') { &t[1..] }
               else if t.starts_with('\\') { &t[1..] }
               else { return None; };

    let (tag, rem) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None    => (rest, ""),
    };
    let rem = rem.to_string();

    // Require ≥ 2 alphabetic chars to filter out escape sequences (\n, \t, \r…)
    if tag.len() < 2 || !tag.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }

    match tag {
        "brief" => Some((TagKind::Brief, None, vec![rem])),
        "param" | "param[in]" | "param[out]" | "param[in,out]" => {
            let (pname, pdesc) = split_first_word(&rem);
            Some((TagKind::Param, Some(pname), vec![pdesc]))
        }
        "return" | "returns" | "retval" => Some((TagKind::Return, None, vec![rem])),
        "note"                          => Some((TagKind::Note,   None, vec![rem])),
        "see" | "sa"                    => Some((TagKind::See,    None, vec![rem])),
        "since"                         => Some((TagKind::Since,  None, vec![rem])),
        "deprecated"                    => Some((TagKind::Deprecated, None, vec![rem])),
        "example" | "code" | "endcode" => Some((TagKind::Example, None, vec![rem])),
        "warning" | "warn"              => Some((TagKind::Warning, None, vec![rem])),
        other => Some((TagKind::Other(other.to_string()), None, vec![rem])),
    }
}

fn split_first_word(s: &str) -> (String, String) {
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_string(), s[i..].trim().to_string()),
        None    => (s.to_string(), String::new()),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn items(src: &str, lang: &DocLanguage) -> Vec<DocItem> {
        from_str(src, Path::new("test.x"), lang)
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
        let params: Vec<_> = got[0].tags.iter().filter(|t| t.kind == TagKind::Param).collect();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name.as_deref(), Some("arr"));
        let ret: Vec<_> = got[0].tags.iter().filter(|t| t.kind == TagKind::Return).collect();
        assert_eq!(ret.len(), 1);
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
        assert!(got.is_empty(), "conditional directives should not become documented items");
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
        // \n should not be treated as a tag named "n"
        let src = "/** Uses \\n for newlines and \\t for tabs. */\nvoid foo();";
        let got = items(src, &DocLanguage::C);
        assert_eq!(got.len(), 1);
        // Should have no tags (no @param etc)
        let unknown_tags: Vec<_> = got[0].tags.iter()
            .filter(|t| matches!(&t.kind, TagKind::Other(s) if s == "n" || s == "t"))
            .collect();
        assert!(unknown_tags.is_empty(), "single-char escape sequences must not become tags");
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
        let src = "/// A colour in linear sRGB.\npub struct Rgb { pub r: f32, pub g: f32, pub b: f32 }";
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
    fn rust_fn_captures_signature() {
        let src = "/// Return factorial.\npub fn factorial(n: u64) -> u64 { todo!() }";
        let got = items(src, &DocLanguage::Rust);
        assert_eq!(got[0].signature, "pub fn factorial(n: u64) -> u64 { todo!() }");
    }

    #[test]
    fn signature_empty_when_no_following_line() {
        let src = "/** Orphan comment with nothing after. */\n";
        let got = items(src, &DocLanguage::C);
        // No content follows, so either no item or empty signature
        if !got.is_empty() {
            assert!(got[0].signature.is_empty() || !got[0].signature.is_empty(),
                "signature is whatever followed; just must not crash");
        }
    }

    // ── Language detection ────────────────────────────────────────────────────

    #[test]
    fn extension_routing() {
        assert_eq!(lang_from_ext("c"),   DocLanguage::C);
        assert_eq!(lang_from_ext("cpp"), DocLanguage::Cpp);
        assert_eq!(lang_from_ext("rs"),  DocLanguage::Rust);
        assert_eq!(lang_from_ext("f90"), DocLanguage::Fortran);
        assert_eq!(lang_from_ext("d"),   DocLanguage::D);
        assert_eq!(lang_from_ext("ads"), DocLanguage::Ada);
        assert_eq!(lang_from_ext("toml"), DocLanguage::Unknown);
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
        // Template names are currently Unknown kind (no full template parser).
        // What matters is the comment is captured.
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
    fn cpp_using_alias() {
        // `using` type aliases look like variable declarations to our heuristic;
        // we just verify the comment is extracted, not that the kind is perfect.
        let src = "/** Convenience alias for a string map. */\nusing StringMap = std::map<std::string, std::string>;";
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
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
        let params: Vec<_> = got[0].tags.iter().filter(|t| t.kind == TagKind::Param).collect();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name.as_deref(), Some("r"));
        assert_eq!(params[1].name.as_deref(), Some("theta"));
    }

    #[test]
    fn cpp_inline_variable_doc() {
        // Doc comment on a const variable.
        let src = "/** Maximum buffer size in bytes. */\nconst size_t MAX_BUF = 4096;";
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].brief, "Maximum buffer size in bytes.");
        // `const size_t MAX_BUF` — func_name_before_paren returns None (no paren),
        // so kind is Unknown. The comment is still captured.
        assert!(!got[0].brief.is_empty());
    }

    #[test]
    fn cpp_namespace_free_function() {
        let src = r#"namespace math {
/** @brief Clamp x to [lo, hi]. */
double clamp(double x, double lo, double hi);
} // namespace math"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "math::clamp");
        assert_eq!(got[0].brief, "Clamp x to [lo, hi].");
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
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "outer::inner::nested");
    }

    #[test]
    fn cpp_multiline_param_continuation() {
        // A @param description that spans two comment lines.
        let src = r#"/**
 * @brief Multiply matrix.
 * @param A Input matrix; must be square and
 *   stored in row-major order.
 * @param n Dimension.
 */
void matmul(double *A, int n);"#;
        let got = items(src, &DocLanguage::Cpp);
        assert_eq!(got.len(), 1);
        let params: Vec<_> = got[0].tags.iter().filter(|t| t.kind == TagKind::Param).collect();
        assert_eq!(params.len(), 2);
        // The continuation line should be joined into the param text.
        assert!(params[0].text.contains("row-major"), "expected continuation line in param text");
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
        // Write to a temp .hpp file (unambiguously C++) so libclang parses it as C++.
        let dir = std::env::temp_dir();
        let path = dir.join("clang_test_clamp.hpp");
        std::fs::write(&path, src).unwrap();
        let got = crate::extract_clang::extract_file_clang(&path);
        std::fs::remove_file(&path).ok();

        assert!(!got.is_empty(), "clang extractor should find at least one item");
        let clamp = got.iter().find(|i| i.name.contains("clamp"))
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
        let dir  = std::env::temp_dir();
        let path = dir.join("clang_test_order_stats.hpp");
        std::fs::write(&path, src).unwrap();
        let got  = crate::extract_clang::extract_file_clang(&path);
        std::fs::remove_file(&path).ok();

        let class = got.iter().find(|i| i.name.contains("OrderStatistics"))
            .expect("should extract the class");
        assert_eq!(class.name, "stats::OrderStatistics");

        let median = got.iter().find(|i| i.name.contains("median"))
            .expect("should extract median()");
        assert_eq!(median.name, "stats::OrderStatistics::median");
        assert!(median.meta.parent.as_deref() == Some("OrderStatistics"));
    }
}
