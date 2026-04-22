//! LSP backend: document store + `LanguageServer` trait implementation.

use std::path::{Path, PathBuf};

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crane_core::manifest::{load_manifest_str, validate, validate_dep_compat};
use crane_core::toolchain::{CompilerTemplate, load_templates, templates_dir};

use crate::completion;
use crate::docs;
use crate::position::{byte_to_position, locate, position_to_byte};

pub struct Backend {
    client: Client,
    /// Source text of every open document, keyed by URI.
    docs: DashMap<Url, String>,
    /// Compiler templates loaded once at startup — used for backend-name
    /// completion and template-aware validation.
    templates: Vec<CompilerTemplate>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        let templates = templates_dir().map(|d| load_templates(&d)).unwrap_or_default();
        Self { client, docs: DashMap::new(), templates }
    }

    /// Only crane.toml files get the full treatment.
    fn is_crane_toml(uri: &Url) -> bool {
        uri.path().ends_with("/crane.toml") || uri.path().ends_with("crane.toml")
    }

    async fn refresh_diagnostics(&self, uri: Url, src: &str) {
        let diagnostics = self.compute_diagnostics(&uri, src);
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }

    fn compute_diagnostics(&self, uri: &Url, src: &str) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        let manifest = match load_manifest_str(src) {
            Ok(m) => m,
            Err(e) => {
                // Parse error — serde gives us `line N, column M` in many cases;
                // try to pull those out, otherwise point at line 0.
                let (range, msg) = parse_error_range(src, &e.to_string());
                out.push(diagnostic(range, DiagnosticSeverity::ERROR, msg));
                return out;
            }
        };

        let mut errors = validate(&manifest, &self.templates);
        if let Some(dir) = uri.to_file_path().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
            errors.extend(validate_dep_compat(&manifest, &dir, &self.templates));
        }

        for e in errors {
            let range = locate(src, &e.context);
            out.push(diagnostic(range, DiagnosticSeverity::ERROR, e.message));
        }

        out
    }
}

// ── LanguageServer implementation ────────────────────────────────────────────

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "crane-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "[".into(), ".".into(), "\"".into(), "=".into(),
                    ]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "crane-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> { Ok(()) }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text;
        self.docs.insert(uri.clone(), text.clone());
        if Self::is_crane_toml(&uri) {
            self.refresh_diagnostics(uri, &text).await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // With FULL sync, the client sends the entire document each time.
        let Some(change) = params.content_changes.into_iter().next() else { return };
        let uri = params.text_document.uri.clone();
        self.docs.insert(uri.clone(), change.text.clone());
        if Self::is_crane_toml(&uri) {
            self.refresh_diagnostics(uri, &change.text).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if !Self::is_crane_toml(&uri) { return }
        if let Some(text) = self.docs.get(&uri).map(|t| t.clone()) {
            self.refresh_diagnostics(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.remove(&uri);
        // Clear any lingering diagnostics on close.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        if !Self::is_crane_toml(&uri) { return Ok(None) }
        let Some(src) = self.docs.get(&uri).map(|t| t.clone()) else { return Ok(None) };
        let items = completion::complete(
            &src,
            params.text_document_position.position,
            &self.templates,
        );
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        if !Self::is_crane_toml(&uri) { return Ok(None) }
        let Some(src) = self.docs.get(&uri).map(|t| t.clone()) else { return Ok(None) };

        let pos = params.text_document_position_params.position;
        let Some(field_path) = dotted_path_at(&src, pos) else { return Ok(None) };
        let Some(doc) = docs::lookup(&field_path) else { return Ok(None) };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("**`{field_path}`**\n\n{doc}"),
            }),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        if !Self::is_crane_toml(&uri) { return Ok(None) }
        let Some(src) = self.docs.get(&uri).map(|t| t.clone()) else { return Ok(None) };

        let pos = params.text_document_position_params.position;
        // Find the path string literal under the cursor — only inside a `path = "..."` assignment.
        let Some(rel) = path_dep_target_at(&src, pos) else { return Ok(None) };

        let Ok(this_file) = uri.to_file_path() else { return Ok(None) };
        let Some(project_dir) = this_file.parent() else { return Ok(None) };

        let target_dir: PathBuf = project_dir.join(&rel);
        let target = target_dir.join("crane.toml");
        if !target.exists() { return Ok(None) }

        let Ok(target_uri) = Url::from_file_path(&target) else { return Ok(None) };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range: Range { start: Position { line: 0, character: 0 },
                           end:   Position { line: 0, character: 0 } },
        })))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn diagnostic(range: Range, severity: DiagnosticSeverity, message: String) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(severity),
        source: Some("crane".into()),
        message,
        ..Default::default()
    }
}

/// Pull a `line N, column M` hint out of a TOML parse error and return a
/// single-character range at that spot.
fn parse_error_range(src: &str, msg: &str) -> (Range, String) {
    let (line, col) = extract_line_col(msg).unwrap_or((0, 0));
    let start = Position { line, character: col };
    let end = Position { line, character: col + 1 };
    let range = Range { start, end };

    // Strip the "at line X column Y" suffix from the message if we extracted it —
    // editors already display the range separately.
    let cleaned = msg
        .split(" at line ")
        .next()
        .unwrap_or(msg)
        .trim()
        .to_string();
    let _ = src;
    (range, cleaned)
}

fn extract_line_col(msg: &str) -> Option<(u32, u32)> {
    // serde_toml messages look like "... at line 3 column 5".
    let idx = msg.find("line ")?;
    let rest = &msg[idx + 5..];
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let line: u32 = parts.next()?.parse().ok()?;
    let col_idx = rest.find("column ")?;
    let col_rest = &rest[col_idx + 7..];
    let col: u32 = col_rest
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    // Convert 1-based to 0-based.
    Some((line.saturating_sub(1), col.saturating_sub(1)))
}

/// Figure out which dotted manifest path the cursor is on so we can look up hover docs.
/// Returns `"package.name"`, `"compiler.backend"`, `"dependencies"`, etc.
fn dotted_path_at(src: &str, pos: Position) -> Option<String> {
    let byte = position_to_byte(src, pos);
    let before = &src[..byte];

    // Most recent header.
    let section = before.lines().rev().find_map(|l| parse_header(l.trim()));

    // Current line.
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = src[byte..].find('\n').map(|i| byte + i).unwrap_or(src.len());
    let line = &src[line_start..line_end];

    // Inside a header → return the header name itself (`[compiler]` → `compiler`).
    let trimmed = line.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return parse_header(trimmed);
    }

    // On an assignment line — return `<section>.<key>`.
    if let Some(eq) = line.find('=') {
        let key = line[..eq].trim().trim_matches('"').to_string();
        if key.is_empty() { return None }
        return match section {
            Some(s) => Some(format!("{s}.{key}")),
            None => Some(key),
        };
    }

    None
}

fn parse_header(line: &str) -> Option<String> {
    let s = line.strip_prefix("[[").and_then(|s| s.strip_suffix("]]"))
        .or_else(|| line.strip_prefix('[').and_then(|s| s.strip_suffix(']')))?;
    // Trim the array index off e.g. "bin" so docs keys line up.
    Some(s.to_string())
}

/// If the cursor is inside a `path = "<value>"` string literal, return `<value>`.
fn path_dep_target_at(src: &str, pos: Position) -> Option<String> {
    let byte = position_to_byte(src, pos);
    let line_start = src[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = src[byte..].find('\n').map(|i| byte + i).unwrap_or(src.len());
    let line = &src[line_start..line_end];

    // Accept `path = "..."` anywhere on the line — covers both
    // `mylib = { path = "../mylib" }` and standalone `path = "..."`.
    let eq_pos = line.find("path")?;
    let rest = &line[eq_pos..];
    let first_quote = rest.find('"')?;
    let after_quote = &rest[first_quote + 1..];
    let end_quote = after_quote.find('"')?;

    // Cursor byte relative to the line.
    let cursor_in_line = byte - line_start;
    let val_start = eq_pos + first_quote + 1;
    let val_end = val_start + end_quote;
    if cursor_in_line < val_start || cursor_in_line > val_end { return None }

    Some(after_quote[..end_quote].to_string())
}

// Silence the dead_code warning — byte_to_position is re-exported for the
// tests in position.rs and may be handy for future diagnostic ranges.
#[allow(dead_code)]
fn _keep(src: &str) -> Position { byte_to_position(src, src.len()) }
