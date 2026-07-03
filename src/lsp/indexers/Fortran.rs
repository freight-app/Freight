use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fortran_lsp::{
    CodeAction, CompletionItem, DiagnosticSeverity, DocumentSymbol, InlayHint, Location, Position,
    Range, RenameError, SelectionRange, SignatureHelp, Symbol, SymbolKind, TextEdit, Workspace,
};
use serde_json::{json, Value};

use crate::build::deps;
use crate::lsp::index::LanguageIndexer;
use crate::lsp::protocol::{path_from_uri, position, uri_from_path};
use crate::manifest::load_manifest;
use crate::manifest::types::{Dependency, Manifest};

pub struct FortranIndexer {
    workspace: Workspace,
    sources: HashMap<PathBuf, String>,
}

impl FortranIndexer {
    pub fn new() -> Self {
        Self {
            workspace: Workspace::new(),
            sources: HashMap::new(),
        }
    }

    fn is_fortran(path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
                .unwrap_or(""),
            "f" | "for" | "ftn" | "f90" | "f95" | "f03" | "f08" | "f18" | "f77" | "f66"
        )
    }

    fn ensure_file(&mut self, path: &Path) -> Option<&str> {
        if !self.sources.contains_key(path) {
            let source = std::fs::read_to_string(path).ok()?;
            self.workspace.upsert_file(path.to_path_buf(), &source);
            self.sources.insert(path.to_path_buf(), source);
        }
        self.sources.get(path).map(String::as_str)
    }

    /// Index every Fortran file under the include roots so cross-file
    /// hover/definition/diagnostics work with a single file open (fortls scans
    /// the workspace root the same way at initialize). Files with a live editor
    /// buffer (`sources`) are skipped — the buffer is authoritative. Unchanged
    /// files are skipped, so a manifest refresh only reparses what changed on
    /// disk. Parsing is pure, so changed files are parsed in parallel.
    fn index_workspace_sources(&mut self, roots: &[PathBuf]) {
        use rayon::prelude::*;

        let mut files = Vec::new();
        let mut visited_dirs = std::collections::HashSet::new();
        for root in roots {
            collect_fortran_files(root, &mut visited_dirs, &mut files);
        }
        let changed: Vec<(PathBuf, String)> = files
            .into_iter()
            .filter(|path| !self.sources.contains_key(path))
            .filter_map(|path| {
                let source = std::fs::read_to_string(&path).ok()?;
                let unchanged = self
                    .workspace
                    .file(&path)
                    .is_some_and(|f| f.source == source);
                (!unchanged).then_some((path, source))
            })
            .collect();
        let defines = self.workspace.predefined_macros().to_vec();
        let parsed: Vec<fortran_lsp::ParsedFile> = changed
            .into_par_iter()
            .map(|(path, source)| {
                fortran_lsp::ParsedFile::parse_with_defines(path, &source, &defines)
            })
            .collect();
        for file in parsed {
            self.workspace.upsert_parsed(file);
        }
    }
}

/// Recursively collect Fortran source files, skipping build output, VCS
/// internals, and symlink cycles. Dep roots under `.pkgs` are still walked
/// because they are passed in as roots directly.
fn collect_fortran_files(
    dir: &Path,
    visited: &mut std::collections::HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "build" {
                continue;
            }
            collect_fortran_files(&path, visited, out);
        } else if file_type.is_file() && FortranIndexer::is_fortran(&path) {
            out.push(path);
        }
    }
}

impl Default for FortranIndexer {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_fortran_include_roots(manifest_dir: &Path, profile: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    push_existing_dir(&mut roots, &mut seen, manifest_dir.to_path_buf());
    push_existing_dir(&mut roots, &mut seen, manifest_dir.join("include"));
    push_existing_dir(&mut roots, &mut seen, manifest_dir.join("inc"));
    push_existing_dir(&mut roots, &mut seen, manifest_dir.join("src"));

    let Ok(manifest) = load_manifest(manifest_dir) else {
        return roots;
    };

    for include in manifest.build_settings_for(profile).include_paths {
        push_existing_dir(&mut roots, &mut seen, manifest_dir.join(include));
    }

    collect_dep_include_roots(manifest_dir, manifest_dir, &manifest, &mut roots, &mut seen);
    roots
}

/// The build's `-D` set for this project — `[compiler]`/`[os.*]` defines plus
/// default-feature defines — as `(NAME, VALUE)` pairs for the fortran-lsp
/// preprocessor, so `#ifdef` regions are evaluated as the compiler would.
fn fortran_predefined_macros(manifest_dir: &Path, profile: &str) -> Vec<(String, String)> {
    let Ok(manifest) = load_manifest(manifest_dir) else {
        return Vec::new();
    };
    let mut raw = manifest.build_settings_for(profile).defines;
    if let Ok(resolution) = crate::build::features::resolve_features(&manifest.features, &[], true)
    {
        raw.extend(crate::build::features::to_defines(&resolution.active));
        raw.extend(resolution.defines.iter().cloned());
    }
    let mut macros: Vec<(String, String)> = raw
        .iter()
        .map(|d| {
            let d = d.trim().trim_start_matches("-D");
            match d.split_once('=') {
                Some((name, value)) => (name.to_string(), value.to_string()),
                None => (d.to_string(), String::new()),
            }
        })
        .filter(|(name, _)| !name.is_empty())
        .collect();
    macros.sort();
    macros.dedup();
    macros
}

fn fortran_line_length_limits(manifest_dir: &Path) -> (Option<usize>, Option<usize>) {
    let Ok(manifest) = load_manifest(manifest_dir) else {
        return (None, None);
    };
    let Some(settings) = manifest.language.get("fortran") else {
        return (None, None);
    };
    (
        parse_positive_usize_option(settings.extra_options().get("max_line_length")),
        parse_positive_usize_option(settings.extra_options().get("max_comment_line_length")),
    )
}

fn parse_positive_usize_option(value: Option<&String>) -> Option<usize> {
    let parsed = value?.trim().parse::<usize>().ok()?;
    (parsed > 0).then_some(parsed)
}

fn collect_dep_include_roots(
    root_dir: &Path,
    declaring_dir: &Path,
    manifest: &Manifest,
    roots: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    for (name, dep) in manifest.effective_dependencies() {
        if crate::manifest::types::is_platform_dep(&name) {
            continue;
        }
        if matches!(&dep, Dependency::Detailed(detail) if detail.optional) {
            continue;
        }
        let Some(dep_dir) = dependency_source_dir(root_dir, declaring_dir, &name, &dep) else {
            continue;
        };
        if !dep_dir.is_dir() {
            continue;
        }

        push_existing_dir(roots, seen, dep_dir.clone());

        let dep_manifest = load_manifest(&dep_dir).ok();
        if let Some(ref manifest) = dep_manifest {
            for include in deps::dep_include_dirs(&dep_dir, manifest) {
                push_existing_dir(roots, seen, include);
            }
        } else {
            push_existing_dir(roots, seen, dep_dir.join("include"));
            push_existing_dir(roots, seen, dep_dir.join("inc"));
        }

        if let Dependency::Detailed(detail) = &dep {
            for include in &detail.include {
                push_existing_dir(roots, seen, dep_dir.join(include));
            }
        }

        if let Some(dep_manifest) = dep_manifest {
            collect_dep_include_roots(root_dir, &dep_dir, &dep_manifest, roots, seen);
        }
    }
}

fn dependency_source_dir(
    root_dir: &Path,
    declaring_dir: &Path,
    name: &str,
    dep: &Dependency,
) -> Option<PathBuf> {
    match dep {
        Dependency::Simple(_) => Some(root_dir.join(".pkgs").join(name)),
        Dependency::Detailed(detail) => {
            if let Some(path) = &detail.path {
                Some(declaring_dir.join(path))
            } else if detail.registry.as_deref() == Some("system") {
                None
            } else if detail.version.is_some() || detail.url.is_some() || detail.is_git() {
                Some(root_dir.join(".pkgs").join(name))
            } else {
                None
            }
        }
    }
}

fn push_existing_dir(
    roots: &mut Vec<PathBuf>,
    seen: &mut std::collections::HashSet<PathBuf>,
    path: PathBuf,
) -> bool {
    if !path.is_dir() {
        return false;
    }
    let key = path.canonicalize().unwrap_or_else(|_| path.clone());
    if seen.insert(key) {
        roots.push(path);
        true
    } else {
        false
    }
}

impl LanguageIndexer for FortranIndexer {
    fn handles(&self, path: &Path) -> bool {
        Self::is_fortran(path)
    }

    fn refresh_flags(&mut self, manifest_dir: &Path, _profile: &str) {
        let roots = collect_fortran_include_roots(manifest_dir, _profile);
        self.workspace.set_include_roots(roots.clone());
        let (max_line_length, max_comment_line_length) = fortran_line_length_limits(manifest_dir);
        self.workspace
            .set_line_length_limits(max_line_length, max_comment_line_length);
        self.workspace
            .set_predefined_macros(fortran_predefined_macros(manifest_dir, _profile));
        self.index_workspace_sources(&roots);
    }

    fn evict(&mut self, path: &Path) {
        // Closing a buffer must not un-index the file: other files' modules
        // still resolve through it. Drop the live buffer and restore the
        // on-disk content; only a file gone from disk leaves the index.
        self.sources.remove(path);
        match std::fs::read_to_string(path) {
            Ok(source) => {
                self.workspace.upsert_file(path.to_path_buf(), &source);
            }
            Err(_) => self.workspace.remove_file(path),
        }
    }

    fn reparse(&mut self, uri: &str, content: &str) {
        let Some(path) = path_from_uri(uri) else {
            return;
        };
        if !Self::is_fortran(&path) {
            return;
        }
        self.workspace.upsert_file(path.clone(), content);
        self.sources.insert(path, content.to_string());
    }

    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        let Some(path) = path_from_uri(uri) else {
            return Vec::new();
        };
        if !Self::is_fortran(&path) {
            return Vec::new();
        }
        self.ensure_file(&path);
        self.workspace
            .diagnostics(&path)
            .into_iter()
            .map(diagnostic_to_lsp)
            .collect()
    }

    fn hover(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        let md = self
            .workspace
            .hover(&path, Position::new(line, character), &source)?;
        Some(json!({ "contents": { "kind": "markdown", "value": md } }))
    }

    fn signature_help(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        self.workspace
            .signature_help(&path, Position::new(line, character), &source)
            .map(signature_help_to_lsp)
    }

    fn goto_definition(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        let loc =
            self.workspace
                .definition_location(&path, Position::new(line, character), &source)?;
        Some(location_to_lsp(&loc))
    }

    fn goto_implementation(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        let loc = self.workspace.implementation_location(
            &path,
            Position::new(line, character),
            &source,
        )?;
        Some(location_to_lsp(&loc))
    }

    fn completion(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?;
        let prefix = identifier_prefix(source.lines().nth(line).unwrap_or(""), character);
        let items: Vec<Value> = self
            .workspace
            .completions_at(&path, Position::new(line, character), &prefix)
            .into_iter()
            .map(completion_to_lsp)
            .collect();
        Some(json!({ "isIncomplete": false, "items": items }))
    }

    fn document_symbols(&mut self, uri: &str) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        self.ensure_file(&path);
        Some(
            self.workspace
                .document_symbols(&path)
                .iter()
                .map(document_symbol_to_lsp)
                .collect(),
        )
    }

    fn workspace_symbols(&mut self, query: &str) -> Option<Vec<Value>> {
        Some(
            self.workspace
                .workspace_symbols(query)
                .iter()
                .map(workspace_symbol_to_lsp)
                .collect(),
        )
    }

    fn folding_ranges(&mut self, uri: &str) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        self.ensure_file(&path);
        let mut ranges = Vec::new();
        for sym in self.workspace.document_symbols(&path) {
            collect_symbol_folds(&sym, &mut ranges);
        }
        Some(ranges)
    }

    fn code_actions(&mut self, uri: &str, _msg: &Value) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        self.ensure_file(&path);
        Some(
            self.workspace
                .code_actions(&path)
                .into_iter()
                .map(code_action_to_lsp)
                .collect(),
        )
    }

    fn references(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        Some(
            self.workspace
                .references(&path, Position::new(line, character), &source)
                .into_iter()
                .map(|loc| location_to_lsp(&loc))
                .collect(),
        )
    }

    fn document_highlight(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let (line, character) = position(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        Some(
            self.workspace
                .references(&path, Position::new(line, character), &source)
                .into_iter()
                .filter(|loc| loc.file == path)
                .map(|loc| json!({ "range": range_to_lsp(&loc.range), "kind": 1 }))
                .collect(),
        )
    }

    fn selection_ranges(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let positions = selection_range_positions(msg)?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        Some(
            positions
                .into_iter()
                .filter_map(|pos| self.workspace.selection_range(&path, pos, &source))
                .map(selection_range_to_lsp)
                .collect(),
        )
    }

    fn inlay_hints(&mut self, uri: &str, msg: &Value) -> Option<Vec<Value>> {
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        self.ensure_file(&path);
        let range = msg.get("params")?.get("range")?;
        let start_line = range.get("start")?.get("line")?.as_u64()? as usize;
        let end_line = range.get("end")?.get("line")?.as_u64()? as usize;
        Some(
            self.workspace
                .inlay_hints(&path, start_line, end_line)
                .into_iter()
                .map(inlay_hint_to_lsp)
                .collect(),
        )
    }

    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<u32>> {
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        self.ensure_file(&path);
        Some(self.workspace.semantic_token_data(&path))
    }

    fn rename(&mut self, uri: &str, msg: &Value) -> Option<Value> {
        let (line, character) = position(msg)?;
        let new_name = msg.get("params")?.get("newName")?.as_str()?;
        let path = path_from_uri(uri)?;
        if !Self::is_fortran(&path) {
            return None;
        }
        let source = self.ensure_file(&path)?.to_string();
        match self
            .workspace
            .rename(&path, Position::new(line, character), &source, new_name)
        {
            Ok(edits) => Some(workspace_edit_to_lsp(edits)),
            Err(err) => Some(rename_error_to_lsp(err)),
        }
    }
}

fn diagnostic_to_lsp(diagnostic: fortran_lsp::Diagnostic) -> Value {
    let severity = match diagnostic.severity {
        DiagnosticSeverity::Error => 1,
        DiagnosticSeverity::Warning => 2,
        DiagnosticSeverity::Information => 3,
    };
    json!({
        "range": range_to_lsp(&diagnostic.range),
        "severity": severity,
        "source": "fortran-lsp",
        "message": diagnostic.message
    })
}

fn signature_help_to_lsp(help: SignatureHelp) -> Value {
    let parameters: Vec<Value> = help
        .parameters
        .iter()
        .map(|param| json!({ "label": param }))
        .collect();
    let mut signature = json!({
        "label": help.label,
        "parameters": parameters
    });
    if let Some(docs) = help.documentation {
        signature["documentation"] = json!({ "kind": "markdown", "value": docs });
    }
    json!({
        "signatures": [signature],
        "activeSignature": 0,
        "activeParameter": help.active_parameter
    })
}

fn completion_to_lsp(item: CompletionItem) -> Value {
    let mut out = json!({
        "label": item.label,
        "kind": completion_kind(item.kind),
        "detail": item.detail
    });
    if let Some(docs) = item.documentation {
        out["documentation"] = json!({ "kind": "markdown", "value": docs });
    }
    out
}

fn inlay_hint_to_lsp(hint: InlayHint) -> Value {
    json!({
        "position": position_to_lsp(&hint.position),
        "label": hint.label,
        "kind": 2,
        "paddingRight": true
    })
}

fn document_symbol_to_lsp(sym: &DocumentSymbol) -> Value {
    json!({
        "name": sym.name,
        "detail": sym.detail,
        "kind": symbol_kind(sym.kind),
        "range": range_to_lsp(&sym.range),
        "selectionRange": range_to_lsp(&sym.selection_range),
        "children": sym.children.iter().map(document_symbol_to_lsp).collect::<Vec<_>>()
    })
}

fn workspace_symbol_to_lsp(sym: &Symbol) -> Value {
    let mut item = json!({
        "name": sym.qualified_name(),
        "kind": symbol_kind(sym.kind),
        "location": location_to_lsp(&Location {
            file: sym.file.clone(),
            range: sym.selection_range.clone(),
        })
    });
    if !sym.scope.is_empty() {
        item.as_object_mut().unwrap().insert(
            "containerName".to_string(),
            Value::String(sym.scope.join("::")),
        );
    }
    item
}

fn selection_range_to_lsp(selection: SelectionRange) -> Value {
    let mut item = json!({ "range": range_to_lsp(&selection.range) });
    if let Some(parent) = selection.parent {
        item["parent"] = selection_range_to_lsp(*parent);
    }
    item
}

fn collect_symbol_folds(sym: &DocumentSymbol, out: &mut Vec<Value>) {
    if sym.range.end.line > sym.range.start.line {
        out.push(json!({
            "startLine": sym.range.start.line,
            "startCharacter": sym.range.start.character,
            "endLine": sym.range.end.line,
            "endCharacter": sym.range.end.character,
            "kind": "region"
        }));
    }
    for child in &sym.children {
        collect_symbol_folds(child, out);
    }
}

fn location_to_lsp(loc: &Location) -> Value {
    json!({
        "uri": uri_from_path(&loc.file),
        "range": range_to_lsp(&loc.range)
    })
}

fn code_action_to_lsp(action: CodeAction) -> Value {
    json!({
        "title": action.title,
        "kind": action.kind,
        "edit": workspace_edit_to_lsp(action.edits)
    })
}

fn workspace_edit_to_lsp(edits: Vec<TextEdit>) -> Value {
    let mut changes = serde_json::Map::new();
    for edit in edits {
        changes
            .entry(uri_from_path(&edit.file))
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("workspace edit entry is always an array")
            .push(json!({
                "range": range_to_lsp(&edit.range),
                "newText": edit.new_text
            }));
    }
    json!({ "changes": changes })
}

fn selection_range_positions(msg: &Value) -> Option<Vec<Position>> {
    let positions = msg.get("params")?.get("positions")?.as_array()?;
    Some(
        positions
            .iter()
            .filter_map(|pos| {
                Some(Position::new(
                    pos.get("line")?.as_u64()? as usize,
                    pos.get("character")?.as_u64()? as usize,
                ))
            })
            .collect(),
    )
}

fn rename_error_to_lsp(err: RenameError) -> Value {
    let message = match err {
        RenameError::UnresolvedSymbol => "No Fortran symbol at cursor".to_string(),
        RenameError::InvalidIdentifier => "New name is not a valid Fortran identifier".to_string(),
        RenameError::ConflictingSymbol { file, range } => format!(
            "Rename would conflict with symbol at {}:{}:{}",
            file.display(),
            range.start.line + 1,
            range.start.character + 1
        ),
    };
    json!({
        "documentChanges": [],
        "failureReason": message
    })
}

fn range_to_lsp(range: &Range) -> Value {
    json!({
        "start": { "line": range.start.line, "character": range.start.character },
        "end": { "line": range.end.line, "character": range.end.character }
    })
}

fn position_to_lsp(position: &Position) -> Value {
    json!({ "line": position.line, "character": position.character })
}

fn symbol_kind(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Module | SymbolKind::Program | SymbolKind::Submodule => 2,
        SymbolKind::Interface => 11,
        SymbolKind::Type => 23,
        SymbolKind::Subroutine | SymbolKind::Function => 12,
        SymbolKind::Method => 6,
        SymbolKind::Variable => 13,
        SymbolKind::Block | SymbolKind::Associate | SymbolKind::SelectType => 3,
        SymbolKind::Use => 2,
    }
}

fn completion_kind(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Module | SymbolKind::Program | SymbolKind::Submodule => 9,
        SymbolKind::Interface | SymbolKind::Type => 7,
        SymbolKind::Subroutine | SymbolKind::Function | SymbolKind::Method => 3,
        SymbolKind::Variable => 6,
        SymbolKind::Block | SymbolKind::Associate | SymbolKind::SelectType => 14,
        SymbolKind::Use => 9,
    }
}

fn identifier_prefix(line: &str, character: usize) -> String {
    let byte_idx = byte_idx_for_utf16_col(line, character);
    let prefix = &line[..byte_idx.min(line.len())];
    let start = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| !(*ch == '_' || ch.is_ascii_alphanumeric()))
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    prefix[start..].to_string()
}

fn byte_idx_for_utf16_col(line: &str, character: usize) -> usize {
    let mut utf16 = 0;
    for (idx, ch) in line.char_indices() {
        if utf16 >= character {
            return idx;
        }
        let next = utf16 + ch.len_utf16();
        if next > character {
            return idx;
        }
        utf16 = next;
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::{collect_fortran_include_roots, fortran_line_length_limits, FortranIndexer};
    use crate::lsp::index::LanguageIndexer;
    use crate::lsp::protocol::uri_from_path;
    use std::path::Path;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn contains_dir(roots: &[std::path::PathBuf], dir: &Path) -> bool {
        let expected = dir.canonicalize().unwrap();
        roots
            .iter()
            .any(|root| root.canonicalize().unwrap() == expected)
    }

    #[test]
    fn fortran_include_roots_follow_manifest_and_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("include")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("fortran-mods")).unwrap();
        std::fs::create_dir_all(root.join("deps/math/include")).unwrap();
        std::fs::create_dir_all(root.join("deps/math/generated")).unwrap();
        std::fs::create_dir_all(root.join(".pkgs/fft/include")).unwrap();

        write(
            &root.join("freight.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"

[compiler]
includes = ["fortran-mods"]

[dependencies]
math = { path = "deps/math", include = ["generated"] }
fft = "1.0"
"#,
        );
        write(
            &root.join("deps/math/freight.toml"),
            r#"
[package]
name = "math"
version = "0.1.0"

[compiler]
includes = ["include"]
"#,
        );
        write(
            &root.join(".pkgs/fft/freight.toml"),
            r#"
[package]
name = "fft"
version = "1.0.0"
"#,
        );

        let roots = collect_fortran_include_roots(root, "debug");
        assert!(contains_dir(&roots, root));
        assert!(contains_dir(&roots, &root.join("include")));
        assert!(contains_dir(&roots, &root.join("src")));
        assert!(contains_dir(&roots, &root.join("fortran-mods")));
        assert!(contains_dir(&roots, &root.join("deps/math")));
        assert!(contains_dir(&roots, &root.join("deps/math/include")));
        assert!(contains_dir(&roots, &root.join("deps/math/generated")));
        assert!(contains_dir(&roots, &root.join(".pkgs/fft")));
        assert!(contains_dir(&roots, &root.join(".pkgs/fft/include")));
    }

    #[test]
    fn fortran_line_length_limits_follow_manifest_language_options() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("freight.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"

[language.fortran]
max_line_length = "12"
max_comment_line_length = "10"
"#,
        );

        assert_eq!(fortran_line_length_limits(root), (Some(12), Some(10)));
    }

    #[test]
    fn fortran_indexer_uses_manifest_line_length_limits_for_diagnostics() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("freight.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"

[language.fortran]
max_line_length = "12"
max_comment_line_length = "10"
"#,
        );
        let path = root.join("app.f90");
        let source = "program app\ninteger :: long_name\n! comment that is too long\nend program";
        write(&path, source);

        let uri = uri_from_path(&path);
        let mut indexer = FortranIndexer::new();
        indexer.refresh_flags(root, "debug");
        indexer.reparse(&uri, source);
        let diagnostics = indexer.diagnostics(&uri);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            diagnostics[0]["message"],
            "Line length exceeds \"max_line_length\" (12)"
        );
        assert_eq!(diagnostics[0]["source"], "fortran-lsp");
        assert_eq!(
            diagnostics[1]["message"],
            "Comment line length exceeds \"max_comment_line_length\" (10)"
        );
    }

    /// Manifest defines ([compiler] + default-feature defines) drive the
    /// fortran-lsp preprocessor: an `#ifdef`-guarded symbol only exists when
    /// the build would define the macro.
    #[test]
    fn fortran_indexer_applies_manifest_defines_to_preprocessor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("freight.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [compiler]\ndefines = [\"API_LEVEL=3\"]\n\n\
             [features]\ndefault = [\"fast\"]\nfast = [\"define:WITH_FAST\"]\n",
        );
        write(
            &root.join("src/guarded.F90"),
            "module m\n#ifdef WITH_FAST\ninteger :: fast_var\n#endif\n\
             #if API_LEVEL >= 3\ninteger :: modern_var\n#endif\nend module",
        );

        let mut indexer = FortranIndexer::new();
        indexer.refresh_flags(root, "debug");
        let symbols = indexer.workspace_symbols("_var").unwrap_or_default();
        let names: Vec<&str> = symbols.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(
            names.contains(&"m::fast_var") && names.contains(&"m::modern_var"),
            "feature + compiler defines must reach the preprocessor, got {names:?}",
        );
    }

    /// `refresh_flags` indexes the whole workspace so cross-file resolution
    /// works with a single file open — and closing a buffer (`evict`) must not
    /// un-index the file for other files that use its modules.
    #[test]
    fn fortran_indexer_indexes_unopened_workspace_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("freight.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        );
        write(
            &root.join("src/answer_mod.f90"),
            "module answer_mod\ncontains\ninteger function answer()\nanswer = 42\n\
             end function\nend module",
        );
        let main = root.join("src/main.f90");
        let main_src = "program main\nuse answer_mod\nprint *, answer()\nend program";
        write(&main, main_src);

        let mut indexer = FortranIndexer::new();
        indexer.refresh_flags(root, "debug");
        // Only main.f90 is "opened"; answer_mod.f90 was indexed by the walk.
        let uri = uri_from_path(&main);
        indexer.reparse(&uri, main_src);

        assert!(
            indexer.diagnostics(&uri).is_empty(),
            "use of a module in an unopened sibling file must resolve",
        );
        let msg = serde_json::json!({
            "params": { "position": { "line": 2, "character": 10 } }
        });
        let def = indexer
            .goto_definition(&uri, &msg)
            .expect("definition into an unopened file");
        assert!(
            def["uri"].as_str().unwrap().ends_with("answer_mod.f90"),
            "definition should land in the walked file, got {def}",
        );

        // Closing the dep buffer keeps the on-disk index.
        indexer.evict(&root.join("src/answer_mod.f90"));
        assert!(
            indexer.diagnostics(&uri).is_empty(),
            "evict must reload from disk, not un-index the module",
        );
    }

    #[test]
    fn fortran_indexer_serves_semantic_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("math.f90");
        let source = "module math\ninteger :: value\nvalue = 1\nend module";
        write(&path, source);

        let uri = uri_from_path(&path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&uri, source);
        let data = indexer
            .semantic_tokens(&uri)
            .expect("semantic tokens for Fortran source");

        assert!(!data.is_empty());
        assert_eq!(data.len() % 5, 0);
    }

    #[test]
    fn fortran_indexer_serves_workspace_symbols() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("math.f90");
        let source = "module math\ncontains\nsubroutine axpy()\nend subroutine\nend module";
        write(&path, source);

        let uri = uri_from_path(&path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&uri, source);
        let symbols = indexer
            .workspace_symbols("ax")
            .expect("workspace symbols for Fortran source");

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0]["name"], "math::axpy");
        assert_eq!(symbols[0]["containerName"], "math");
        assert_eq!(symbols[0]["location"]["uri"], uri);
    }

    #[test]
    fn fortran_indexer_serves_selection_ranges() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("math.f90");
        let source =
            "module math\ncontains\nsubroutine axpy()\ninteger :: value\nvalue = 1\nend subroutine\nend module";
        write(&path, source);

        let uri = uri_from_path(&path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&uri, source);
        let msg = serde_json::json!({
            "params": {
                "textDocument": { "uri": uri },
                "positions": [{ "line": 4, "character": 1 }]
            }
        });
        let ranges = indexer
            .selection_ranges(&uri, &msg)
            .expect("selection ranges for Fortran source");

        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0]["range"]["start"]["line"], 4);
        assert_eq!(ranges[0]["range"]["end"]["character"], 5);
        assert_eq!(ranges[0]["parent"]["range"]["start"]["line"], 2);
        assert_eq!(
            ranges[0]["parent"]["parent"]["parent"]["range"]["start"]["line"],
            0
        );
    }

    #[test]
    fn fortran_indexer_serves_implementation_locations() {
        let tmp = tempfile::tempdir().unwrap();
        let module_path = tmp.path().join("math.f90");
        let impl_path = tmp.path().join("math_impl.f90");
        let module = "module math\ninterface\nmodule subroutine axpy()\nend subroutine\nend interface\nend module";
        let implementation =
            "submodule (math) math_impl\ncontains\nmodule procedure axpy\nend procedure\nend submodule";
        write(&module_path, module);
        write(&impl_path, implementation);

        let module_uri = uri_from_path(&module_path);
        let impl_uri = uri_from_path(&impl_path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&module_uri, module);
        indexer.reparse(&impl_uri, implementation);
        let msg = serde_json::json!({
            "params": {
                "textDocument": { "uri": module_uri },
                "position": { "line": 2, "character": 18 }
            }
        });
        let location = indexer
            .goto_implementation(&module_uri, &msg)
            .expect("implementation location for module procedure prototype");

        assert_eq!(location["uri"], impl_uri);
        assert_eq!(location["range"]["start"]["line"], 2);
        assert_eq!(location["range"]["start"]["character"], 17);
    }

    #[test]
    fn fortran_indexer_serves_inlay_hints() {
        let tmp = tempfile::tempdir().unwrap();
        let module_path = tmp.path().join("math.f90");
        let app_path = tmp.path().join("app.f90");
        let module = "module math\ncontains\nsubroutine axpy(a, x, y)\nend subroutine\nend module";
        let app = "program app\nuse math, only: axpy\ncall axpy(alpha, xs, y=ys)\nend program";
        write(&module_path, module);
        write(&app_path, app);

        let module_uri = uri_from_path(&module_path);
        let app_uri = uri_from_path(&app_path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&module_uri, module);
        indexer.reparse(&app_uri, app);
        let msg = serde_json::json!({
            "params": {
                "textDocument": { "uri": app_uri },
                "range": {
                    "start": { "line": 2, "character": 0 },
                    "end": { "line": 2, "character": 30 }
                }
            }
        });
        let hints = indexer
            .inlay_hints(&app_uri, &msg)
            .expect("inlay hints for Fortran source");

        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0]["label"], "a:");
        assert_eq!(hints[0]["kind"], 2);
        assert_eq!(hints[1]["label"], "x:");
    }

    #[test]
    fn fortran_indexer_serves_highlights_and_folding_ranges() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("math.f90");
        let source =
            "module math\ncontains\nsubroutine axpy()\ninteger :: value\nvalue = value + 1\nend subroutine\nend module";
        write(&path, source);

        let uri = uri_from_path(&path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&uri, source);
        let msg = serde_json::json!({
            "params": {
                "textDocument": { "uri": uri },
                "position": { "line": 4, "character": 1 }
            }
        });
        let highlights = indexer
            .document_highlight(&uri, &msg)
            .expect("document highlights for Fortran source");
        let folds = indexer
            .folding_ranges(&uri)
            .expect("folding ranges for Fortran source");

        assert!(highlights.iter().any(|item| {
            item["range"]["start"]["line"] == 3 && item["range"]["start"]["character"] == 11
        }));
        assert!(highlights.iter().any(|item| {
            item["range"]["start"]["line"] == 4 && item["range"]["start"]["character"] == 0
        }));
        assert!(folds.iter().any(|item| {
            item["startLine"] == 0 && item["endLine"] == 6 && item["kind"] == "region"
        }));
        assert!(folds.iter().any(|item| {
            item["startLine"] == 2 && item["endLine"] == 5 && item["kind"] == "region"
        }));
    }

    #[test]
    fn fortran_indexer_serves_code_actions() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("types.f90");
        let source = "module m\n\
type, abstract :: shape\n\
contains\n\
procedure(draw_iface), deferred :: draw\n\
end type\n\
type, extends(shape) :: circle\n\
end type\n\
end module";
        write(&path, source);

        let uri = uri_from_path(&path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&uri, source);
        let actions = indexer
            .code_actions(
                &uri,
                &serde_json::json!({ "params": { "textDocument": { "uri": uri } } }),
            )
            .expect("code actions for Fortran source");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["kind"], "quickfix");
        assert_eq!(
            actions[0]["title"],
            "Implement deferred procedures for `circle`"
        );
        assert_eq!(
            actions[0]["edit"]["changes"][&uri][0]["newText"],
            "contains\n  procedure :: draw => draw\n"
        );
    }

    #[test]
    fn fortran_indexer_serves_rename_workspace_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let module_path = tmp.path().join("math.f90");
        let app_path = tmp.path().join("app.f90");
        let module = "module math\ncontains\nsubroutine axpy()\nend subroutine axpy\nend module";
        let app = "program app\nuse math, only: axpy\ncall axpy()\nend program";
        write(&module_path, module);
        write(&app_path, app);

        let module_uri = uri_from_path(&module_path);
        let app_uri = uri_from_path(&app_path);
        let mut indexer = FortranIndexer::new();
        indexer.reparse(&module_uri, module);
        indexer.reparse(&app_uri, app);
        let msg = serde_json::json!({
            "params": {
                "textDocument": { "uri": app_uri },
                "position": { "line": 2, "character": 6 },
                "newName": "saxpy"
            }
        });
        let edit = indexer
            .rename(&app_uri, &msg)
            .expect("rename workspace edit for Fortran source");

        let module_edits = edit["changes"][&module_uri].as_array().unwrap();
        let app_edits = edit["changes"][&app_uri].as_array().unwrap();
        assert!(module_edits.iter().any(|item| item["newText"] == "saxpy"));
        assert!(app_edits
            .iter()
            .any(|item| { item["range"]["start"]["line"] == 2 && item["newText"] == "saxpy" }));
    }
}
