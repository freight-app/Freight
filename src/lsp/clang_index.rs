//! libclang-backed TU cache for hover, go-to-definition, and inlay hints.
//!
//! libclang is loaded at runtime via dlopen (clang-sys `runtime` feature).
//! If the library is absent `TuCache::try_new` returns `None` and all callers
//! fall back to the existing text-based paths — no hard dependency at link time.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use clang_sys::*;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: Option<String>,
    pub type_str: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HoverTag {
    pub kind: HoverTagKind,
    pub name: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoverTagKind {
    Param,
    TParam,
    Return,
    Throws,
    Note,
    Warning,
    Example,
    See,
    Since,
    Deprecated,
}

#[derive(Clone, Debug)]
pub struct HoverInfo {
    /// Display name — for functions includes the parameter type list.
    pub display_name: String,
    /// Fully-qualified semantic name when libclang can derive one.
    pub qualified_name: Option<String>,
    /// Full declaration text from libclang's declaration pretty-printer.
    pub pretty_decl: Option<String>,
    /// Return type for functions, declared type for variables, empty for types.
    pub type_str: Option<String>,
    /// Canonical type spelling when it differs from `type_str`.
    pub canonical_type: Option<String>,
    /// Explicit result type for callables.
    pub result_type: Option<String>,
    /// Function/method parameters with names and types.
    pub params: Vec<ParamInfo>,
    /// Brief doc comment from the declaration site.
    pub doc: Option<String>,
    /// Extended doc body, excluding parsed structured tags.
    pub body: Option<String>,
    /// Structured tags parsed from Doxygen-style raw comments.
    pub tags: Vec<HoverTag>,
    /// Absolute path to the file where the symbol is declared.
    pub source_file: Option<PathBuf>,
    /// 1-based line of the declaration.
    pub source_line: Option<u32>,
    /// 1-based column of the declaration.
    pub source_col: Option<u32>,
    /// Cursor kind — reserved for future use (e.g. choose code-block language tag).
    #[allow(dead_code)]
    pub cursor_kind: u32,
}

pub struct DefinitionLocation {
    pub path: PathBuf,
    /// 0-based line.
    pub line: u32,
    /// 0-based column.
    pub col: u32,
}

/// Resolved `#include` / `#import` directive from `clang_getInclusions`.
pub struct InclusionInfo {
    /// Absolute path to the included file.
    pub full_path: PathBuf,
    /// `true` when the file is in a system include directory.
    pub is_system: bool,
}

/// A named symbol extracted from the TU top-level declarations,
/// used to replace the docify-based `DocIndex` for C/C++ files.
pub struct TuSymbol {
    pub name: String,
    pub hover: HoverInfo,
    /// 0-based line of the declaration (reserved for document-symbol outline).
    #[allow(dead_code)]
    pub line: u32,
}

/// A single inlay hint produced from the AST (parameter name or deduced type).
pub struct AstInlayHint {
    /// 0-based line.
    pub line: u32,
    /// 0-based column (position *before* which the hint is inserted).
    pub col: u32,
    pub label: String,
    /// 1 = Type, 2 = Parameter (LSP InlayHintKind).
    pub kind: u32,
    pub padding_left: bool,
    pub padding_right: bool,
}

// ---------------------------------------------------------------------------
// TuCache
// ---------------------------------------------------------------------------

pub struct TuCache {
    index: CXIndex,
    tus: HashMap<PathBuf, CXTranslationUnit>,
    /// line (0-based) → inclusion, rebuilt on every open/reparse.
    inclusions: HashMap<PathBuf, HashMap<u32, InclusionInfo>>,
    /// top-level symbols, rebuilt on every open/reparse.
    symbols: HashMap<PathBuf, Vec<TuSymbol>>,
    cc_dir: Option<PathBuf>,
}

// Safety: CXIndex / CXTranslationUnit are opaque pointers. The Server lives
// entirely on the main thread; these are never aliased across threads.
unsafe impl Send for TuCache {}
unsafe impl Sync for TuCache {}

impl TuCache {
    /// Load libclang via dlopen and create a CXIndex.
    /// Returns `None` if libclang is not available on this system.
    pub fn try_new(cc_dir: Option<PathBuf>) -> Option<Self> {
        if !clang_sys::is_loaded() && clang_sys::load().is_err() {
            tracing::info!("libclang not found — AST-backed LSP features disabled");
            return None;
        }
        let index = unsafe { clang_createIndex(0, 0) };
        if index.is_null() {
            return None;
        }
        tracing::info!("libclang loaded — AST-backed hover/definition/hints active");
        Some(Self {
            index,
            tus: HashMap::new(),
            inclusions: HashMap::new(),
            symbols: HashMap::new(),
            cc_dir,
        })
    }

    pub fn set_cc_dir(&mut self, cc_dir: Option<PathBuf>) {
        if self.cc_dir == cc_dir {
            return;
        }
        self.cc_dir = cc_dir;
        // Reparse every open TU now that we have compile flags.
        let paths: Vec<PathBuf> = self.tus.keys().cloned().collect();
        for path in paths {
            self.open(&path);
        }
    }

    // -----------------------------------------------------------------------
    // TU lifecycle
    // -----------------------------------------------------------------------

    /// Parse or reparse the translation unit for `path`.
    pub fn open(&mut self, path: &Path) {
        let flags = self.compile_flags_for(path);
        let c_path = match CString::new(path.to_string_lossy().as_bytes()) {
            Ok(s) => s,
            Err(_) => return,
        };
        let c_flags: Vec<CString> = flags
            .iter()
            .filter_map(|f| CString::new(f.as_str()).ok())
            .collect();
        let c_flag_ptrs: Vec<*const i8> = c_flags.iter().map(|s| s.as_ptr()).collect();

        if let Some(&tu) = self.tus.get(path) {
            let result = unsafe {
                clang_reparseTranslationUnit(
                    tu,
                    0,
                    std::ptr::null_mut(),
                    clang_defaultReparseOptions(tu),
                )
            };
            if result != 0 {
                unsafe { clang_disposeTranslationUnit(tu) };
                self.tus.remove(path);
                self.inclusions.remove(path);
                self.symbols.remove(path);
                self.parse_fresh(&c_path, &c_flag_ptrs, path);
            } else {
                // Reparse succeeded — refresh derived caches.
                self.refresh_derived(path, tu);
            }
        } else {
            self.parse_fresh(&c_path, &c_flag_ptrs, path);
        }
    }

    pub fn close(&mut self, path: &Path) {
        if let Some(tu) = self.tus.remove(path) {
            unsafe { clang_disposeTranslationUnit(tu) };
        }
        self.inclusions.remove(path);
        self.symbols.remove(path);
    }

    // -----------------------------------------------------------------------
    // Hover
    // -----------------------------------------------------------------------

    /// Return hover information for the cursor at `(line, col)` (0-based).
    pub fn hover(&self, path: &Path, line: u32, col: u32) -> Option<HoverInfo> {
        let tu = *self.tus.get(path)?;
        let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
        unsafe {
            let file = clang_getFile(tu, c_path.as_ptr());
            if file.is_null() {
                return None;
            }
            // clang uses 1-based line/column.
            let loc = clang_getLocation(tu, file, line + 1, col + 1);
            let cursor = clang_getCursor(tu, loc);
            let ck = clang_getCursorKind(cursor);
            if clang_Cursor_isNull(cursor) != 0
                || ck == CXCursor_TranslationUnit
                || ck == CXCursor_InvalidFile
            {
                return None;
            }

            // Walk to referenced symbol for richer info.
            let referenced = clang_getCursorReferenced(cursor);
            let src = if clang_Cursor_isNull(referenced) == 0 {
                referenced
            } else {
                cursor
            };

            hover_info_for_cursor(src)
        }
    }

    // -----------------------------------------------------------------------
    // Go-to-definition
    // -----------------------------------------------------------------------

    /// Return the definition location for the cursor at `(line, col)` (0-based).
    pub fn definition(&self, path: &Path, line: u32, col: u32) -> Option<DefinitionLocation> {
        let tu = *self.tus.get(path)?;
        let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
        unsafe {
            let file = clang_getFile(tu, c_path.as_ptr());
            if file.is_null() {
                return None;
            }
            let loc = clang_getLocation(tu, file, line + 1, col + 1);
            let cursor = clang_getCursor(tu, loc);
            if clang_Cursor_isNull(cursor) != 0 {
                return None;
            }

            // Prefer definition; fall back to declaration.
            let def = clang_getCursorDefinition(cursor);
            let target = if clang_Cursor_isNull(def) == 0 {
                def
            } else {
                let referenced = clang_getCursorReferenced(cursor);
                if clang_Cursor_isNull(referenced) == 0 {
                    referenced
                } else {
                    return None;
                }
            };

            let target_loc = clang_getCursorLocation(target);
            let mut def_file: CXFile = std::ptr::null_mut();
            let mut def_line: u32 = 0;
            let mut def_col: u32 = 0;
            let mut def_offset: u32 = 0;
            clang_getSpellingLocation(
                target_loc,
                &mut def_file,
                &mut def_line,
                &mut def_col,
                &mut def_offset,
            );

            if def_file.is_null() {
                return None;
            }
            let def_path = cx_string(clang_getFileName(def_file));
            if def_path.is_empty() {
                return None;
            }

            Some(DefinitionLocation {
                path: PathBuf::from(def_path),
                line: def_line.saturating_sub(1),
                col: def_col.saturating_sub(1),
            })
        }
    }

    // -----------------------------------------------------------------------
    // Inclusions and symbols
    // -----------------------------------------------------------------------

    /// Return the inclusion at `line` (0-based) in `path`, if the TU is loaded.
    pub fn inclusion_at(&self, path: &Path, line: u32) -> Option<&InclusionInfo> {
        self.inclusions.get(path)?.get(&line)
    }

    /// Return all inclusions in `path` (line → info), if the TU is loaded.
    pub fn inclusions_for(&self, path: &Path) -> Option<&HashMap<u32, InclusionInfo>> {
        self.inclusions.get(path)
    }

    /// Return all top-level symbols extracted from the TU for `path`.
    pub fn symbols_for(&self, path: &Path) -> Option<&[TuSymbol]> {
        self.symbols.get(path).map(Vec::as_slice)
    }

    // -----------------------------------------------------------------------
    // AST inlay hints (Phase 4)
    // -----------------------------------------------------------------------

    /// Returns `true` if a parsed TU exists for `path`.
    pub fn has_tu(&self, path: &Path) -> bool {
        self.tus.contains_key(path)
    }

    /// Walk the AST in `[start_line, end_line]` (0-based) and collect:
    /// - parameter name hints at call-expression argument positions
    /// - deduced-type hints on `auto` variable declarations
    pub fn ast_inlay_hints(
        &self,
        path: &Path,
        start_line: u32,
        end_line: u32,
    ) -> Option<Vec<AstInlayHint>> {
        let tu = *self.tus.get(path)?;
        let root = unsafe { clang_getTranslationUnitCursor(tu) };
        let mut v = HintVisitor {
            hints: Vec::new(),
            start_line,
            end_line,
        };
        unsafe {
            clang_visitChildren(root, hint_visitor, &mut v as *mut _ as CXClientData);
        }
        Some(v.hints)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn parse_fresh(&mut self, c_path: &CString, c_flag_ptrs: &[*const i8], path: &Path) {
        let options = unsafe { clang_defaultEditingTranslationUnitOptions() };
        let tu = unsafe {
            clang_parseTranslationUnit(
                self.index,
                c_path.as_ptr(),
                c_flag_ptrs.as_ptr(),
                c_flag_ptrs.len() as i32,
                std::ptr::null_mut(),
                0,
                options,
            )
        };
        if !tu.is_null() {
            self.tus.insert(path.to_path_buf(), tu);
            self.refresh_derived(path, tu);
            tracing::debug!(path = %path.display(), "libclang: TU parsed");
        } else {
            tracing::warn!(path = %path.display(), "libclang: clang_parseTranslationUnit returned null");
        }
    }

    fn refresh_derived(&mut self, path: &Path, tu: CXTranslationUnit) {
        self.inclusions
            .insert(path.to_path_buf(), build_inclusions(path, tu));
        self.symbols.insert(path.to_path_buf(), build_symbols(tu));
    }

    fn compile_flags_for(&self, path: &Path) -> Vec<String> {
        let Some(cc_dir) = &self.cc_dir else {
            return vec![];
        };
        let cc_file = cc_dir.join("compile_commands.json");
        let Ok(content) = std::fs::read_to_string(&cc_file) else {
            return vec![];
        };
        let Ok(entries) = serde_json::from_str::<Vec<Value>>(&content) else {
            return vec![];
        };
        let path_abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        for entry in &entries {
            let file = entry.get("file").and_then(Value::as_str).unwrap_or("");
            let directory = entry
                .get("directory")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .or_else(|| cc_dir.parent().map(PathBuf::from))
                .unwrap_or_else(|| cc_dir.to_path_buf());
            let file_path = PathBuf::from(file);
            let file_abs = if file_path.is_absolute() {
                file_path
            } else {
                directory.join(file_path)
            };
            let file_abs = file_abs.canonicalize().unwrap_or(file_abs);
            if file_abs == path_abs {
                return extract_compile_flags(entry, &directory, &path_abs);
            }
        }
        vec![]
    }
}

impl Drop for TuCache {
    fn drop(&mut self) {
        for tu in self.tus.values().copied() {
            unsafe { clang_disposeTranslationUnit(tu) };
        }
        unsafe { clang_disposeIndex(self.index) };
    }
}

// ---------------------------------------------------------------------------
// Inclusions builder (clang_getInclusions)
// ---------------------------------------------------------------------------

struct InclusionCollector {
    map: HashMap<u32, InclusionInfo>,
    source_file: CXFile,
    tu: CXTranslationUnit,
}

extern "C" fn on_inclusion(
    included_file: CXFile,
    inclusion_stack: *mut CXSourceLocation,
    include_len: u32,
    data: CXClientData,
) {
    if include_len == 0 || included_file.is_null() {
        return;
    }
    let col = unsafe { &mut *(data as *mut InclusionCollector) };

    // Only care about direct includes from our source file.
    let directive_loc = unsafe { *inclusion_stack };
    let mut inc_file: CXFile = std::ptr::null_mut();
    let mut line: u32 = 0;
    let mut dummy_col: u32 = 0;
    let mut dummy_off: u32 = 0;
    unsafe {
        clang_getSpellingLocation(
            directive_loc,
            &mut inc_file,
            &mut line,
            &mut dummy_col,
            &mut dummy_off,
        )
    };
    if inc_file != col.source_file || line == 0 {
        return;
    }

    let full_path_str = unsafe { cx_string(clang_getFileName(included_file)) };
    if full_path_str.is_empty() {
        return;
    }

    // Determine system status by asking libclang about a location inside the
    // included file itself (line 1, col 1).
    let inner_loc = unsafe { clang_getLocation(col.tu, included_file, 1, 1) };
    let is_system = unsafe { clang_Location_isInSystemHeader(inner_loc) } != 0;

    col.map.insert(
        line - 1,
        InclusionInfo {
            full_path: PathBuf::from(full_path_str),
            is_system,
        },
    );
}

fn build_inclusions(path: &Path, tu: CXTranslationUnit) -> HashMap<u32, InclusionInfo> {
    let c_path = match CString::new(path.to_string_lossy().as_bytes()) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    let source_file = unsafe { clang_getFile(tu, c_path.as_ptr()) };
    if source_file.is_null() {
        return HashMap::new();
    }
    let mut collector = InclusionCollector {
        map: HashMap::new(),
        source_file,
        tu,
    };
    unsafe {
        clang_getInclusions(tu, on_inclusion, &mut collector as *mut _ as CXClientData);
    }
    collector.map
}

// ---------------------------------------------------------------------------
// Symbol builder (top-level declaration walk)
// ---------------------------------------------------------------------------

struct SymbolCollector {
    symbols: Vec<TuSymbol>,
    source_file: CXFile,
}

extern "C" fn on_symbol(
    cursor: CXCursor,
    _parent: CXCursor,
    data: CXClientData,
) -> CXChildVisitResult {
    let col = unsafe { &mut *(data as *mut SymbolCollector) };
    let kind = unsafe { clang_getCursorKind(cursor) };

    // Skip cursors from other files (headers etc.).
    let loc = unsafe { clang_getCursorLocation(cursor) };
    let mut file: CXFile = std::ptr::null_mut();
    let mut line: u32 = 0;
    let mut col_num: u32 = 0;
    let mut offset: u32 = 0;
    unsafe { clang_getSpellingLocation(loc, &mut file, &mut line, &mut col_num, &mut offset) };
    if file != col.source_file {
        // Not from our source file — skip but don't recurse.
        return CXChildVisit_Continue;
    }

    // Recurse into containers so we find symbols in namespaces and classes,
    // but record the container declaration itself first.
    #[allow(non_upper_case_globals)]
    let is_container = matches!(
        kind,
        CXCursor_Namespace
            | CXCursor_ClassDecl
            | CXCursor_StructDecl
            | CXCursor_ClassTemplate
            | CXCursor_ClassTemplatePartialSpecialization
            | CXCursor_UnionDecl
    );

    if unsafe { clang_isDeclaration(kind) } == 0 || line == 0 {
        return if is_container {
            CXChildVisit_Recurse
        } else {
            CXChildVisit_Continue
        };
    }

    let name = unsafe { cx_string(clang_getCursorSpelling(cursor)) };
    if name.is_empty() {
        return CXChildVisit_Continue;
    }

    col.symbols.push(TuSymbol {
        hover: match unsafe { hover_info_for_cursor(cursor) } {
            Some(hover) => hover,
            None => return CXChildVisit_Continue,
        },
        name,
        line: line - 1,
    });

    if is_container {
        CXChildVisit_Recurse
    } else {
        // Don't recurse into function bodies.
        CXChildVisit_Continue
    }
}

fn build_symbols(tu: CXTranslationUnit) -> Vec<TuSymbol> {
    let c_path_str = unsafe { cx_string(clang_getTranslationUnitSpelling(tu)) };
    if c_path_str.is_empty() {
        return vec![];
    }
    let c_path = match CString::new(c_path_str.as_bytes()) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let source_file = unsafe { clang_getFile(tu, c_path.as_ptr()) };
    if source_file.is_null() {
        return vec![];
    }

    let mut col = SymbolCollector {
        symbols: Vec::new(),
        source_file,
    };
    let root = unsafe { clang_getTranslationUnitCursor(tu) };
    unsafe {
        clang_visitChildren(root, on_symbol, &mut col as *mut _ as CXClientData);
    }
    col.symbols
}

// ---------------------------------------------------------------------------
// Hover extraction helpers
// ---------------------------------------------------------------------------

unsafe fn hover_info_for_cursor(cursor: CXCursor) -> Option<HoverInfo> {
    let display_name = cx_string(clang_getCursorDisplayName(cursor));
    if display_name.is_empty() {
        return None;
    }

    let kind = clang_getCursorKind(cursor);
    let type_cx = clang_getCursorType(cursor);
    let type_str = type_spelling(type_cx);
    let canonical_type = canonical_type_spelling(type_cx, type_str.as_deref());
    let result_type = cursor_result_type(cursor);
    let params = param_infos(cursor);
    let parsed_doc = doc_comment(cursor);
    let qualified_name = qualified_name(cursor, &display_name);
    let (source_file, source_line, source_col) = cursor_source_location(cursor);
    let pretty_decl = concise_decl(
        cursor,
        type_str.as_deref(),
        result_type.as_deref(),
        &display_name,
    )
    .filter(|decl| !is_recovery_decl(decl))
    .or_else(|| source_decl_line(source_file.as_deref(), source_line));

    Some(HoverInfo {
        display_name,
        qualified_name,
        pretty_decl,
        type_str,
        canonical_type,
        result_type,
        params,
        doc: parsed_doc.brief,
        body: parsed_doc.body,
        tags: parsed_doc.tags,
        source_file,
        source_line,
        source_col,
        cursor_kind: kind as u32,
    })
}

unsafe fn pretty_decl(cursor: CXCursor) -> Option<String> {
    let policy = clang_getCursorPrintingPolicy(cursor);
    let printed = if policy.is_null() {
        cx_string(clang_getCursorPrettyPrinted(cursor, std::ptr::null_mut()))
    } else {
        clang_PrintingPolicy_setProperty(policy, CXPrintingPolicy_PolishForDeclaration, 1);
        clang_PrintingPolicy_setProperty(policy, CXPrintingPolicy_FullyQualifiedName, 0);
        clang_PrintingPolicy_setProperty(policy, CXPrintingPolicy_IncludeTagDefinition, 0);
        clang_PrintingPolicy_setProperty(policy, CXPrintingPolicy_SuppressInitializers, 1);
        let s = cx_string(clang_getCursorPrettyPrinted(cursor, policy));
        clang_PrintingPolicy_dispose(policy);
        s
    };
    non_empty(printed)
}

unsafe fn concise_decl(
    cursor: CXCursor,
    type_str: Option<&str>,
    result_type: Option<&str>,
    display_name: &str,
) -> Option<String> {
    let kind = clang_getCursorKind(cursor);
    let pretty = pretty_decl(cursor).and_then(|decl| concise_pretty_decl(&decl));
    if let Some(pretty) = pretty {
        return Some(pretty);
    }

    #[allow(non_upper_case_globals)]
    match kind {
        CXCursor_ClassDecl | CXCursor_ClassTemplate => {
            Some(format!("class {}", type_str.unwrap_or(display_name)))
        }
        CXCursor_StructDecl => Some(format!("struct {}", type_str.unwrap_or(display_name))),
        CXCursor_UnionDecl => Some(format!("union {}", type_str.unwrap_or(display_name))),
        CXCursor_EnumDecl => Some(format!("enum {}", display_name)),
        CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_FunctionTemplate => {
            result_type.map(|ret| format!("{ret} {display_name}"))
        }
        CXCursor_Constructor | CXCursor_Destructor => Some(display_name.to_string()),
        CXCursor_VarDecl | CXCursor_FieldDecl | CXCursor_ParmDecl => {
            type_str.map(|ty| format!("{ty} {display_name}"))
        }
        CXCursor_TypedefDecl | CXCursor_TypeAliasDecl => {
            type_str.map(|ty| format!("using {display_name} = {ty}"))
        }
        _ => None,
    }
}

fn concise_pretty_decl(decl: &str) -> Option<String> {
    let mut line = decl.trim();
    if line.is_empty() || line.contains("<recovery-expr>") {
        return None;
    }
    if let Some(before_body) = line.split_once('{').map(|(before, _)| before.trim()) {
        line = before_body;
    }
    if let Some(before_init) = split_initializer(line) {
        line = before_init.trim_end();
    }
    let line = line.trim_end_matches(';').trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn split_initializer(line: &str) -> Option<&str> {
    let mut angle_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut chars = line.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '=' if angle_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                return Some(&line[..idx]);
            }
            _ => {}
        }
    }
    None
}

unsafe fn type_spelling(ty: CXType) -> Option<String> {
    if ty.kind == CXType_Invalid {
        return None;
    }
    non_empty(cx_string(clang_getTypeSpelling(ty)))
}

unsafe fn canonical_type_spelling(ty: CXType, declared: Option<&str>) -> Option<String> {
    if ty.kind == CXType_Invalid {
        return None;
    }
    let canonical = clang_getCanonicalType(ty);
    if canonical.kind == CXType_Invalid {
        return None;
    }
    let s = cx_string(clang_getTypeSpelling(canonical));
    match (non_empty(s), declared) {
        (Some(canonical), Some(declared)) if canonical == declared => None,
        (value, _) => value,
    }
}

unsafe fn cursor_result_type(cursor: CXCursor) -> Option<String> {
    let ret = clang_getCursorResultType(cursor);
    if ret.kind == CXType_Invalid {
        return None;
    }
    non_empty(cx_string(clang_getTypeSpelling(ret)))
}

unsafe fn param_infos(cursor: CXCursor) -> Vec<ParamInfo> {
    let count = clang_Cursor_getNumArguments(cursor);
    if count <= 0 {
        return Vec::new();
    }
    let mut params = Vec::with_capacity(count as usize);
    for i in 0..count as u32 {
        let param = clang_Cursor_getArgument(cursor, i);
        if clang_Cursor_isNull(param) != 0 {
            continue;
        }
        let name = non_empty(cx_string(clang_getCursorSpelling(param)));
        let type_str = type_spelling(clang_getCursorType(param));
        params.push(ParamInfo { name, type_str });
    }
    params
}

struct ParsedDoc {
    brief: Option<String>,
    body: Option<String>,
    tags: Vec<HoverTag>,
}

unsafe fn doc_comment(cursor: CXCursor) -> ParsedDoc {
    let direct = direct_doc_comment(cursor);
    if !direct.is_empty() {
        return direct;
    }

    let kind = clang_getCursorKind(cursor);
    #[allow(non_upper_case_globals)]
    let is_ctor_or_dtor = matches!(kind, CXCursor_Constructor | CXCursor_Destructor);

    if is_ctor_or_dtor {
        let parent = clang_getCursorSemanticParent(cursor);
        if clang_Cursor_isNull(parent) == 0
            && clang_getCursorKind(parent) != CXCursor_TranslationUnit
        {
            let parent_doc = doc_comment(parent);
            if !parent_doc.is_empty() {
                return parent_doc;
            }
        }
    }

    if direct.is_empty() {
        let canonical = clang_getCanonicalCursor(cursor);
        if clang_Cursor_isNull(canonical) == 0 && clang_equalCursors(canonical, cursor) == 0 {
            let canonical_doc = direct_doc_comment(canonical);
            if !canonical_doc.is_empty() {
                return canonical_doc;
            }
        }
    }

    direct
}

unsafe fn direct_doc_comment(cursor: CXCursor) -> ParsedDoc {
    let brief = cx_string(clang_Cursor_getBriefCommentText(cursor));
    let raw = cx_string(clang_Cursor_getRawCommentText(cursor));
    if raw.is_empty() {
        let (source_file, source_line, _) = cursor_source_location(cursor);
        if let Some(trailing) = trailing_doc_comment(source_file.as_deref(), source_line) {
            return trailing;
        }
        return ParsedDoc {
            brief: non_empty(brief),
            body: None,
            tags: Vec::new(),
        };
    }

    let stripped = strip_comment_markers(&raw);
    let (fallback_brief, body, tags) = parse_doc_sections(&stripped);
    ParsedDoc {
        brief: non_empty(brief).or(fallback_brief),
        body,
        tags,
    }
}

impl ParsedDoc {
    fn is_empty(&self) -> bool {
        self.brief.is_none() && self.body.is_none() && self.tags.is_empty()
    }
}

fn trailing_doc_comment(source_file: Option<&Path>, source_line: Option<u32>) -> Option<ParsedDoc> {
    let path = source_file?;
    let line = source_line?;
    let text = std::fs::read_to_string(path).ok()?;
    let line_text = text.lines().nth(line.saturating_sub(1) as usize)?;
    let comment = trailing_doc_fragment(line_text)?;
    let stripped = strip_comment_markers(comment);
    let (brief, body, tags) = parse_doc_sections(&stripped);
    let parsed = ParsedDoc { brief, body, tags };
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

fn trailing_doc_fragment(line: &str) -> Option<&str> {
    let markers = ["///<", "//!<", "///", "//!", "/**<", "/*!<", "/**", "/*!"];
    for marker in markers {
        if let Some(idx) = line.find(marker) {
            let before = line[..idx].trim();
            if !before.is_empty() && !before.starts_with("//") && !before.starts_with("/*") {
                return Some(&line[idx..]);
            }
        }
    }
    None
}

fn parse_doc_sections(text: &str) -> (Option<String>, Option<String>, Vec<HoverTag>) {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = Vec::new();
    let mut tags = Vec::new();
    let mut last_tag: Option<usize> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
            last_tag = None;
            continue;
        }

        if let Some(tag) = parse_doc_tag(line) {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
            tags.push(tag);
            last_tag = Some(tags.len() - 1);
        } else if let Some(idx) = last_tag {
            if !tags[idx].text.is_empty() {
                tags[idx].text.push(' ');
            }
            tags[idx].text.push_str(line);
        } else {
            current.push(line.to_string());
        }
    }

    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }

    let brief = paragraphs.first().cloned();
    let body = if paragraphs.len() > 1 {
        Some(paragraphs[1..].join("\n\n"))
    } else {
        None
    };
    (brief, body, tags)
}

fn parse_doc_tag(line: &str) -> Option<HoverTag> {
    let trimmed = line.trim_start();
    let tag_start = trimmed
        .strip_prefix('@')
        .or_else(|| trimmed.strip_prefix('\\'))?;
    let (raw_kind, rest) = split_first_word(tag_start);
    let kind = match raw_kind.to_ascii_lowercase().as_str() {
        "param" | "arg" | "argument" => HoverTagKind::Param,
        "tparam" | "typeparam" | "templateparam" => HoverTagKind::TParam,
        "return" | "returns" | "retval" => HoverTagKind::Return,
        "throws" | "throw" | "exception" => HoverTagKind::Throws,
        "note" => HoverTagKind::Note,
        "warning" | "warn" => HoverTagKind::Warning,
        "example" => HoverTagKind::Example,
        "see" | "sa" => HoverTagKind::See,
        "since" => HoverTagKind::Since,
        "deprecated" => HoverTagKind::Deprecated,
        _ => return None,
    };

    let mut rest = rest.trim_start();
    let mut name = None;
    if matches!(
        kind,
        HoverTagKind::Param | HoverTagKind::TParam | HoverTagKind::Throws
    ) {
        rest = rest
            .strip_prefix("[in]")
            .or_else(|| rest.strip_prefix("[out]"))
            .or_else(|| rest.strip_prefix("[in,out]"))
            .unwrap_or(rest)
            .trim_start();
        let (first, tail) = split_first_word(rest);
        if !first.is_empty() {
            name = Some(first.trim_matches(['<', '>', '`']).to_string());
            rest = tail.trim_start();
        }
    }

    Some(HoverTag {
        kind,
        name,
        text: rest.to_string(),
    })
}

fn split_first_word(s: &str) -> (&str, &str) {
    let trimmed = s.trim_start();
    if let Some(idx) = trimmed.find(char::is_whitespace) {
        (&trimmed[..idx], &trimmed[idx..])
    } else {
        (trimmed, "")
    }
}

unsafe fn qualified_name(cursor: CXCursor, display_name: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut parent = clang_getCursorSemanticParent(cursor);
    while clang_Cursor_isNull(parent) == 0
        && clang_getCursorKind(parent) != CXCursor_TranslationUnit
    {
        let name = cx_string(clang_getCursorSpelling(parent));
        if !name.is_empty() {
            parts.push(name);
        }
        parent = clang_getCursorSemanticParent(parent);
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    parts.push(display_name.to_string());
    Some(parts.join("::"))
}

unsafe fn cursor_source_location(cursor: CXCursor) -> (Option<PathBuf>, Option<u32>, Option<u32>) {
    let decl_loc = clang_getCursorLocation(cursor);
    let mut decl_file: CXFile = std::ptr::null_mut();
    let mut decl_line: u32 = 0;
    let mut decl_col: u32 = 0;
    let mut decl_off: u32 = 0;
    clang_getSpellingLocation(
        decl_loc,
        &mut decl_file,
        &mut decl_line,
        &mut decl_col,
        &mut decl_off,
    );
    let source_file = if decl_file.is_null() {
        None
    } else {
        non_empty(cx_string(clang_getFileName(decl_file))).map(PathBuf::from)
    };
    let source_line = if decl_line > 0 { Some(decl_line) } else { None };
    let source_col = if decl_col > 0 { Some(decl_col) } else { None };
    (source_file, source_line, source_col)
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// AST visitor for inlay hints
// ---------------------------------------------------------------------------

struct HintVisitor {
    hints: Vec<AstInlayHint>,
    start_line: u32,
    end_line: u32,
}

extern "C" fn hint_visitor(
    cursor: CXCursor,
    _parent: CXCursor,
    data: CXClientData,
) -> CXChildVisitResult {
    let v = unsafe { &mut *(data as *mut HintVisitor) };
    let kind = unsafe { clang_getCursorKind(cursor) };

    // Skip invalid / unexposed cursors entirely.
    if unsafe { clang_isInvalid(kind) } != 0 {
        return CXChildVisit_Continue;
    }

    let loc = unsafe { clang_getCursorLocation(cursor) };
    // Skip nodes that live inside system headers — no hints needed there.
    if unsafe { clang_Location_isInSystemHeader(loc) } != 0 {
        return CXChildVisit_Continue;
    }

    let mut file: CXFile = std::ptr::null_mut();
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    let mut offset: u32 = 0;
    unsafe { clang_getSpellingLocation(loc, &mut file, &mut line, &mut col, &mut offset) };
    if line == 0 {
        return CXChildVisit_Recurse;
    }
    let line0 = line - 1;

    if line0 > v.end_line {
        // Node starts after our range — skip its subtree too.
        return CXChildVisit_Continue;
    }

    if line0 >= v.start_line {
        #[allow(non_upper_case_globals)]
        match kind {
            CXCursor_CallExpr => collect_param_hints(cursor, v),
            CXCursor_VarDecl => collect_auto_type_hint(cursor, v),
            _ => {}
        }
    }

    CXChildVisit_Recurse
}

fn collect_param_hints(call_expr: CXCursor, v: &mut HintVisitor) {
    let num_args = unsafe { clang_Cursor_getNumArguments(call_expr) };
    if num_args <= 0 {
        return;
    }

    // Get the function/method declaration to read parameter names.
    let func_ref = unsafe { clang_getCursorReferenced(call_expr) };
    if unsafe { clang_Cursor_isNull(func_ref) } != 0 {
        return;
    }
    let func_num_params = unsafe { clang_Cursor_getNumArguments(func_ref) };
    if func_num_params <= 0 {
        return;
    }

    let n = (num_args as u32).min(func_num_params as u32);
    for i in 0..n {
        let arg_cursor = unsafe { clang_Cursor_getArgument(call_expr, i) };
        let param_cursor = unsafe { clang_Cursor_getArgument(func_ref, i) };
        if unsafe { clang_Cursor_isNull(param_cursor) } != 0 {
            continue;
        }

        let param_name = unsafe { cx_string(clang_getCursorSpelling(param_cursor)) };
        // Skip unnamed, underscore-prefixed, or single-char params.
        if param_name.is_empty() || param_name.starts_with('_') || param_name.len() == 1 {
            continue;
        }

        let arg_loc = unsafe { clang_getCursorLocation(arg_cursor) };
        let mut file: CXFile = std::ptr::null_mut();
        let mut line: u32 = 0;
        let mut col: u32 = 0;
        let mut offset: u32 = 0;
        unsafe { clang_getSpellingLocation(arg_loc, &mut file, &mut line, &mut col, &mut offset) };
        if line == 0 || file.is_null() {
            continue;
        }

        v.hints.push(AstInlayHint {
            line: line - 1,
            col: col - 1,
            label: format!("{param_name}:"),
            kind: 2,
            padding_left: false,
            padding_right: true,
        });
    }
}

fn collect_auto_type_hint(var_decl: CXCursor, v: &mut HintVisitor) {
    let ty = unsafe { clang_getCursorType(var_decl) };
    if ty.kind == CXType_Invalid {
        return;
    }
    // Only emit when the declared type contains "auto".
    let ty_spell = unsafe { cx_string(clang_getTypeSpelling(ty)) };
    if !ty_spell.contains("auto") {
        return;
    }
    // Get the canonical (deduced) type.
    let canonical = unsafe { clang_getCanonicalType(ty) };
    let deduced = unsafe { cx_string(clang_getTypeSpelling(canonical)) };
    if deduced.is_empty() || deduced.contains("auto") {
        return;
    }

    let name_loc = unsafe { clang_getCursorLocation(var_decl) };
    let spelling = unsafe { cx_string(clang_getCursorSpelling(var_decl)) };
    let mut file: CXFile = std::ptr::null_mut();
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    let mut offset: u32 = 0;
    unsafe { clang_getSpellingLocation(name_loc, &mut file, &mut line, &mut col, &mut offset) };
    if line == 0 || file.is_null() {
        return;
    }

    v.hints.push(AstInlayHint {
        line: line - 1,
        col: col - 1 + spelling.len() as u32,
        label: format!(": {deduced}"),
        kind: 1,
        padding_left: true,
        padding_right: false,
    });
}

// ---------------------------------------------------------------------------
// Hover markdown rendering
// ---------------------------------------------------------------------------

pub fn hover_info_to_markdown(info: &HoverInfo) -> String {
    let mut out = String::new();

    let decl = if let Some(pretty) = &info.pretty_decl {
        pretty.clone()
    } else if let Some(ret) = &info.result_type {
        format!("{ret} {}", info.display_name)
    } else if let Some(ty) = &info.type_str {
        format!("{ty} {}", info.display_name)
    } else {
        info.display_name.clone()
    };
    out.push_str(&format!("```cpp\n{decl}\n```"));

    if let Some(doc) = &info.doc {
        out.push_str("\n\n");
        out.push_str(doc);
    }

    if let Some(body) = &info.body {
        out.push_str("\n\n");
        out.push_str(body);
    }

    render_doc_tags(&mut out, &info.tags);

    if let Some(ty) = info
        .type_str
        .as_deref()
        .filter(|ty| should_show_type(ty, &decl))
    {
        out.push_str("\n\n");
        out.push_str("**Type:** `");
        out.push_str(ty);
        out.push('`');
    }
    if let Some(canonical) = info
        .canonical_type
        .as_deref()
        .filter(|canonical| should_show_type(canonical, &decl))
    {
        out.push_str("\n\n");
        out.push_str("**Canonical:** `");
        out.push_str(canonical);
        out.push('`');
    }

    if !info.params.is_empty() {
        out.push_str("\n\n");
        out.push_str("| parameter | type |\n| --- | --- |\n");
        for param in &info.params {
            let name = param.name.as_deref().unwrap_or("_");
            let ty = param.type_str.as_deref().unwrap_or("_");
            out.push_str("| `");
            out.push_str(name);
            out.push_str("` | `");
            out.push_str(ty);
            out.push_str("` |\n");
        }
    }

    if let Some(file) = info
        .source_file
        .as_ref()
        .filter(|_| should_show_source_footer(info.cursor_kind))
    {
        out.push_str("\n\n---\n*defined in `");
        out.push_str(&display_source_path(file));
        if let Some(line) = info.source_line {
            out.push(':');
            out.push_str(&line.to_string());
            if let Some(col) = info.source_col {
                out.push(':');
                out.push_str(&col.to_string());
            }
        }
        out.push_str("`*");
    }

    out
}

fn should_show_type(type_str: &str, declaration: &str) -> bool {
    let ty = type_str.trim();
    if ty.is_empty() {
        return false;
    }

    // The declaration pretty-printer already contains the useful type spelling
    // for ordinary functions, variables, fields, aliases, and enum constants.
    if declaration.contains(ty) {
        return false;
    }

    // Function prototype spellings like `int (int)` are usually less readable
    // than the pretty declaration and caused noisy hovers dominated by `int`.
    if ty.contains('(') && ty.contains(')') {
        return false;
    }

    true
}

fn is_recovery_decl(decl: &str) -> bool {
    decl.contains("<recovery-expr>")
}

fn source_decl_line(source_file: Option<&Path>, source_line: Option<u32>) -> Option<String> {
    let path = source_file?;
    let line = source_line?;
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .nth(line.saturating_sub(1) as usize)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

#[allow(non_upper_case_globals)]
fn should_show_source_footer(cursor_kind: u32) -> bool {
    cursor_kind != CXCursor_Namespace as u32
}

fn display_source_path(path: &Path) -> String {
    if let Some(rel) = relative_to_freight_package(path) {
        return rel;
    }
    if let Some(rel) = relative_to_pkgs_package(path) {
        return rel;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn display_include_path(path: &Path) -> String {
    if let Some(rel) = relative_to_freight_package(path) {
        return rel;
    }
    if let Some(rel) = relative_to_pkgs_package(path) {
        return rel;
    }
    if let Some(rel) = relative_to_include_root(path) {
        return rel;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn relative_to_freight_package(path: &Path) -> Option<String> {
    for ancestor in path.ancestors() {
        if ancestor.join("freight.toml").is_file() {
            return path
                .strip_prefix(ancestor)
                .ok()
                .map(|rel| rel.to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

fn relative_to_pkgs_package(path: &Path) -> Option<String> {
    let parts: Vec<_> = path.components().collect();
    let pkgs_idx = parts
        .iter()
        .position(|component| component.as_os_str() == ".pkgs")?;
    let pkg_idx = pkgs_idx + 1;
    if pkg_idx >= parts.len() {
        return None;
    }

    let pkg_root: PathBuf = parts[..=pkg_idx].iter().collect();
    let pkg_name = parts[pkg_idx].as_os_str().to_string_lossy();
    let rel = path.strip_prefix(&pkg_root).ok()?;
    Some(format!(
        "{}/{}",
        pkg_name,
        rel.to_string_lossy().replace('\\', "/")
    ))
}

fn relative_to_include_root(path: &Path) -> Option<String> {
    let parts: Vec<_> = path.components().collect();
    let include_idx = parts
        .iter()
        .rposition(|component| component.as_os_str() == "include")?;
    let rel_start = include_idx + 1;
    if rel_start >= parts.len() {
        return None;
    }
    let rel: PathBuf = parts[rel_start..].iter().collect();
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn render_doc_tags(out: &mut String, tags: &[HoverTag]) {
    let deprecated: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Deprecated)
        .collect();
    let params: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Param)
        .collect();
    let tparams: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::TParam)
        .collect();
    let returns: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Return)
        .collect();
    let throws: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Throws)
        .collect();
    let notes: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Note)
        .collect();
    let warnings: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Warning)
        .collect();
    let examples: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Example)
        .collect();
    let sees: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::See)
        .collect();
    let since: Vec<&HoverTag> = tags
        .iter()
        .filter(|t| t.kind == HoverTagKind::Since)
        .collect();

    if let Some(tag) = deprecated.first() {
        out.push_str("\n\n> **Deprecated**");
        let text = tag.text.trim();
        if !text.is_empty() {
            out.push_str(": ");
            out.push_str(text);
        }
    }

    if !params.is_empty() || !tparams.is_empty() {
        out.push_str("\n\n");
        for tag in &tparams {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str("- `<");
            out.push_str(name);
            out.push_str(">` - ");
            out.push_str(tag.text.trim());
            out.push('\n');
        }
        for tag in &params {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str("- `");
            out.push_str(name);
            out.push_str("` - ");
            out.push_str(tag.text.trim());
            out.push('\n');
        }
    }

    if !returns.is_empty() {
        out.push_str("\n");
        for tag in &returns {
            out.push_str("\n**Returns** ");
            out.push_str(tag.text.trim());
        }
    }

    if !throws.is_empty() {
        out.push_str("\n\n");
        for tag in &throws {
            let name = tag.name.as_deref().unwrap_or("_");
            out.push_str("- `throws ");
            out.push_str(name);
            out.push_str("` - ");
            out.push_str(tag.text.trim());
            out.push('\n');
        }
    }

    if !notes.is_empty() {
        out.push_str("\n\n**Note**\n\n");
        for tag in &notes {
            out.push_str("> ");
            out.push_str(tag.text.trim());
            out.push('\n');
        }
    }

    if !warnings.is_empty() {
        out.push_str("\n\n**Warning**\n\n");
        for tag in &warnings {
            out.push_str("> ");
            out.push_str(tag.text.trim());
            out.push('\n');
        }
    }

    if !examples.is_empty() {
        for tag in &examples {
            out.push_str("\n\n**Example**\n\n");
            let text = tag.text.trim();
            if text.starts_with("```") {
                out.push_str(text);
            } else {
                out.push_str("```\n");
                out.push_str(text);
                out.push_str("\n```");
            }
        }
    }

    if !sees.is_empty() {
        out.push_str("\n\n**See also**: ");
        let refs: Vec<String> = sees
            .iter()
            .map(|t| format!("`{}`", t.text.trim()))
            .collect();
        out.push_str(&refs.join(", "));
    }

    if !since.is_empty() {
        out.push_str("\n\n**Since**: ");
        let values: Vec<&str> = since.iter().map(|t| t.text.trim()).collect();
        out.push_str(&values.join(", "));
    }
}

// ---------------------------------------------------------------------------
// compile_commands.json flag extraction
// ---------------------------------------------------------------------------

/// Strip `///`, `//!`, `//`, `/**`, `*/`, and leading `*` from raw comment text.
fn strip_comment_markers(raw: &str) -> String {
    raw.lines()
        .map(|l| {
            let t = l.trim();
            // Block comment delimiters
            if t.starts_with("/**<") || t.starts_with("/*!<") {
                return t[4..].trim_end_matches("*/").trim().to_string();
            }
            if t.starts_with("/**") {
                return t[3..]
                    .trim_start_matches('*')
                    .trim_end_matches("*/")
                    .trim()
                    .to_string();
            }
            if t.starts_with("/*!") {
                return t[3..].trim_end_matches("*/").trim().to_string();
            }
            if t.starts_with("*/") || t == "*" {
                return String::new();
            }
            if let Some(rest) = t.strip_prefix("* ").or_else(|| t.strip_prefix('*')) {
                return rest.to_string();
            }
            // Line comment markers
            for marker in &["///< ", "///<", "/// ", "///", "//! ", "//!", "// ", "//"] {
                if let Some(rest) = t.strip_prefix(marker) {
                    return rest.to_string();
                }
            }
            t.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn extract_compile_flags(entry: &Value, directory: &Path, source_path: &Path) -> Vec<String> {
    // Prefer "arguments" array; fall back to shell-split "command" string.
    if let Some(args) = entry.get("arguments").and_then(Value::as_array) {
        let argv: Vec<String> = args
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let compiler = argv.first().cloned();
        let mut flags: Vec<String> = argv.into_iter().skip(1).collect();
        strip_output_and_source(&mut flags, directory, source_path);
        absolutize_compile_paths(&mut flags, directory);
        append_compiler_system_includes(&mut flags, compiler.as_deref());
        return flags;
    }
    if let Some(cmd) = entry.get("command").and_then(Value::as_str) {
        let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
        let compiler = argv.first().cloned();
        let mut flags: Vec<String> = argv.into_iter().skip(1).collect();
        strip_output_and_source(&mut flags, directory, source_path);
        absolutize_compile_paths(&mut flags, directory);
        append_compiler_system_includes(&mut flags, compiler.as_deref());
        return flags;
    }
    vec![]
}

/// Remove `-o <path>` pairs and the source file argument from a flags list.
fn strip_output_and_source(flags: &mut Vec<String>, directory: &Path, source_path: &Path) {
    let source_exts = [
        ".c", ".cc", ".cpp", ".cxx", ".c++", ".cu", ".hip", ".m", ".mm", ".cl", ".ispc",
    ];
    let mut i = 0;
    while i < flags.len() {
        let f = &flags[i];
        if f == "-o" {
            flags.remove(i);
            if i < flags.len() {
                flags.remove(i);
            }
        } else if f.starts_with("-o") && f.len() > 2 {
            flags.remove(i);
        } else if !f.starts_with('-')
            && source_exts.iter().any(|ext| f.ends_with(ext))
            && same_compile_path(f, directory, source_path)
        {
            flags.remove(i);
        } else {
            i += 1;
        }
    }
}

fn same_compile_path(flag: &str, directory: &Path, source_path: &Path) -> bool {
    let path = PathBuf::from(flag);
    let absolute = if path.is_absolute() {
        path
    } else {
        directory.join(path)
    };
    absolute.canonicalize().unwrap_or(absolute) == source_path
}

fn absolutize_compile_paths(flags: &mut [String], directory: &Path) {
    let mut i = 0;
    while i < flags.len() {
        let flag = flags[i].clone();
        if matches!(
            flag.as_str(),
            "-I" | "-isystem" | "-iquote" | "-idirafter" | "-iframework" | "-include"
        ) {
            if let Some(next) = flags.get_mut(i + 1) {
                absolutize_path_arg(next, directory);
            }
            i += 2;
            continue;
        }

        for prefix in ["-I", "-isystem", "-iquote", "-idirafter", "-iframework"] {
            if flag.starts_with(prefix) && flag.len() > prefix.len() {
                let value = &flag[prefix.len()..];
                flags[i] = format!("{prefix}{}", absolute_path_arg(value, directory));
                break;
            }
        }

        i += 1;
    }
}

fn absolutize_path_arg(value: &mut String, directory: &Path) {
    *value = absolute_path_arg(value, directory);
}

fn absolute_path_arg(value: &str, directory: &Path) -> String {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        value.to_string()
    } else {
        directory.join(path).to_string_lossy().into_owned()
    }
}

fn append_compiler_system_includes(flags: &mut Vec<String>, compiler: Option<&str>) {
    let Some(compiler) = compiler.filter(|c| !c.is_empty()) else {
        return;
    };
    flags.extend(system_include_flags_for_compiler(compiler));
}

fn system_include_flags_for_compiler(compiler: &str) -> Vec<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(flags) = cache.lock().unwrap().get(compiler).cloned() {
        return flags;
    }

    let flags = probe_compiler_system_include_flags(compiler);
    cache
        .lock()
        .unwrap()
        .insert(compiler.to_string(), flags.clone());
    flags
}

fn probe_compiler_system_include_flags(compiler: &str) -> Vec<String> {
    let Ok(output) = std::process::Command::new(compiler)
        .args(["-x", "c++", "-E", "-v", "/dev/null"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
    else {
        return Vec::new();
    };

    parse_system_include_flags(&String::from_utf8_lossy(&output.stderr))
}

fn parse_system_include_flags(stderr: &str) -> Vec<String> {
    let mut flags = Vec::new();
    let mut in_block = false;
    for line in stderr.lines() {
        if line.contains("#include <...> search starts here") {
            in_block = true;
            continue;
        }
        if in_block {
            if line.contains("End of search list") {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let path = trimmed
                .split_once(" (")
                .map(|(path, _)| path)
                .unwrap_or(trimmed);
            flags.push("-isystem".to_string());
            flags.push(path.to_string());
        }
    }
    flags
}

// ---------------------------------------------------------------------------
// CXString helper
// ---------------------------------------------------------------------------

/// Convert a `CXString` to a Rust `String` and dispose it.
unsafe fn cx_string(s: CXString) -> String {
    let ptr = clang_getCString(s);
    let result = if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    clang_disposeString(s);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_doxygen_tags_for_hover_markdown() {
        let raw = r#"
Brief summary.

Longer body text.

@tparam T value type
@param input input value
@return computed value
@throws Error when invalid
@note stable API
@warning allocates memory
@example
auto x = make_value(1);
@see make_other
@since 1.2.0
@deprecated use make_value2
"#;

        let (brief, body, tags) = parse_doc_sections(raw);
        assert_eq!(brief.as_deref(), Some("Brief summary."));
        assert_eq!(body.as_deref(), Some("Longer body text."));
        assert!(tags.iter().any(|t| t.kind == HoverTagKind::Param
            && t.name.as_deref() == Some("input")
            && t.text == "input value"));
        assert!(tags.iter().any(|t| t.kind == HoverTagKind::TParam
            && t.name.as_deref() == Some("T")
            && t.text == "value type"));

        let info = HoverInfo {
            display_name: "make_value(int)".to_string(),
            qualified_name: Some("freight::make_value(int)".to_string()),
            pretty_decl: Some("int make_value(int input)".to_string()),
            type_str: Some("int (int)".to_string()),
            canonical_type: None,
            result_type: Some("int".to_string()),
            params: vec![ParamInfo {
                name: Some("input".to_string()),
                type_str: Some("int".to_string()),
            }],
            doc: brief,
            body,
            tags,
            source_file: Some(PathBuf::from("include/freight/value.hpp")),
            source_line: Some(12),
            source_col: Some(5),
            cursor_kind: 0,
        };

        let md = hover_info_to_markdown(&info);
        assert!(md.contains("```cpp\nint make_value(int input)\n```"));
        assert!(md.contains("Brief summary."));
        assert!(md.contains("Longer body text."));
        assert!(md.contains("- `<T>` - value type"));
        assert!(md.contains("- `input` - input value"));
        assert!(md.contains("**Returns** computed value"));
        assert!(!md.contains("**Type:** `int (int)`"));
        assert!(!md.contains("**Returns:** `int`"));
        assert!(!md.contains("**Symbol:**"));
        assert!(!md.contains("USR:"));
        assert!(!md.contains("kind:"));
        assert!(md.contains("**See also**: `make_other`"));
        assert!(md.contains("*defined in `value.hpp:12:5`*"));
    }

    #[test]
    fn compile_flags_match_relative_compile_command_files() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(project.join("inc")).unwrap();
        let source = project.join("src/main.cpp");
        std::fs::write(&source, "int main() { return 0; }\n").unwrap();

        let entry = serde_json::json!({
            "directory": project,
            "file": "src/main.cpp",
            "arguments": [
                "/usr/bin/clang++",
                "-std=c++20",
                "-Iinc",
                "-c",
                "src/main.cpp",
                "-o",
                "target/dev/objs/src/main.o"
            ]
        });

        let flags = extract_compile_flags(&entry, project, &source.canonicalize().unwrap());
        assert!(flags.contains(&"-std=c++20".to_string()));
        assert!(flags.contains(&format!("-I{}", project.join("inc").display())));
        assert!(!flags.contains(&"src/main.cpp".to_string()));
        assert!(!flags.contains(&"-o".to_string()));
    }

    #[test]
    fn recovery_decl_falls_back_to_source_line() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("main.cpp");
        std::fs::write(
            &source,
            "int main() {\n    std::vector<double> data = {2.0, 4.0};\n}\n",
        )
        .unwrap();

        assert!(is_recovery_decl("int data = <recovery-expr>({2., 4.})"));
        assert_eq!(
            source_decl_line(Some(&source), Some(2)).as_deref(),
            Some("std::vector<double> data = {2.0, 4.0};")
        );
    }

    #[test]
    fn trailing_right_side_doc_comment_is_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("iostream");
        std::fs::write(
            &source,
            "namespace std {\nextern ostream cout; /// Linked to standard output\n}\n",
        )
        .unwrap();

        let parsed = trailing_doc_comment(Some(&source), Some(2)).unwrap();
        assert_eq!(parsed.brief.as_deref(), Some("Linked to standard output"));
    }

    #[test]
    fn parses_compiler_system_include_search_list() {
        let stderr = r#"
#include <...> search starts here:
 /usr/lib/gcc/x86_64-linux-gnu/13/../../../../include/c++/13
 /usr/lib/gcc/x86_64-linux-gnu/13/../../../../include/x86_64-linux-gnu/c++/13
 /usr/lib/llvm-18/lib/clang/18/include
 /System/Library/Frameworks (framework directory)
End of search list.
"#;

        let flags = parse_system_include_flags(stderr);
        assert_eq!(
            flags,
            vec![
                "-isystem",
                "/usr/lib/gcc/x86_64-linux-gnu/13/../../../../include/c++/13",
                "-isystem",
                "/usr/lib/gcc/x86_64-linux-gnu/13/../../../../include/x86_64-linux-gnu/c++/13",
                "-isystem",
                "/usr/lib/llvm-18/lib/clang/18/include",
                "-isystem",
                "/System/Library/Frameworks",
            ]
        );
    }

    #[test]
    fn concise_decl_strips_type_bodies_and_initializers() {
        assert_eq!(
            concise_pretty_decl("class vector<double> {\npublic:\n  void push_back(double);\n};")
                .as_deref(),
            Some("class vector<double>")
        );
        assert_eq!(
            concise_pretty_decl("std::vector<double> data = {2.0, 4.0};").as_deref(),
            Some("std::vector<double> data")
        );
        assert_eq!(
            concise_pretty_decl("std::pair<double, double> tada(mean(data), variance(data));")
                .as_deref(),
            Some("std::pair<double, double> tada(mean(data), variance(data))")
        );
        assert_eq!(
            concise_pretty_decl("int data = <recovery-expr>({2., 4.})"),
            None
        );
    }

    #[test]
    fn source_footer_is_project_relative_and_skips_namespaces() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("hello");
        let source = project.join("src/main.cpp");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            project.join("freight.toml"),
            "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(&source, "namespace api {}\n").unwrap();

        assert_eq!(display_source_path(&source), "src/main.cpp");

        let namespace = HoverInfo {
            display_name: "api".to_string(),
            qualified_name: None,
            pretty_decl: Some("namespace api".to_string()),
            type_str: None,
            canonical_type: None,
            result_type: None,
            params: Vec::new(),
            doc: None,
            body: None,
            tags: Vec::new(),
            source_file: Some(source.clone()),
            source_line: Some(1),
            source_col: Some(1),
            cursor_kind: CXCursor_Namespace as u32,
        };
        assert!(!hover_info_to_markdown(&namespace).contains("defined in"));

        let variable = HoverInfo {
            display_name: "data".to_string(),
            qualified_name: None,
            pretty_decl: Some("std::vector<double> data".to_string()),
            type_str: None,
            canonical_type: None,
            result_type: None,
            params: Vec::new(),
            doc: None,
            body: None,
            tags: Vec::new(),
            source_file: Some(source),
            source_line: Some(7),
            source_col: Some(25),
            cursor_kind: CXCursor_VarDecl as u32,
        };
        let md = hover_info_to_markdown(&variable);
        assert!(md.contains("*defined in `src/main.cpp:7:25`*"));
        assert!(!md.contains(tmp.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn include_paths_can_be_rendered_relative_to_include_root() {
        assert_eq!(
            display_include_path(Path::new("/usr/include/c++/13/vector")),
            "c++/13/vector"
        );
        assert_eq!(
            display_include_path(Path::new("/usr/lib/llvm-18/lib/clang/18/include/stddef.h")),
            "stddef.h"
        );
    }
}
