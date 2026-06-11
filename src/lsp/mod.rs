//! `freight lsp` — Language Server Protocol multiplexer for freight.toml and
//! source files (clangd, fortls, asm-lsp passthroughs).

mod index;
pub mod indexers;
pub mod log;
mod manifest;
mod protocol;

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use indexers::ClangIndexer;
use index::LanguageIndexer;

use crate::build::generate_lsp_compile_commands_at;
use crate::manifest::types::{Dependency, Manifest};
use crate::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use crate::toolchain::{detect_all_cached, load_all_templates};
use serde_json::{json, Value};

use index::{
    include_hover_markdown, include_inlay_label, parse_include_header,
    HeaderDirSpec, HeaderEntry, HeaderIndex, HeaderOrigin,
};
use manifest::{
    completion_result, hover_result, manifest_diagnostics, signature_help_result,
    WorkspaceInventory, WorkspacePackage,
};
use protocol::*;

pub(crate) const INTERNAL_ID_PREFIX: &str = "__freight_";
const INTERNAL_CLANGD_INIT_ID: &str = "__freight_clangd_initialize";
const INTERNAL_FORTLS_INIT_ID: &str = "__freight_fortls_initialize";
const INTERNAL_ASM_LSP_INIT_ID: &str = "__freight_asm_lsp_initialize";

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct Args {
    #[arg(long, default_value = "clangd")]
    pub clangd: String,
    #[arg(long)]
    pub no_clangd: bool,
    #[arg(long, default_value = "fortls")]
    pub fortls: String,
    #[arg(long)]
    pub no_fortls: bool,
    #[arg(long, default_value = "asm-lsp")]
    pub asm_lsp: String,
    #[arg(long)]
    pub no_asm_lsp: bool,
    #[arg(long, default_value = "dev")]
    pub profile: String,
    /// Use the in-process clang bridge to serve C/C++ language features (hover,
    /// goto, completion, document symbols, folding, references, highlight,
    /// semantic tokens, inlay hints, diagnostics) instead of forwarding to
    /// clangd. Off by default while the bridge matures — clangd is the reliable
    /// path; enable this to test or use the bridge.
    #[arg(long)]
    pub use_clang_bridge: bool,
    /// Extra flags forwarded verbatim to clangd (repeatable).
    /// E.g. `--clangd-arg=--hover-style=detailed`
    #[arg(long = "clangd-arg", value_name = "ARG")]
    pub clangd_args: Vec<String>,
    /// Accepted for compatibility with LSP clients that append --stdio.
    #[arg(long, hide = true)]
    pub stdio: bool,
    /// Write PID to /tmp/freight-lsp-debug.pid and busy-wait until a debugger
    /// attaches. Used by the VS Code extension in development mode.
    #[arg(long, hide = true)]
    pub wait_for_debugger: bool,
}

impl Args {
    pub fn run(self) {
        if self.wait_for_debugger {
            wait_for_debugger();
        }
        // Build `out` first so we can share it with the log layer.
        let out = Arc::new(Mutex::new(io::stdout()));
        log::init_lsp_logging(Arc::clone(&out));
        tracing::info!("freight lsp starting");
        if let Err(e) = Server::with_out(self, out).run() {
            eprintln!("freight lsp: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Debugger wait
// ---------------------------------------------------------------------------

/// Write PID to a well-known file and spin until a debugger attaches.
/// Allows the VS Code extension to attach CodeLLDB before the LSP loop starts.
/// Whether `clangd_bin --help` advertises `flag`. Used to gate recent flags so
/// an older clangd (which would exit on an unknown flag) is left untouched.
fn clangd_supports_flag(clangd_bin: &str, flag: &str) -> bool {
    Command::new(clangd_bin)
        .arg("--help")
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).contains(flag)
                || String::from_utf8_lossy(&o.stderr).contains(flag)
        })
        .unwrap_or(false)
}

fn wait_for_debugger() {
    let pid = std::process::id();
    let pid_file = "/tmp/freight-lsp-debug.pid";
    let _ = std::fs::write(pid_file, pid.to_string());
    eprintln!("freight lsp: waiting for debugger (PID {pid}) — attach now");

    // Spin until TracerPid in /proc/self/status is non-zero (Linux).
    // On other platforms just sleep 10 s and trust the user attached in time.
    #[cfg(target_os = "linux")]
    {
        loop {
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                let attached = status
                    .lines()
                    .find(|l| l.starts_with("TracerPid:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(0);
                if attached != 0 {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    #[cfg(not(target_os = "linux"))]
    std::thread::sleep(std::time::Duration::from_secs(10));

    let _ = std::fs::remove_file(pid_file);
    eprintln!("freight lsp: debugger attached, continuing");
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

struct Server {
    args: Args,
    out: Arc<Mutex<io::Stdout>>,
    state: ServerState,
}

/// Per-URI merged diagnostic state.
///
/// Both fields are updated independently: `clangd` whenever the passthrough
/// reader receives a `publishDiagnostics` notification from clangd, `tidy`
/// when the background clang-tidy thread finishes. Every update re-publishes
/// the union to the client so the client always sees a consistent set.
#[derive(Default)]
struct DiagCache {
    clangd:  Vec<Value>,
    tidy:    Vec<Value>,
    /// Freight-generated diagnostics (e.g. undeclared-include warnings).
    freight: Vec<Value>,
}

struct ServerState {
    root_dir: PathBuf,
    manifest_dir: Option<PathBuf>,
    compile_commands_dir: Option<PathBuf>,
    docs: HashMap<String, String>,
    templates: Vec<crate::toolchain::CompilerTemplate>,
    clangd: Option<Passthrough>,
    fortls: Option<Passthrough>,
    asm_lsp: Option<Passthrough>,
    /// Header → package mapping for `#include` hover.
    header_index: HeaderIndex,
    workspace_inventory: WorkspaceInventory,
    /// Pending clangd intercepts: rewritten-id → request metadata.
    /// Shared with the clangd reader thread so it can merge clangd's semantic
    /// answers with Freight package/docs context before forwarding.
    clangd_pending: Arc<Mutex<HashMap<String, PendingClangdRequest>>>,
    /// Merged diagnostic cache shared between the main loop, the clangd
    /// passthrough reader thread, and background clang-tidy threads.
    diag_cache: Arc<Mutex<HashMap<String, DiagCache>>>,

    /// Per-language indexers; iterated in order for each LSP request.
    indexers: Vec<Box<dyn LanguageIndexer>>,

    /// Cached compiler built-in include dirs, probed once (used by the
    /// include-hygiene check to confirm an undeclared header exists).
    system_include_dirs: Option<Vec<PathBuf>>,
}

struct Passthrough {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

enum PendingClangdRequest {
    InlayHint {
        original_id: Value,
        freight_hints: Vec<Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceServer {
    Clangd,
    Fortls,
    AsmLsp,
}

// ---------------------------------------------------------------------------
// Server impl
// ---------------------------------------------------------------------------

impl Server {
    fn with_out(args: Args, out: Arc<Mutex<io::Stdout>>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let manifest_dir = find_manifest_dir(&cwd);
        let root_dir = manifest_dir.clone().unwrap_or(cwd);
        // Off by default: clangd serves C/C++. The clang-bridge indexer is only
        // registered when explicitly opted in, so every indexer-backed handler
        // (hover/goto/completion/documentSymbol/folding/references/highlight/
        // semanticTokens/inlay/diagnostics) falls through to the clangd forward.
        let indexers: Vec<Box<dyn LanguageIndexer>> = if args.use_clang_bridge {
            vec![Box::new(ClangIndexer::new())]
        } else {
            vec![]
        };
        Self {
            args,
            out,
            state: ServerState {
                root_dir,
                manifest_dir,
                compile_commands_dir: None,
                docs: HashMap::new(),
                templates: load_all_templates(),
                clangd: None,
                fortls: None,
                asm_lsp: None,
                header_index: HeaderIndex::default(),
                workspace_inventory: WorkspaceInventory::default(),
                clangd_pending: Arc::new(Mutex::new(HashMap::new())),
                diag_cache: Arc::new(Mutex::new(HashMap::new())),
                indexers,
                system_include_dirs: None,
            },
        }
    }

    fn run(mut self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut input = BufReader::new(stdin.lock());
        while let Some(msg) = read_lsp_message(&mut input)? {
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            if method.is_empty() {
                if !is_internal_client_response(&msg) {
                    self.forward_to_all_passthroughs(&msg)?;
                }
                continue;
            }
            match level_for_method(method) {
                tracing::Level::INFO => tracing::info!(method, "← client"),
                _ => tracing::debug!(method, "← client"),
            }
            match method {
                "initialize" => self.handle_initialize(msg)?,
                "initialized" => {
                    self.register_manifest_file_watcher()?;
                    self.forward_to_all_passthroughs(&msg)?;
                }
                "shutdown" => self.handle_shutdown(msg)?,
                "exit" => {
                    self.forward_to_all_passthroughs(&msg)?;
                    break;
                }
                "textDocument/didOpen" => self.handle_did_open(msg)?,
                "textDocument/didChange" => self.handle_did_change(msg)?,
                "textDocument/didSave" => self.handle_did_save(msg)?,
                "textDocument/didClose" => self.handle_did_close(msg)?,
                "workspace/didChangeWatchedFiles" => self.handle_watched_files_changed(msg)?,
                "textDocument/completion" => self.handle_completion_or_forward(msg)?,
                "textDocument/hover" => self.handle_hover_or_forward(msg)?,
                "textDocument/signatureHelp" => self.handle_signature_help_or_forward(msg)?,
                "textDocument/codeAction" => self.handle_code_action_or_forward(msg)?,
                "textDocument/inlayHint" => self.handle_inlay_hints(msg)?,
                "textDocument/definition" => self.handle_definition_or_forward(msg)?,
                "textDocument/declaration" => self.handle_definition_or_forward(msg)?,
                "textDocument/documentLink" => self.handle_document_links(msg)?,
                "textDocument/documentSymbol" => self.handle_document_symbol(msg)?,
                "textDocument/foldingRange" => self.handle_folding_range(msg)?,
                "textDocument/references" => self.handle_references(msg)?,
                "textDocument/documentHighlight" => self.handle_document_highlight(msg)?,
                "textDocument/semanticTokens/full" => self.handle_semantic_tokens(msg)?,
                "freight/workspaceInfo" => self.handle_workspace_info(msg)?,
                "freight/setConfig" => self.handle_set_config(msg)?,
                _ => self.forward_or_null(msg)?,
            }
        }
        tracing::info!("freight lsp shutting down");
        self.kill_passthroughs();
        Ok(())
    }

    fn handle_initialize(&mut self, msg: Value) -> io::Result<()> {
        if let Some(root) = root_from_initialize(&msg) {
            self.state.root_dir = root;
            self.state.manifest_dir = find_manifest_dir(&self.state.root_dir);
        }
        self.refresh_workspace_inventory();
        self.refresh_compile_commands();
        let mut source_caps = Vec::new();
        if !self.args.no_clangd {
            if let Some(caps) = self.start_clangd(&msg) {
                source_caps.push(caps);
            }
        }
        if !self.args.no_fortls {
            if let Some(caps) = self.start_fortls(&msg) {
                source_caps.push(caps);
            }
        }
        if !self.args.no_asm_lsp {
            if let Some(caps) = self.start_asm_lsp(&msg) {
                source_caps.push(caps);
            }
        }
        let capabilities = merged_capabilities(source_caps, self.args.use_clang_bridge);
        self.respond(
            msg.get("id").cloned(),
            json!({
                "capabilities": capabilities,
                "serverInfo": { "name": "freight", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
    }

    fn handle_shutdown(&mut self, msg: Value) -> io::Result<()> {
        self.shutdown_passthroughs();
        self.respond(msg.get("id").cloned(), Value::Null)
    }

    fn handle_did_open(&mut self, msg: Value) -> io::Result<()> {
        if let Some((uri, text)) = opened_text(&msg) {
            if is_freight_manifest_uri(&uri) {
                self.state.docs.insert(uri.clone(), text);
                self.publish_manifest_diagnostics(&uri)?;
                return Ok(());
            }
            // If we don't have a manifest dir yet, try to find one from the opened file.
            if self.state.manifest_dir.is_none() {
                if let Some(path) = path_from_uri(&uri) {
                    if let Some(parent) = path.parent() {
                        if let Some(found) = find_manifest_dir(parent) {
                            tracing::info!(
                                path = %found.display(),
                                "manifest dir discovered from opened file"
                            );
                            self.state.manifest_dir = Some(found);
                            self.refresh_compile_commands();
                        }
                    }
                }
            }
            // Flag undeclared #includes (no-op for non-C/C++).
            self.compute_include_hygiene(&uri, &text);
            // Keep the live buffer so include/import hints reflect unsaved edits.
            self.state.docs.insert(uri, text);
        }
        self.forward_by_text_document(&msg)
    }

    fn handle_did_change(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_to_all_passthroughs(&msg);
        };
        if !is_freight_manifest_uri(&uri) {
            // Reparse the clang-bridge TU so hover/completion reflect the live buffer.
            // Full-text sync (change: 1) guarantees changed_full_text is always present.
            if let Some(text) = changed_full_text(&msg) {
                for ix in &mut self.state.indexers { ix.reparse(&uri, &text); }
                self.compute_include_hygiene(&uri, &text);
                self.state.docs.insert(uri.clone(), text);
            }
            return self.forward_by_uri(&uri, &msg);
        }
        if let Some(text) = changed_full_text(&msg) {
            for ix in &mut self.state.indexers { ix.reparse(&uri, &text); }
            self.state.docs.insert(uri.clone(), text);
            self.publish_manifest_diagnostics(&uri)?;
        }
        Ok(())
    }

    fn handle_did_save(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_to_all_passthroughs(&msg);
        };
        if !is_freight_manifest_uri(&uri) {
            self.forward_by_uri(&uri, &msg)?;
            self.spawn_tidy(&uri);
            // Recompute undeclared-include diagnostics from the saved contents.
            let text = msg
                .get("params")
                .and_then(|p| p.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| path_from_uri(&uri).and_then(|p| std::fs::read_to_string(p).ok()));
            if let Some(text) = text {
                self.compute_include_hygiene(&uri, &text);
                self.state.docs.insert(uri.clone(), text);
            }
            return Ok(());
        }
        if let Some(text) = msg
            .get("params")
            .and_then(|p| p.get("text"))
            .and_then(Value::as_str)
        {
            self.state.docs.insert(uri.clone(), text.to_string());
        } else if let Some(path) = path_from_uri(&uri) {
            if let Ok(text) = std::fs::read_to_string(path) {
                self.state.docs.insert(uri.clone(), text);
            }
        }
        self.publish_manifest_diagnostics(&uri)?;
        self.refresh_compile_commands();
        self.notify_compile_commands_changed()
    }

    fn handle_did_close(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_to_all_passthroughs(&msg);
        };
        if is_freight_manifest_uri(&uri) {
            self.state.docs.remove(&uri);
            self.publish_diagnostics(&uri, vec![])?;
            return Ok(());
        }
        self.state.docs.remove(&uri);
        if let Some(path) = path_from_uri(&uri) {
            for ix in &mut self.state.indexers { ix.evict(&path); }
        }
        self.forward_by_uri(&uri, &msg)
    }

    fn handle_watched_files_changed(&mut self, msg: Value) -> io::Result<()> {
        let manifest_changes: Vec<String> = msg
            .get("params")
            .and_then(|p| p.get("changes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.get("uri").and_then(Value::as_str))
            .filter(|uri| is_freight_manifest_uri(uri))
            .map(ToString::to_string)
            .collect();
        if manifest_changes.is_empty() {
            return self.forward_to_all_passthroughs(&msg);
        }
        for uri in manifest_changes {
            if let Some(path) = path_from_uri(&uri) {
                if let Some(parent) = path.parent() {
                    if !self.root_is_workspace() {
                        self.state.manifest_dir = Some(parent.to_path_buf());
                    }
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    self.state.docs.insert(uri.clone(), text);
                }
            }
            self.publish_manifest_diagnostics(&uri)?;
        }
        self.refresh_workspace_inventory();
        self.refresh_compile_commands();
        self.notify_compile_commands_changed()
    }

    fn handle_completion_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if is_freight_manifest_uri(&uri) {
            let text = self.manifest_text(&uri);
            let result = completion_result(
                text.as_deref(),
                position(&msg),
                Some(&self.state.workspace_inventory),
            );
            return self.respond(msg.get("id").cloned(), result);
        }
        if let Some(result) = self.state.indexers.iter_mut().find_map(|ix| ix.completion(&uri, &msg)) {
            return self.respond(msg.get("id").cloned(), result);
        }
        self.forward_or_null(msg)
    }

    fn handle_hover_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };

        if is_freight_manifest_uri(&uri) {
            let text = self.manifest_text(&uri);
            let result = hover_result(text.as_deref(), position(&msg));
            return self.respond(msg.get("id").cloned(), result.unwrap_or(Value::Null));
        }

        if let Some(result) = self.state.indexers.iter_mut().find_map(|ix| ix.hover(&uri, &msg)) {
            return self.respond(msg.get("id").cloned(), result);
        }

        // Forward all hover requests directly to the passthrough server.
        match source_server_for_uri(&uri) {
            Some(SourceServer::Clangd)
            | Some(SourceServer::Fortls)
            | Some(SourceServer::AsmLsp) => self.forward_by_uri(&uri, &msg),
            _ => self.respond(msg.get("id").cloned(), Value::Null),
        }
    }


    /// Go-to-definition / go-to-declaration on an `#include` line:
    /// jump directly to the header file. Falls through to clangd for non-include lines.
    fn handle_definition_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if let Some((line, _)) = position(&msg) {
            if let Some(location) = self.include_definition(&uri, line) {
                return self.respond(msg.get("id").cloned(), location);
            }
        }
        if let Some(location) = self.state.indexers.iter_mut().find_map(|ix| ix.goto_definition(&uri, &msg)) {
            return self.respond(msg.get("id").cloned(), location);
        }
        self.forward_by_uri(&uri, &msg)
    }

    fn include_definition(&self, uri: &str, line: usize) -> Option<Value> {
        let path = path_from_uri(uri)?;
        let text = self.doc_text(uri, &path)?;
        let line_text = text.lines().nth(line)?;
        let (header, is_system) = parse_include_header(line_text)?;

        let full_path = if let Some(e) = self.state.header_index.lookup(&header) {
            e.full_path.clone()
        } else if is_system {
            self.state.header_index.lookup_system(&header)?.full_path
        } else {
            return None;
        };

        if full_path.as_os_str().is_empty() || !full_path.exists() {
            return None;
        }
        let target_uri = uri_from_path(&full_path);
        tracing::debug!(header, target = %full_path.display(), "include definition");
        Some(json!({
            "uri": target_uri,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }
        }))
    }

    // ── Indexer delegates ─────────────────────────────────────────────────────

    fn refresh_indexer_flags(&mut self) {
        let Some(ref manifest_dir) = self.state.manifest_dir.clone() else { return; };
        let profile = self.args.profile.clone();
        for ix in &mut self.state.indexers {
            ix.refresh_flags(&manifest_dir, &profile);
        }
    }

    /// Document links for the entire file: freight provides links for
    /// `#include`/`import` lines; clangd is not involved.
    fn handle_document_links(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned();
        let links = self.compute_document_links(&msg).unwrap_or_default();
        self.respond(id, Value::Array(links))
    }

    fn compute_document_links(&self, msg: &Value) -> Option<Vec<Value>> {
        let uri = text_document_uri(msg)?;
        let path = path_from_uri(&uri)?;
        let text = self.doc_text(&uri, &path)?;

        let mut links = Vec::new();
        for (idx, line_text) in text.lines().enumerate() {
            let Some((header, is_system)) = parse_include_header(line_text) else {
                continue;
            };

            let full_path = if let Some(e) = self.state.header_index.lookup(&header) {
                e.full_path.clone()
            } else if is_system {
                match self.state.header_index.lookup_system(&header) {
                    Some(e) => e.full_path,
                    None => continue,
                }
            } else {
                continue;
            };

            if full_path.as_os_str().is_empty() || !full_path.exists() {
                continue;
            }

            // Range covers just the header name inside the delimiters.
            let trimmed = line_text.trim();
            let name_start = line_text.find(&header).unwrap_or(0);
            let name_end = name_start + header.len();
            links.push(json!({
                "range": {
                    "start": { "line": idx, "character": name_start },
                    "end":   { "line": idx, "character": name_end }
                },
                "target": uri_from_path(&full_path),
                "tooltip": trimmed
            }));
        }
        Some(links)
    }

    /// `textDocument/documentSymbol` — prefer a language indexer (clang-bridge
    /// for C/C++), otherwise forward to the source server (clangd/fortls/…).
    fn handle_document_symbol(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let uri = text_document_uri(&msg);
        let result = uri.as_deref().and_then(|u| {
            self.state.indexers.iter_mut().find_map(|ix| ix.document_symbols(u))
        });
        if let Some(syms) = result {
            return self.respond(Some(id), Value::Array(syms));
        }
        self.forward_or_null(msg)
    }

    /// `textDocument/foldingRange` — prefer a language indexer, else forward.
    fn handle_folding_range(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let uri = text_document_uri(&msg);
        let result = uri.as_deref().and_then(|u| {
            self.state.indexers.iter_mut().find_map(|ix| ix.folding_ranges(u))
        });
        if let Some(ranges) = result {
            return self.respond(Some(id), Value::Array(ranges));
        }
        self.forward_or_null(msg)
    }

    /// `textDocument/references` — prefer a language indexer, else forward.
    fn handle_references(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let uri = text_document_uri(&msg);
        let result = uri.as_deref().and_then(|u| {
            self.state.indexers.iter_mut().find_map(|ix| ix.references(u, &msg))
        });
        if let Some(locs) = result {
            return self.respond(Some(id), Value::Array(locs));
        }
        self.forward_or_null(msg)
    }

    /// `textDocument/documentHighlight` — prefer a language indexer, else forward.
    fn handle_document_highlight(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let uri = text_document_uri(&msg);
        let result = uri.as_deref().and_then(|u| {
            self.state.indexers.iter_mut().find_map(|ix| ix.document_highlight(u, &msg))
        });
        if let Some(hls) = result {
            return self.respond(Some(id), Value::Array(hls));
        }
        self.forward_or_null(msg)
    }

    /// `textDocument/semanticTokens/full` — prefer a language indexer, else
    /// forward.  The indexer returns the LSP-encoded `data` array directly.
    fn handle_semantic_tokens(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let uri = text_document_uri(&msg);
        let result = uri.as_deref().and_then(|u| {
            self.state.indexers.iter_mut().find_map(|ix| ix.semantic_tokens(u))
        });
        if let Some(data) = result {
            return self.respond(Some(id), json!({ "data": data }));
        }
        self.forward_or_null(msg)
    }

    fn handle_inlay_hints(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);

        // #include / import annotation hints (e.g. "← stdlib", "← vecmath-1.0").
        let include_hints = self.compute_inlay_hints(&msg).unwrap_or_default();

        // Source-code hints (parameter names, deduced types) from whichever
        // language indexer handles this file. Clang-bridge provides these for
        // C/C++ with filtered, correct labels (no system-header names, main-file
        // only). When an indexer covers the file we skip clangd for hints so
        // its unfiltered output does not overwrite ours.
        let uri = text_document_uri(&msg);
        let source_hints: Option<Vec<Value>> = uri.as_deref().and_then(|u| {
            self.state.indexers.iter_mut().find_map(|ix| ix.inlay_hints(u, &msg))
        });

        if let Some(mut all) = source_hints {
            all.extend(include_hints);
            return self.respond(Some(id), Value::Array(all));
        }

        // No indexer covered this file — fall back to the old clangd-merge path.
        let goes_to_clangd = uri
            .as_deref()
            .map(|u| matches!(source_server_for_uri(u), Some(SourceServer::Clangd)))
            .unwrap_or(false);

        if goes_to_clangd && self.state.clangd.is_some() {
            let orig_id_str = match &id {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => "0".to_string(),
            };
            let rewritten = format!("__freight_inlayhint_{orig_id_str}");
            self.state.clangd_pending.lock().unwrap().insert(
                rewritten.clone(),
                PendingClangdRequest::InlayHint {
                    original_id: id,
                    freight_hints: include_hints,
                },
            );
            let mut fwd = msg;
            fwd.as_object_mut()
                .unwrap()
                .insert("id".to_string(), json!(rewritten));
            self.forward_to_passthrough(SourceServer::Clangd, &fwd)?;
        } else {
            self.respond(Some(id), Value::Array(include_hints))?;
        }
        Ok(())
    }

    fn compute_inlay_hints(&self, msg: &Value) -> Option<Vec<Value>> {
        let uri = text_document_uri(msg)?;
        let path = path_from_uri(&uri)?;
        let text = self.doc_text(&uri, &path)?;

        let range = msg.get("params")?.get("range")?;
        let start_line = range.get("start")?.get("line")?.as_u64()? as usize;
        let end_line = range.get("end")?.get("line")?.as_u64()? as usize;

        let mut hints = Vec::new();
        for (idx, line_text) in text.lines().enumerate() {
            if idx < start_line || idx > end_line {
                continue;
            }
            let Some((header, is_system)) = parse_include_header(line_text) else {
                continue;
            };

            let owned;
            let entry: &HeaderEntry = if let Some(e) = self.state.header_index.lookup(&header) {
                e
            } else if is_system {
                // File-based system lookup (e.g. <vector> → /usr/include/c++/.../vector).
                // Named C++20 modules (e.g. `import std.core`) won't be found this way;
                // synthesise a System entry so we still show "← stdlib".
                owned = self
                    .state
                    .header_index
                    .lookup_system(&header)
                    .unwrap_or_else(|| HeaderEntry {
                        package_name: "stdlib".to_string(),
                        package_version: None,
                        full_path: std::path::PathBuf::new(),
                        origin: HeaderOrigin::System,
                        dep_key: None,
                    });
                &owned
            } else {
                continue;
            };

            let label = include_inlay_label(entry);
            // Position at end of line.
            let col = line_text.len();
            hints.push(json!({
                "position": { "line": idx, "character": col },
                "label": label,
                "kind": 2,       // Parameter kind — renders as dimmed text
                "paddingLeft": true,
                "tooltip": {
                    "kind": "markdown",
                    "value": include_hover_markdown(&header, entry)
                }
            }));
        }
        Some(hints)
    }

    fn handle_signature_help_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if !is_freight_manifest_uri(&uri) {
            return self.forward_by_uri(&uri, &msg);
        }
        let text = self.manifest_text(&uri);
        let result = signature_help_result(text.as_deref(), position(&msg));
        self.respond(msg.get("id").cloned(), result.unwrap_or(Value::Null))
    }

    fn handle_code_action_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if is_freight_manifest_uri(&uri) {
            return self.respond(msg.get("id").cloned(), json!([]));
        }
        self.forward_by_uri(&uri, &sanitize_code_action_diagnostics(&msg))
    }

    fn handle_workspace_info(&self, msg: Value) -> io::Result<()> {
        let packages: Vec<Value> = self
            .state
            .workspace_inventory
            .packages
            .iter()
            .map(|pkg| json!({ "name": pkg.name, "path": pkg.path, "lib": pkg.lib, "bins": pkg.bins }))
            .collect();

        // Detected compiler families available on this machine.
        let toolchains: Vec<Value> = {
            use crate::toolchain::group_into_toolchains;
            let detected = detect_all_cached(&self.state.templates);
            let groups = group_into_toolchains(detected);
            groups.toolchains.iter().map(|tc| {
                let compiler = tc.compilers.first();
                json!({
                    "family":   tc.name,
                    "path":     compiler.map(|c| c.path.to_string_lossy().into_owned()).unwrap_or_default(),
                    "version":  compiler.map(|c| c.version.clone()).unwrap_or_default(),
                    "languages": tc.languages,
                })
            }).collect()
        };

        // Current sysroot from the active manifest's [compiler] section.
        let manifest_dir = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        let current_sysroot: Option<String> = load_manifest(&manifest_dir)
            .ok()
            .and_then(|m| m.compiler.sysroot);

        self.respond(
            msg.get("id").cloned(),
            json!({
                "schemaVersion": 2,
                "root": manifest_dir,
                "packages": packages,
                "toolchains": toolchains,
                "sysroot": current_sysroot,
            }),
        )
    }

    /// `freight/setConfig` — write a single config key to the project's `freight.toml`.
    ///
    /// Params: `{ "key": "compiler.sysroot", "value": "/path" | null }`
    fn handle_set_config(&self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let key = params.get("key").and_then(Value::as_str).unwrap_or("");
        let value = params.get("value");

        let manifest_dir = match self.active_manifest_dir() {
            Some(d) => d,
            None => {
                return self.respond(id, json!({"error": "no freight.toml found"}));
            }
        };
        let manifest_path = manifest_dir.join("freight.toml");

        match set_manifest_config(&manifest_path, key, value) {
            Ok(()) => self.respond(id, json!({"ok": true})),
            Err(e) => self.respond(id, json!({"error": e.to_string()})),
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest config helper
// ---------------------------------------------------------------------------

/// Write or clear a single dotted key in `freight.toml` using `toml_edit` so
/// that comments and formatting are preserved.
///
/// Supported keys:
///   `compiler.sysroot` — `Option<String>` in `[compiler]`
fn set_manifest_config(path: &Path, key: &str, value: Option<&Value>) -> anyhow::Result<()> {
    use toml_edit::{value as tv, DocumentMut, Item, Table};

    let src = std::fs::read_to_string(path)?;
    let mut doc: DocumentMut = src.parse()?;

    match key {
        "compiler.sysroot" => {
            match value.and_then(Value::as_str) {
                Some(s) => {
                    // Ensure [compiler] table exists.
                    if doc.get("compiler").is_none() {
                        doc["compiler"] = Item::Table(Table::new());
                    }
                    doc["compiler"]["sysroot"] = tv(s.to_owned());
                }
                None => {
                    // Clear: remove the key if present.
                    if let Some(Item::Table(t)) = doc.get_mut("compiler") {
                        t.remove("sysroot");
                    }
                }
            }
        }
        other => anyhow::bail!("unknown config key: {other}"),
    }

    std::fs::write(path, doc.to_string())?;
    Ok(())
}

fn merge_clangd_inlay_response(
    mut msg: Value,
    original_id: Value,
    freight_hints: Vec<Value>,
) -> Value {
    // Collect the line numbers Freight already covers (#include / import lines)
    // so clangd's hints on those same lines are dropped in favour of ours.
    let freight_lines: std::collections::HashSet<u64> = freight_hints
        .iter()
        .filter_map(|h| h.get("position")?.get("line")?.as_u64())
        .collect();

    let clangd_hints: Vec<Value> = msg
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| {
            let line = h
                .get("position")
                .and_then(|p| p.get("line"))
                .and_then(Value::as_u64);
            !matches!(line, Some(l) if freight_lines.contains(&l))
        })
        .collect();

    let mut merged = clangd_hints;
    merged.extend(freight_hints);
    if let Some(obj) = msg.as_object_mut() {
        obj.insert("id".to_string(), original_id);
        obj.insert("result".to_string(), Value::Array(merged));
    }
    msg
}

impl Server {
    fn forward_or_null(&mut self, msg: Value) -> io::Result<()> {
        if let Some(uri) = text_document_uri(&msg) {
            self.forward_by_uri(&uri, &msg)
        } else if msg.get("method").is_some() && msg.get("id").is_none() {
            self.forward_to_all_passthroughs(&msg)
        } else if msg.get("id").is_some() {
            self.respond(msg.get("id").cloned(), Value::Null)
        } else {
            Ok(())
        }
    }

    fn forward_by_text_document(&mut self, msg: &Value) -> io::Result<()> {
        if let Some(uri) = text_document_uri(msg) {
            self.forward_by_uri(&uri, msg)
        } else {
            self.forward_to_all_passthroughs(msg)
        }
    }

    fn forward_by_uri(&mut self, uri: &str, msg: &Value) -> io::Result<()> {
        let Some(kind) = source_server_for_uri(uri) else {
            return self.respond_null_if_request(msg);
        };
        self.forward_to_passthrough(kind, msg).and_then(|sent| {
            if sent {
                Ok(())
            } else {
                self.respond_null_if_request(msg)
            }
        })
    }

    fn forward_to_passthrough(&mut self, kind: SourceServer, msg: &Value) -> io::Result<bool> {
        let server = match kind {
            SourceServer::Clangd => self.state.clangd.as_ref(),
            SourceServer::Fortls => self.state.fortls.as_ref(),
            SourceServer::AsmLsp => self.state.asm_lsp.as_ref(),
        };
        let Some(server) = server else {
            return Ok(false);
        };
        write_lsp_message(&mut *server.stdin.lock().unwrap(), msg)?;
        Ok(true)
    }

    fn forward_to_all_passthroughs(&mut self, msg: &Value) -> io::Result<()> {
        for kind in [
            SourceServer::Clangd,
            SourceServer::Fortls,
            SourceServer::AsmLsp,
        ] {
            let _ = self.forward_to_passthrough(kind, msg);
        }
        Ok(())
    }

    fn respond_null_if_request(&self, msg: &Value) -> io::Result<()> {
        if msg.get("id").is_some() {
            self.respond(msg.get("id").cloned(), Value::Null)
        } else {
            Ok(())
        }
    }

    fn respond(&self, id: Option<Value>, result: Value) -> io::Result<()> {
        let Some(id) = id else {
            return Ok(());
        };
        self.write_to_client(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    fn write_to_client(&self, msg: &Value) -> io::Result<()> {
        write_lsp_message(&mut *self.out.lock().unwrap(), msg)
    }

    fn register_manifest_file_watcher(&self) -> io::Result<()> {
        self.write_to_client(&json!({
            "jsonrpc": "2.0",
            "id": "__freight_client_watch_freight_toml",
            "method": "client/registerCapability",
            "params": { "registrations": [{ "id": "freight-toml-watch", "method": "workspace/didChangeWatchedFiles", "registerOptions": { "watchers": [{ "globPattern": "**/freight.toml", "kind": 7 }] } }] }
        }))
    }

    /// Spawn a background thread that runs clang-tidy on `uri` and publishes
    /// merged (clangd + tidy) diagnostics once the run completes.
    /// No-op for non-C/C++ files or files without a source-server mapping.
    fn spawn_tidy(&self, uri: &str) {
        if !matches!(source_server_for_uri(uri), Some(SourceServer::Clangd)) {
            return;
        }
        let Some(path) = path_from_uri(uri) else { return };
        let flags: Vec<String> = self.state.indexers.iter()
            .flat_map(|ix| ix.flags_for(&path))
            .collect();
        let uri  = uri.to_string();
        let out  = Arc::clone(&self.out);
        let cache = Arc::clone(&self.state.diag_cache);
        thread::spawn(move || {
            let path_str   = path.to_string_lossy().into_owned();
            let flag_refs: Vec<&str> = flags.iter().map(String::as_str).collect();
            let tidy_diags: Vec<Value> =
                clang_bridge::tidy::run(None, &path_str, None, &flag_refs)
                    .filter(|d| d.file == path_str)
                    .map(|d| indexers::Clang::diag_to_lsp(&d, "clang-tidy"))
                    .collect();
            tracing::debug!(file = %path_str, count = tidy_diags.len(), "clang-tidy done");
            let mut guard = cache.lock().unwrap();
            let entry = guard.entry(uri.clone()).or_default();
            entry.tidy = tidy_diags;
            let merged: Vec<Value> = entry.clangd.iter()
                .chain(entry.tidy.iter())
                .chain(entry.freight.iter())
                .cloned()
                .collect();
            drop(guard);
            let msg = json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": { "uri": uri, "diagnostics": merged }
            });
            let _ = write_lsp_message(&mut *out.lock().unwrap(), &msg);
        });
    }

    /// Compute undeclared-include diagnostics for a C/C++ source file and merge
    /// them into the published set. `text` is the live document contents. No-op
    /// for non-C/C++ files or when the `undeclared-include` lint is `allow`.
    fn compute_include_hygiene(&mut self, uri: &str, text: &str) {
        use crate::build::include_policy as ip;
        if !matches!(source_server_for_uri(uri), Some(SourceServer::Clangd)) {
            return;
        }
        let Some(path) = path_from_uri(uri) else { return };

        let severity = match self.undeclared_include_level() {
            crate::manifest::LintLevel::Allow => {
                self.set_freight_diags(uri, Vec::new());
                return;
            }
            crate::manifest::LintLevel::Warn => 2, // LSP DiagnosticSeverity::Warning
            crate::manifest::LintLevel::Deny => 1, // LSP DiagnosticSeverity::Error
        };

        let file_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let (declared, compiler) = self.declared_dirs_and_compiler(&path);
        let lang = ip::Language::from_path(&path);
        let system = self.cached_system_dirs(compiler.as_deref(), lang);

        let diags: Vec<Value> = ip::check_includes(text, &file_dir, &declared, &system, lang)
            .iter()
            .map(|f| {
                json!({
                    "range": {
                        "start": { "line": f.line, "character": f.start_col },
                        "end":   { "line": f.line, "character": f.end_col }
                    },
                    "severity": severity,
                    "source": "freight",
                    "code": "undeclared-include",
                    "message": format!(
                        "{} is not provided by any declared dependency; add it to \
                         [dependencies] in freight.toml",
                        f.spelling
                    ),
                })
            })
            .collect();

        self.set_freight_diags(uri, diags);
    }

    /// Store freight-generated diagnostics for `uri` and re-publish the merged set.
    fn set_freight_diags(&self, uri: &str, diags: Vec<Value>) {
        let merged: Vec<Value> = {
            let mut guard = self.state.diag_cache.lock().unwrap();
            let entry = guard.entry(uri.to_string()).or_default();
            entry.freight = diags;
            entry.clangd.iter()
                .chain(entry.tidy.iter())
                .chain(entry.freight.iter())
                .cloned()
                .collect()
        };
        let _ = self.publish_diagnostics(uri, merged);
    }

    /// The `[lints].undeclared-include` level for the active project (default warn).
    fn undeclared_include_level(&self) -> crate::manifest::LintLevel {
        self.active_manifest_dir()
            .and_then(|d| crate::manifest::load_manifest(&d).ok())
            .map(|m| m.lints.undeclared_include)
            .unwrap_or_default()
    }

    /// The declared include dirs (`-I`/`-isystem`/`-iquote`) and the compiler for
    /// `path`, read from the generated compile_commands.json.
    fn declared_dirs_and_compiler(&self, path: &Path) -> (Vec<PathBuf>, Option<String>) {
        let Some(dir) = self.state.compile_commands_dir.clone() else {
            return (Vec::new(), None);
        };
        let cmds = crate::build::compile_commands::load(&dir);
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let Some(cmd) = cmds.iter().find(|c| {
            c.file.canonicalize().map(|p| p == target).unwrap_or(c.file == *path)
        }) else {
            return (Vec::new(), None);
        };
        let base = &cmd.directory;
        let args = &cmd.arguments;
        let compiler = args.first().cloned();
        let mut dirs = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            for flag in ["-I", "-isystem", "-iquote"] {
                if let Some(rest) = a.strip_prefix(flag) {
                    let raw = if rest.is_empty() {
                        i += 1;
                        args.get(i).cloned().unwrap_or_default()
                    } else {
                        rest.to_string()
                    };
                    if !raw.is_empty() {
                        let pb = PathBuf::from(&raw);
                        dirs.push(if pb.is_absolute() { pb } else { base.join(pb) });
                    }
                    break;
                }
            }
            i += 1;
        }
        (dirs, compiler)
    }

    /// Probe (and cache) the compiler's built-in include dirs.
    fn cached_system_dirs(
        &mut self,
        compiler: Option<&str>,
        lang: crate::build::include_policy::Language,
    ) -> Vec<PathBuf> {
        if let Some(cached) = &self.state.system_include_dirs {
            return cached.clone();
        }
        let cc = compiler.unwrap_or("c++");
        let dirs = crate::build::include_policy::system_include_dirs(Path::new(cc), lang);
        self.state.system_include_dirs = Some(dirs.clone());
        dirs
    }

    fn publish_manifest_diagnostics(&mut self, uri: &str) -> io::Result<()> {
        let text = self.manifest_text(uri).unwrap_or_default();
        let dir = path_from_uri(uri)
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| self.state.manifest_dir.clone())
            .unwrap_or_else(|| self.state.root_dir.clone());
        let diagnostics = manifest_diagnostics(&text, &dir, &self.state.templates);
        self.publish_diagnostics(uri, diagnostics)
    }

    fn publish_diagnostics(&self, uri: &str, diagnostics: Vec<Value>) -> io::Result<()> {
        self.write_to_client(&json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diagnostics }
        }))
    }

    fn manifest_text(&self, uri: &str) -> Option<String> {
        self.state
            .docs
            .get(uri)
            .cloned()
            .or_else(|| path_from_uri(uri).and_then(|p| std::fs::read_to_string(p).ok()))
    }

    /// The live document text for `uri` (the open editor buffer, kept current on
    /// didOpen/didChange/didSave) falling back to the file on disk. Lets the
    /// `#include`/`import` hints reflect unsaved edits.
    fn doc_text(&self, uri: &str, path: &Path) -> Option<String> {
        self.state
            .docs
            .get(uri)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
    }

    fn refresh_compile_commands(&mut self) {
        let Some(dir) = self.active_manifest_dir() else {
            tracing::info!(
                root_dir = %self.state.root_dir.display(),
                manifest_dir = ?self.state.manifest_dir,
                "no manifest dir — skipping compile commands"
            );
            return;
        };
        if let Ok(dir) = generate_lsp_compile_commands_at(&dir, &self.args.profile) {
            tracing::info!(path = %dir.display(), "compile_commands.json refreshed");
            self.state.compile_commands_dir = Some(dir);
        }
        self.refresh_indexer_flags();
        self.refresh_header_index();
    }

    fn refresh_header_index(&mut self) {
        let base = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        let header_specs = build_header_specs(&base, self.root_is_workspace());
        let pkgs_dir = base.join(".pkgs");
        let pkgs_opt = if pkgs_dir.is_dir() {
            Some(pkgs_dir.as_path())
        } else {
            None
        };
        self.state.header_index = HeaderIndex::build(&header_specs, pkgs_opt);
    }

    fn refresh_workspace_inventory(&mut self) {
        self.state.workspace_inventory = build_workspace_inventory(
            &self
                .active_manifest_dir()
                .unwrap_or_else(|| self.state.root_dir.clone()),
        );
    }

    fn notify_compile_commands_changed(&mut self) -> io::Result<()> {
        let Some(dir) = self.compile_commands_dir() else {
            return Ok(());
        };
        let uri = uri_from_path(&dir.join("compile_commands.json"));
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": [{ "uri": uri, "type": 2 }] }
        });
        self.forward_to_all_passthroughs(&msg)
    }

    fn compile_commands_dir(&self) -> Option<PathBuf> {
        self.state.compile_commands_dir.clone()
    }

    fn root_is_workspace(&self) -> bool {
        load_workspace_manifest(&self.state.root_dir).is_some()
    }

    fn active_manifest_dir(&self) -> Option<PathBuf> {
        if self.root_is_workspace() {
            return Some(self.state.root_dir.clone());
        }
        self.state.manifest_dir.clone()
    }

    fn start_clangd(&mut self, initialize_msg: &Value) -> Option<Value> {
        let dir = self.compile_commands_dir()?;
        let root = self.active_manifest_dir()?;
        let compile_commands_arg = format!("--compile-commands-dir={}", dir.display());
        let pending = Arc::clone(&self.state.clangd_pending);
        let mut clangd_flags = vec![
            compile_commands_arg,
            "--background-index=false".to_string(),
            "--header-insertion=never".to_string(),
        ];
        // Enable clangd's experimental C++20 modules support when this clangd
        // knows the flag (it is recent — clangd 19+). Passing an unknown flag
        // makes clangd exit, so gate on `--help` advertising it.
        if clangd_supports_flag(&self.args.clangd, "--experimental-modules-support") {
            clangd_flags.push("--experimental-modules-support".to_string());
        }
        clangd_flags.extend(self.args.clangd_args.clone());
        let diag_cache = Arc::clone(&self.state.diag_cache);
        let (server, caps) = self.start_passthrough_in(
            "clangd",
            &self.args.clangd,
            &clangd_flags,
            INTERNAL_CLANGD_INIT_ID,
            initialize_msg,
            Some(&root),
            Some(pending),
            Some(diag_cache),
        )?;
        self.state.clangd = Some(server);
        caps
    }

    fn start_fortls(&mut self, initialize_msg: &Value) -> Option<Value> {
        // Tell fortls where the sources live so it can resolve modules and
        // includes. We always include `src/` plus the project root.
        let root = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        let src_dir = root.join("src");
        let mut args = vec![
            "--source_dirs".to_string(),
            if src_dir.is_dir() {
                format!("{}", src_dir.display())
            } else {
                format!("{}", root.display())
            },
            "--incremental_sync".to_string(),
            "--notify_init".to_string(),
        ];
        // Suppress fortls's banner chatter in the output channel.
        args.push("--silent".to_string());
        let (server, caps) = self.start_passthrough_in(
            "fortls",
            &self.args.fortls,
            &args,
            INTERNAL_FORTLS_INIT_ID,
            initialize_msg,
            Some(&root),
            None,
            None,
        )?;
        self.state.fortls = Some(server);
        caps
    }

    fn start_asm_lsp(&mut self, initialize_msg: &Value) -> Option<Value> {
        let root = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        let (server, caps) = self.start_passthrough_in(
            "asm-lsp",
            &self.args.asm_lsp,
            &[],
            INTERNAL_ASM_LSP_INIT_ID,
            initialize_msg,
            Some(&root),
            None,
            None,
        )?;
        self.state.asm_lsp = Some(server);
        caps
    }

    fn start_passthrough_in(
        &self,
        _name: &str,
        command: &str,
        args: &[String],
        init_id: &str,
        initialize_msg: &Value,
        cwd: Option<&Path>,
        pending: Option<Arc<Mutex<HashMap<String, PendingClangdRequest>>>>,
        diag_cache: Option<Arc<Mutex<HashMap<String, DiagCache>>>>,
    ) -> Option<(Passthrough, Option<Value>)> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().ok()?;
        let mut child_stdin = child.stdin.take()?;
        let child_stdout = child.stdout.take()?;
        let mut init = initialize_msg.clone();
        if let Some(obj) = init.as_object_mut() {
            obj.insert("id".to_string(), json!(init_id));
        }
        write_lsp_message(&mut child_stdin, &init).ok()?;
        let mut reader = BufReader::new(child_stdout);
        let init_response = loop {
            let msg = read_lsp_message(&mut reader).ok()??;
            if msg.get("id").and_then(Value::as_str) == Some(init_id) {
                break msg;
            }
        };
        let caps = init_response
            .get("result")
            .and_then(|r| r.get("capabilities"))
            .cloned();
        let out = Arc::clone(&self.out);
        thread::spawn(move || {
            while let Ok(Some(msg)) = read_lsp_message(&mut reader) {
                if let Some(id_str) = msg.get("id").and_then(Value::as_str) {
                    if let Some(ref p) = pending {
                        let intercepted = p.lock().unwrap().remove(id_str);
                        match intercepted {
                            Some(PendingClangdRequest::InlayHint {
                                original_id,
                                freight_hints,
                            }) => {
                                let response =
                                    merge_clangd_inlay_response(msg, original_id, freight_hints);
                                let _ = write_lsp_message(&mut *out.lock().unwrap(), &response);
                                continue;
                            }
                            None => {}
                        }
                    }
                }
                // Intercept publishDiagnostics from this passthrough so we can
                // merge its results with clang-tidy before forwarding to the client.
                if msg.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                {
                    if let Some(ref cache) = diag_cache {
                        let uri = msg["params"]["uri"]
                            .as_str().unwrap_or("").to_string();
                        let new_diags = msg["params"]["diagnostics"]
                            .as_array().cloned().unwrap_or_default();
                        let mut guard = cache.lock().unwrap();
                        let entry = guard.entry(uri.clone()).or_default();
                        entry.clangd = new_diags;
                        let merged: Vec<Value> = entry.clangd.iter()
                            .chain(entry.tidy.iter())
                            .chain(entry.freight.iter())
                            .cloned()
                            .collect();
                        drop(guard);
                        let merged_msg = json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": { "uri": uri, "diagnostics": merged }
                        });
                        let _ = write_lsp_message(&mut *out.lock().unwrap(), &merged_msg);
                        continue;
                    }
                }
                if is_internal_passthrough_response(&msg) {
                    continue;
                }
                let _ = write_lsp_message(&mut *out.lock().unwrap(), &msg);
            }
        });
        Some((
            Passthrough {
                child,
                stdin: Arc::new(Mutex::new(child_stdin)),
            },
            caps,
        ))
    }

    fn shutdown_passthroughs(&mut self) {
        let shutdowns = [
            (SourceServer::Clangd, "__freight_clangd_shutdown"),
            (SourceServer::Fortls, "__freight_fortls_shutdown"),
            (SourceServer::AsmLsp, "__freight_asm_lsp_shutdown"),
        ];
        for (kind, id) in shutdowns {
            let msg = json!({"jsonrpc": "2.0", "id": id, "method": "shutdown"});
            let _ = self.forward_to_passthrough(kind, &msg);
        }
    }

    fn kill_passthroughs(&mut self) {
        if let Some(mut s) = self.state.clangd.take() {
            let _ = s.child.kill();
        }
        if let Some(mut s) = self.state.fortls.take() {
            let _ = s.child.kill();
        }
        if let Some(mut s) = self.state.asm_lsp.take() {
            let _ = s.child.kill();
        }
    }
}

fn build_workspace_inventory(root: &Path) -> WorkspaceInventory {
    let mut packages = Vec::new();
    if let Some(workspace) = load_workspace_manifest(root) {
        for member in workspace.members {
            let member_path = member.trim_end_matches('/').to_string();
            let member_dir = root.join(&member_path);
            if let Some(package) = package_inventory(&member_dir, member_path) {
                packages.push(package);
            }
        }
    } else if let Some(package) = package_inventory(root, ".".to_string()) {
        packages.push(package);
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    WorkspaceInventory { packages }
}

fn package_inventory(dir: &Path, path: String) -> Option<WorkspacePackage> {
    let manifest = load_manifest(dir).ok()?;
    let mut bins: Vec<String> = manifest.bins.iter().map(|bin| bin.name.clone()).collect();
    bins.sort();
    let lib = manifest
        .lib
        .as_ref()
        .map(|lib| format!("{:?}", lib.lib_type));
    Some(WorkspacePackage {
        name: manifest.package.name,
        path,
        bins,
        lib,
    })
}

/// Build `HeaderDirSpec` entries from the freight manifest graph rooted at `base`.
///
/// - `base` itself (or workspace root) → `Own`
/// - Path dependencies → `PathDep` with their dep key
/// - Workspace members → `Workspace`
fn build_header_specs(base: &Path, is_workspace: bool) -> Vec<HeaderDirSpec<'_>> {
    let mut specs: Vec<(PathBuf, HeaderOrigin, Option<String>)> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    let push = |specs: &mut Vec<(PathBuf, HeaderOrigin, Option<String>)>,
                seen: &mut std::collections::HashSet<PathBuf>,
                dir: PathBuf,
                origin: HeaderOrigin,
                dep_key: Option<String>| {
        if !dir.is_dir() {
            return;
        }
        let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if seen.insert(canon) {
            specs.push((dir, origin, dep_key));
        }
    };

    if is_workspace {
        // Workspace root is not a package; add each member as Own and collect their path deps.
        if let Some(ws) = load_workspace_manifest(base) {
            for member_path in &ws.members {
                let member_dir = base.join(member_path);
                push(
                    &mut specs,
                    &mut seen,
                    member_dir.clone(),
                    HeaderOrigin::Own,
                    None,
                );
                // Path deps of this workspace member
                if let Ok(manifest) = load_manifest(&member_dir) {
                    collect_path_dep_specs(&member_dir, &manifest, &mut specs, &mut seen);
                }
            }
        }
    } else {
        push(
            &mut specs,
            &mut seen,
            base.to_path_buf(),
            HeaderOrigin::Own,
            None,
        );
        if let Ok(manifest) = load_manifest(base) {
            collect_path_dep_specs(base, &manifest, &mut specs, &mut seen);
        }
    }

    // We need owned PathBufs to survive the borrow. Allocate and leak refs.
    // (HeaderDirSpec<'_> holds &'_ Path — we need a stable backing store.)
    // Use a boxed slice to pin the paths.
    specs
        .into_iter()
        .map(|(dir, origin, dep_key)| {
            // Safety: HeaderDirSpec borrows a path. We need the path to outlive
            // the Vec. Box it and leak — this is an LSP process, memory is fine.
            let boxed: Box<Path> = dir.into_boxed_path();
            let path: &'static Path = Box::leak(boxed);
            HeaderDirSpec {
                path,
                origin,
                dep_key,
            }
        })
        .collect()
}

fn collect_path_dep_specs(
    project_dir: &Path,
    manifest: &Manifest,
    specs: &mut Vec<(PathBuf, HeaderOrigin, Option<String>)>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    for (dep_key, dep) in manifest.effective_dependencies().into_iter().chain(
        manifest
            .dev_dependencies
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    ) {
        let Dependency::Detailed(detail) = dep else {
            continue;
        };
        let Some(rel_path) = detail.path else {
            continue;
        };
        let dep_dir = project_dir.join(&rel_path);
        if !dep_dir.is_dir() {
            continue;
        }
        let canon = dep_dir.canonicalize().unwrap_or_else(|_| dep_dir.clone());
        if seen.insert(canon) {
            specs.push((dep_dir, HeaderOrigin::PathDep, Some(dep_key)));
        }
    }
}

fn level_for_method(method: &str) -> tracing::Level {
    match method {
        "textDocument/hover"
        | "textDocument/didOpen"
        | "textDocument/didSave"
        | "initialize"
        | "initialized"
        | "shutdown" => tracing::Level::INFO,
        _ => tracing::Level::DEBUG,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::protocol::sanitize_code_action_diagnostics;
    use super::build_workspace_inventory;
    use serde_json::json;

    #[test]
    fn code_action_diagnostic_codes_are_sanitized_to_strings() {
        let msg = json!({
            "jsonrpc": "2.0", "id": 1, "method": "textDocument/codeAction",
            "params": { "context": { "diagnostics": [
                { "code": 123, "message": "numeric" },
                { "code": { "value": "x", "target": "https://example.test" }, "message": "object" },
                { "code": "already-string", "message": "string" }
            ]}}
        });
        let sanitized = sanitize_code_action_diagnostics(&msg);
        let diagnostics = sanitized["params"]["context"]["diagnostics"]
            .as_array()
            .unwrap();
        assert_eq!(diagnostics[0]["code"], json!("123"));
        assert!(diagnostics[1]["code"].is_string());
        assert_eq!(diagnostics[2]["code"], json!("already-string"));
    }

    #[test]
    fn workspace_inventory_collects_member_libs_and_bins() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("freight.toml"),
            r#"
[workspace]
members = ["core", "app"]
"#,
        )
        .unwrap();
        write_manifest(
            &tmp.path().join("core"),
            r#"
[package]
name = "core"
version = "0.1.0"

[language.c]
std = "c17"

[lib]
type = "static"
srcs = ["src/core.c"]
"#,
        );
        write_manifest(
            &tmp.path().join("app"),
            r#"
[package]
name = "app"
version = "0.1.0"

[language.c]
std = "c17"

[[bin]]
name = "demo"
src = "src/main.c"
"#,
        );

        let inventory = build_workspace_inventory(tmp.path());

        assert_eq!(inventory.packages.len(), 2);
        let app = inventory
            .packages
            .iter()
            .find(|pkg| pkg.name == "app")
            .unwrap();
        assert_eq!(app.bins, vec!["demo"]);
        let core = inventory
            .packages
            .iter()
            .find(|pkg| pkg.name == "core")
            .unwrap();
        assert_eq!(core.lib.as_deref(), Some("Static"));
        assert_eq!(core.path, "core");
    }

    fn write_manifest(dir: &std::path::Path, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("freight.toml"), text).unwrap();
    }
}
