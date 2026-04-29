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
    DocSet { items, source_root: dir.to_path_buf() }
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

    while i < lines.len() {
        let t = lines[i].trim();

        // Block comment openers: /** or /*!
        if (t.starts_with("/**") && !t.starts_with("/***/"))
            || t.starts_with("/*!")
        {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_c_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, lang.clone());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        // Line comment: /// (but not ////)
        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_c_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, lang.clone());
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        i += 1;
    }
    items
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
        // typedef <type> <alias>;  — last word before semicolon
        let name = t.trim_end_matches(';')
            .split_whitespace().last().unwrap_or("").to_string();
        return (name, DocKind::Typedef);
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
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Rust);
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        if t.starts_with("/**") && !t.starts_with("/***/") {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_rust_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Rust);
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

    while i < lines.len() {
        if lines[i].trim().starts_with("!>") {
            let (block, end) = collect_fortran_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_fortran_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Fortran);
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    items
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
    // Ordered so "MODULE SUBROUTINE" matches before "MODULE"
    if up.starts_with("MODULE SUBROUTINE ") { return (ci_ident_after(t, "module subroutine "), DocKind::Subroutine); }
    if up.starts_with("MODULE FUNCTION ")   { return (ci_ident_after(t, "module function "),   DocKind::Function); }
    if up.starts_with("SUBROUTINE ")        { return (ci_ident_after(t, "subroutine "),         DocKind::Subroutine); }
    if up.starts_with("FUNCTION ")          { return (ci_ident_after(t, "function "),           DocKind::Function); }
    if up.starts_with("MODULE ") && !up.starts_with("MODULE PROCEDURE") {
        return (ci_ident_after(t, "module "), DocKind::Module);
    }
    if up.contains("::") {
        // TYPE :: Foo  or  TYPE(kind) :: Foo
        if let Some(after) = t.split("::").nth(1) {
            return (first_ident(after.trim()), DocKind::Struct);
        }
    }
    (String::new(), DocKind::Unknown)
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
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::D);
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        if t.starts_with("/**") && !t.starts_with("/***/") {
            let (block, end) = collect_c_block(&lines, i);
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::D);
            if item_has_content(&item) { items.push(item); }
            i = end + 1;
            continue;
        }

        if t.starts_with("///") && !t.starts_with("////") {
            let (block, end) = collect_line_block(&lines, i, "///");
            let sym = next_non_blank(&lines, end + 1);
            let (name, kind) = detect_d_symbol(sym);
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::D);
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
            let item = build_item(block, name, kind, file, i + 1, DocLanguage::Ada);
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

fn item_has_content(item: &DocItem) -> bool {
    !item.brief.is_empty() || !item.body.is_empty() || !item.tags.is_empty()
}

fn build_item(
    raw_lines: Vec<String>,
    name: String,
    kind: DocKind,
    file: &Path,
    line: usize,
    lang: DocLanguage,
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

    DocItem { name, kind, brief, body, tags, file: file.to_path_buf(), line, lang }
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
}
