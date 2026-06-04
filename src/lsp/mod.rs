//! `freight lsp` — Language Server Protocol multiplexer for freight.toml and
//! source files (clangd, fortls, asm-lsp passthroughs).

mod doc_index;
pub mod log;
mod manifest;
mod protocol;

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::build::generate_lsp_compile_commands_at;
use crate::manifest::types::{Dependency, Manifest};
use crate::manifest::{find_manifest_dir, load_manifest, load_workspace_manifest};
use crate::toolchain::{detect_all_cached, load_all_templates};
use serde_json::{json, Value};

use doc_index::{
    include_hover_markdown, item_to_markdown, parse_include_header, word_at,
    DocIndex, HeaderDirSpec, HeaderIndex, HeaderOrigin,
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
    /// Accepted for compatibility with LSP clients that append --stdio.
    #[arg(long, hide = true)]
    pub stdio: bool,
}

impl Args {
    pub fn run(self) {
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
// Types
// ---------------------------------------------------------------------------

struct Server {
    args: Args,
    out: Arc<Mutex<io::Stdout>>,
    state: ServerState,
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
    /// Symbol documentation index — shared with passthrough read threads.
    doc_index: Arc<Mutex<Option<DocIndex>>>,
    /// Header → package mapping for `#include` hover.
    header_index: HeaderIndex,
    workspace_inventory: WorkspaceInventory,
}

struct Passthrough {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
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
                doc_index: Arc::new(Mutex::new(None)),
                header_index: HeaderIndex::default(),
                workspace_inventory: WorkspaceInventory::default(),
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
                tracing::Level::INFO  => tracing::info!(method, "← client"),
                _                     => tracing::debug!(method, "← client"),
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
                "freight/workspaceInfo" => self.handle_workspace_info(msg)?,
                "freight/setConfig"     => self.handle_set_config(msg)?,
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
        let capabilities = merged_capabilities(source_caps);
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
        }
        self.forward_by_text_document(&msg)
    }

    fn handle_did_change(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_to_all_passthroughs(&msg);
        };
        if !is_freight_manifest_uri(&uri) {
            return self.forward_by_uri(&uri, &msg);
        }
        if let Some(text) = changed_full_text(&msg) {
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
            return self.forward_by_uri(&uri, &msg);
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
        if !is_freight_manifest_uri(&uri) {
            return self.forward_or_null(msg);
        }
        let text = self.manifest_text(&uri);
        let result = completion_result(
            text.as_deref(),
            position(&msg),
            Some(&self.state.workspace_inventory),
        );
        self.respond(msg.get("id").cloned(), result)
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

        if let Some((line, col)) = position(&msg) {
            let file = path_from_uri(&uri)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| uri.to_string());
            tracing::debug!(file, line, col, "hover request");
        }

        // 1. #include / #import hover — show package origin.
        if let Some(hover) = self.include_hover(&uri, &msg) {
            return self.respond(msg.get("id").cloned(), hover);
        }

        // 2. DocIndex — position-based then name-based lookup.
        if let Some(hover) = self.doc_hover(&uri, &msg) {
            return self.respond(msg.get("id").cloned(), hover);
        }

        // 3. For non-C/C++ files (Fortran, assembly, …) fall back to the
        //    language-specific passthrough server on a DocIndex miss.
        //    C/C++ hover is DocIndex-only; clangd is kept for diagnostics only.
        match source_server_for_uri(&uri) {
            Some(SourceServer::Fortls) | Some(SourceServer::AsmLsp) => {
                self.forward_by_uri(&uri, &msg)
            }
            _ => self.respond(msg.get("id").cloned(), Value::Null),
        }
    }

    fn include_hover(&self, uri: &str, msg: &Value) -> Option<Value> {
        if self.state.header_index.is_empty() {
            return None;
        }
        let (line, _) = position(msg)?;
        let path = path_from_uri(uri)?;
        let text = std::fs::read_to_string(&path).ok()?;
        let line_text = text.lines().nth(line)?;
        let header = parse_include_header(line_text)?;
        let entry = self.state.header_index.lookup(&header)?;
        let md = include_hover_markdown(&header, entry);
        tracing::debug!(header, package = entry.package_name.as_str(), line, "include hover");
        Some(json!({ "contents": { "kind": "markdown", "value": md } }))
    }

    fn doc_hover(&self, uri: &str, msg: &Value) -> Option<Value> {
        let guard = self.state.doc_index.lock().unwrap();
        let index = match guard.as_ref() {
            Some(idx) if !idx.is_empty() => idx,
            Some(_) => {
                tracing::info!("doc-index hover: index is empty — no items indexed");
                return None;
            }
            None => {
                tracing::info!("doc-index hover: index not built yet");
                return None;
            }
        };

        let Some((line, character)) = position(msg) else {
            tracing::debug!("doc-index hover: no position in message");
            return None;
        };
        let Some(path) = path_from_uri(uri) else {
            tracing::debug!(uri, "doc-index hover: could not resolve path from uri");
            return None;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "doc-index hover: could not read file");
                return None;
            }
        };

        let word = word_at(&text, line, character);
        tracing::debug!(
            word = word.as_deref().unwrap_or("(none)"),
            line,
            col = character,
            index_items = index.len(),
            "doc-index hover lookup"
        );

        // Position-based: find the item whose doc comment is nearest before the cursor.
        // Validate that the word under cursor matches the item's simple name so we don't
        // return the wrong item when the cursor is inside a long function body.
        let item = index
            .lookup_by_location(&path, line)
            .filter(|item| {
                word.as_deref()
                    .map(|w| item.name.to_ascii_lowercase().ends_with(&w.to_ascii_lowercase()))
                    .unwrap_or(true)
            })
            .or_else(|| {
                let w = word.as_deref()?;
                let found = index.lookup(w);
                if found.is_none() {
                    tracing::debug!(word = w, "doc-index hover: name lookup miss");
                }
                found
            });

        match item {
            Some(item) => {
                tracing::info!(
                    symbol = item.name.as_str(),
                    source_file = %item.file.display(),
                    cursor_line = line,
                    cursor_col = character,
                    "doc-index hover hit"
                );
                let markdown = item_to_markdown(item);
                Some(json!({ "contents": { "kind": "markdown", "value": markdown } }))
            }
            None => {
                tracing::info!(
                    word = word.as_deref().unwrap_or("(none)"),
                    line,
                    col = character,
                    "doc-index hover: no match"
                );
                None
            }
        }
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
        let manifest_dir = self.active_manifest_dir().unwrap_or_else(|| self.state.root_dir.clone());
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
        let key   = params.get("key").and_then(Value::as_str).unwrap_or("");
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

    fn refresh_compile_commands(&mut self) {
        let Some(dir) = self.active_manifest_dir() else {
            tracing::info!(
                root_dir = %self.state.root_dir.display(),
                manifest_dir = ?self.state.manifest_dir,
                "no manifest dir — skipping compile commands and doc index"
            );
            return;
        };
        if let Ok(dir) = generate_lsp_compile_commands_at(&dir, &self.args.profile) {
            tracing::info!(path = %dir.display(), "compile_commands.json refreshed");
            self.state.compile_commands_dir = Some(dir);
        }
        self.refresh_doc_index();
    }

    fn refresh_workspace_inventory(&mut self) {
        self.state.workspace_inventory = build_workspace_inventory(
            &self
                .active_manifest_dir()
                .unwrap_or_else(|| self.state.root_dir.clone()),
        );
    }

    fn refresh_doc_index(&mut self) {
        let base = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        let package_dirs = self.doc_index_package_dirs(&base);
        let package_refs: Vec<&Path> = package_dirs.iter().map(PathBuf::as_path).collect();

        // DocIndex — built synchronously; typically well under a second.
        let new_index = DocIndex::build_freight_packages(package_refs.iter().copied());
        tracing::info!(
            packages = package_dirs.len(),
            items = new_index.len(),
            "doc index rebuilt"
        );
        for dir in &package_dirs {
            tracing::debug!(path = %dir.display(), "doc index package dir");
        }
        *self.state.doc_index.lock().unwrap() = Some(new_index);

        // HeaderIndex — tag each dir with its origin.
        let is_workspace = self.root_is_workspace();
        let header_specs: Vec<HeaderDirSpec<'_>> = package_dirs
            .iter()
            .map(|dir| {
                // Workspace members live directly under base; path deps are elsewhere.
                let origin = if is_workspace && dir.parent() == Some(base.as_path()) {
                    HeaderOrigin::Workspace
                } else {
                    HeaderOrigin::Project
                };
                HeaderDirSpec { path: dir.as_path(), origin }
            })
            .collect();

        let pkgs_dir = base.join(".pkgs");
        let pkgs_opt = if pkgs_dir.is_dir() { Some(pkgs_dir.as_path()) } else { None };
        self.state.header_index = HeaderIndex::build(&header_specs, pkgs_opt);
    }

    fn doc_index_package_dirs(&self, base: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if self.root_is_workspace() {
            for pkg in &self.state.workspace_inventory.packages {
                let member_dir = base.join(&pkg.path);
                push_doc_package_dir(&mut dirs, member_dir.clone());
                collect_path_dependency_doc_dirs(&member_dir, &mut dirs);
            }
            if !dirs.is_empty() {
                return dirs;
            }
        }
        push_doc_package_dir(&mut dirs, base.to_path_buf());
        collect_path_dependency_doc_dirs(base, &mut dirs);
        dirs
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
        let (server, caps) = self.start_passthrough_in(
            "clangd",
            &self.args.clangd,
            &[
                compile_commands_arg,
                "--background-index=false".to_string(),
                "--header-insertion=never".to_string(),
            ],
            INTERNAL_CLANGD_INIT_ID,
            initialize_msg,
            Some(&root),
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

fn collect_path_dependency_doc_dirs(project_dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(manifest) = load_manifest(project_dir) else {
        return;
    };
    collect_manifest_path_dependency_doc_dirs(project_dir, &manifest, out);
}

fn collect_manifest_path_dependency_doc_dirs(
    project_dir: &Path,
    manifest: &Manifest,
    out: &mut Vec<PathBuf>,
) {
    for dep in manifest
        .effective_dependencies()
        .into_values()
        .chain(manifest.dev_dependencies.values().cloned())
    {
        let Dependency::Detailed(detail) = dep else {
            continue;
        };
        let Some(path) = detail.path else {
            continue;
        };
        push_doc_package_dir(out, project_dir.join(path));
    }
}

fn push_doc_package_dir(out: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dir.is_dir() {
        return;
    }
    let canonical = dir.canonicalize().unwrap_or(dir);
    if !out
        .iter()
        .any(|existing| existing.canonicalize().unwrap_or_else(|_| existing.clone()) == canonical)
    {
        out.push(canonical);
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
    use super::{build_workspace_inventory, collect_path_dependency_doc_dirs};
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

    #[test]
    fn doc_index_dirs_include_explicit_path_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"
[package]
name = "app"
version = "0.1.0"

[language.c]
std = "c17"

[[bin]]
name = "app"
src = "src/main.c"

[dependencies]
core = { path = "core" }
"#,
        );
        write_manifest(
            &tmp.path().join("core"),
            r#"
[package]
name = "core"
version = "0.1.0"

[language.c]
std = "c17"

[lib]
type = "header"
hdrs = ["include/core.h"]
"#,
        );

        let mut dirs = Vec::new();
        collect_path_dependency_doc_dirs(tmp.path(), &mut dirs);

        assert_eq!(dirs, vec![tmp.path().join("core").canonicalize().unwrap()]);
    }

    fn write_manifest(dir: &std::path::Path, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("freight.toml"), text).unwrap();
    }
}
