//! `freight lsp` — Language Server Protocol multiplexer for freight.toml and
//! source files (clangd, fortls, asm-lsp passthroughs).

pub mod doxygen;
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

use index::LanguageIndexer;
use indexers::{AsmIndexer, ClangIndexer, FortranIndexer};

use crate::build::generate_lsp_compile_commands_at;
use crate::manifest::{find_manifest_dir, load_manifest_cached, load_workspace_manifest};
use crate::toolchain::{detect_all_cached, load_all_templates};
use serde_json::{json, Value};

use index::{
    include_completion, include_hint_line, include_inlay_label, is_std_module, module_hint_line,
    module_inlay_label_for, parse_include_header, HeaderDirSpec, HeaderEntry, HeaderIndex,
    HeaderOrigin, ModuleIndex,
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
    /// Disable the native assembly indexer and fall back to the external
    /// `asm-lsp` passthrough.
    #[arg(long)]
    pub no_native_asm: bool,
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
            // The client closing the connection (it exited, restarted, or the
            // editor window reconnected) surfaces as a broken pipe on the next
            // stdout write, or EOF on stdin. That is a normal shutdown, not an
            // error — exit quietly so the editor doesn't show a spurious
            // "freight lsp: Broken pipe" in its Output panel.
            if !matches!(
                e.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
            ) {
                eprintln!("freight lsp: {e}");
            }
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

fn native_fortran_enabled() -> bool {
    true
}

/// Whether the native assembly indexer serves `.s`/`.asm`/`.nasm` files. When
/// true the external `asm-lsp` passthrough is not started. Disabled with
/// `--no-native-asm` to fall back to the passthrough.
fn native_asm_enabled(args: &Args) -> bool {
    !args.no_native_asm
}

/// Build an end-of-line inlay hint (dimmed text + markdown tooltip).
fn inlay_hint_json(line: usize, col: usize, label: &str, tooltip_md: &str) -> Value {
    json!({
        "position": { "line": line, "character": col },
        "label": label,
        "kind": 2,            // Parameter kind — renders as dimmed text
        "paddingLeft": true,
        "tooltip": { "kind": "markdown", "value": tooltip_md }
    })
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
    clangd: Vec<Value>,
    tidy: Vec<Value>,
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
    /// C++20 module → declaring package mapping for `import …;` hints.
    module_index: ModuleIndex,
    /// Owned active-project model, recomputed by [`Server::refresh_project_model`]
    /// whenever the manifest set changes. The active package's parsed manifest
    /// (lint levels, declared deps) so per-keystroke handlers read it instead of
    /// reloading on each request. `None` until a project manifest is located, or
    /// when the active root is a workspace (no `[package]`).
    active_manifest: Option<crate::manifest::Manifest>,
    /// Owned package layout (project + workspace members + path deps) for the
    /// active project. The single derivation behind the header/module index
    /// refresh; recomputed only when the manifest set changes.
    package_dirs: Vec<(PathBuf, crate::build::PackageKind, Option<String>)>,
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

    /// Cached compiler built-in include dirs (used by the include-hygiene check to
    /// confirm an undeclared header exists), tagged with the sysroot they were
    /// probed against so a cross-target change re-probes.
    system_include_dirs: Option<(Option<String>, Vec<PathBuf>)>,

    /// Per-URI undeclared `#include`/`import` findings (0-based line → spelling),
    /// recomputed with the diagnostics and reused for the inlay-hint markers.
    undeclared_includes: HashMap<String, Vec<(u32, String)>>,

    /// Last parsed `#include`/`import` directives per URI. The include-hygiene
    /// check is skipped when these are unchanged (the common case while editing
    /// code that isn't an include line), so most keystrokes do no disk work.
    last_includes: HashMap<String, Vec<crate::build::include_policy::IncludeDirective>>,

    /// Cached declared include dirs + compiler per source file (from
    /// compile_commands.json). Avoids re-parsing it on every include edit;
    /// invalidated when compile commands are regenerated.
    declared_dirs_cache: HashMap<PathBuf, (Vec<PathBuf>, Option<String>)>,
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
    CodeAction {
        original_id: Value,
        freight_actions: Vec<Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceServer {
    Clangd,
    Fortls,
    AsmLsp,
}

/// A `link-feature-hint`: an `<…>` system-library header whose `[os.*] features`
/// entry is missing. Produces a Hint diagnostic + a "add to freight.toml" fix.
struct LinkFeatureHint {
    line: u32,
    start_col: u32,
    end_col: u32,
    header: String,
    feature: String,
    os: String,
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
        let mut indexers: Vec<Box<dyn LanguageIndexer>> = vec![Box::new(FortranIndexer::new())];
        if native_asm_enabled(&args) {
            indexers.push(Box::new(AsmIndexer::new()));
        }
        if args.use_clang_bridge {
            indexers.push(Box::new(ClangIndexer::new()));
        }
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
                module_index: ModuleIndex::default(),
                active_manifest: None,
                package_dirs: Vec::new(),
                workspace_inventory: WorkspaceInventory::default(),
                clangd_pending: Arc::new(Mutex::new(HashMap::new())),
                diag_cache: Arc::new(Mutex::new(HashMap::new())),
                indexers,
                system_include_dirs: None,
                undeclared_includes: HashMap::new(),
                last_includes: HashMap::new(),
                declared_dirs_cache: HashMap::new(),
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
                "completionItem/resolve" => self.handle_completion_resolve(msg)?,
                "textDocument/hover" => self.handle_hover_or_forward(msg)?,
                "textDocument/signatureHelp" => self.handle_signature_help_or_forward(msg)?,
                "textDocument/codeAction" => self.handle_code_action_or_forward(msg)?,
                "textDocument/rename" => self.handle_rename_or_forward(msg)?,
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
        if !native_fortran_enabled() && !self.args.no_fortls {
            if let Some(caps) = self.start_fortls(&msg) {
                source_caps.push(caps);
            }
        }
        if !native_asm_enabled(&self.args) && !self.args.no_asm_lsp {
            if let Some(caps) = self.start_asm_lsp(&msg) {
                source_caps.push(caps);
            }
        }
        let capabilities = merged_capabilities(source_caps, self.args.use_clang_bridge, true);
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
            for ix in &mut self.state.indexers {
                ix.reparse(&uri, &text);
            }
            self.state.docs.insert(uri.clone(), text);
            self.publish_indexer_diagnostics(&uri)?;
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
                for ix in &mut self.state.indexers {
                    ix.reparse(&uri, &text);
                }
                self.compute_include_hygiene(&uri, &text);
                self.state.docs.insert(uri.clone(), text);
                self.publish_indexer_diagnostics(&uri)?;
            }
            return self.forward_by_uri(&uri, &msg);
        }
        if let Some(text) = changed_full_text(&msg) {
            for ix in &mut self.state.indexers {
                ix.reparse(&uri, &text);
            }
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
                for ix in &mut self.state.indexers {
                    ix.reparse(&uri, &text);
                }
                self.compute_include_hygiene(&uri, &text);
                self.state.docs.insert(uri.clone(), text);
                self.publish_indexer_diagnostics(&uri)?;
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
        self.state.undeclared_includes.remove(&uri);
        self.state.last_includes.remove(&uri);
        if let Some(path) = path_from_uri(&uri) {
            for ix in &mut self.state.indexers {
                ix.evict(&path);
            }
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
        // Completion inside an `#include` / `import` directive is answered by
        // freight, not clangd: only stdlib headers and declared-package headers
        // are offered, each labelled with the library it comes from.
        if matches!(source_server_for_uri(&uri), Some(SourceServer::Clangd)) {
            if let Some(result) = self.include_completion_result(&uri, &msg) {
                return self.respond(msg.get("id").cloned(), result);
            }
        }
        if let Some(result) = self
            .state
            .indexers
            .iter_mut()
            .find_map(|ix| ix.completion(&uri, &msg))
        {
            return self.respond(msg.get("id").cloned(), result);
        }
        self.forward_or_null(msg)
    }

    /// `completionItem/resolve` — for freight's own `#include`/`import` items,
    /// render the target file's Doxygen banner into the documentation panel.
    /// Other items (clangd's lazily-resolved completions) are forwarded.
    fn handle_completion_resolve(&mut self, msg: Value) -> io::Result<()> {
        let item = msg.get("params").cloned().unwrap_or(Value::Null);
        let is_freight = item
            .get("data")
            .and_then(|d| d.get("freightInclude"))
            .and_then(Value::as_bool)
            == Some(true);
        if is_freight {
            let resolved = self.resolve_include_completion_item(item);
            return self.respond(msg.get("id").cloned(), resolved);
        }
        // Not ours — let clangd fill in its item's docs/edits. If clangd isn't
        // running, echo the item back unchanged.
        if self.forward_to_passthrough(SourceServer::Clangd, &msg)? {
            return Ok(());
        }
        self.respond(msg.get("id").cloned(), item)
    }

    /// Add the resolved header/module file's Doxygen banner as the item's
    /// `documentation`. Reads the file lazily (only when the user scrolls to it).
    fn resolve_include_completion_item(&self, mut item: Value) -> Value {
        let data = item.get("data").cloned().unwrap_or(Value::Null);
        let path: Option<std::path::PathBuf> = data
            .get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                data.get("header")
                    .and_then(Value::as_str)
                    .and_then(|h| self.state.header_index.lookup_system(h))
                    .map(|e| e.full_path)
            });
        if let Some(p) = path {
            if let Some(md) = doxygen::file_doc_markdown(&p) {
                if let Some(obj) = item.as_object_mut() {
                    obj.insert(
                        "documentation".to_string(),
                        json!({ "kind": "markdown", "value": md }),
                    );
                }
            }
        }
        item
    }

    /// Freight-owned completion for `#include` / `import` directives. `None`
    /// when the cursor isn't inside one (the request is forwarded as usual).
    fn include_completion_result(&self, uri: &str, msg: &Value) -> Option<Value> {
        let (line_no, col) = position(msg)?;
        let path = path_from_uri(uri)?;
        let text = self.doc_text(uri, &path)?;
        let line = text.lines().nth(line_no)?;
        let lang = crate::build::include_policy::Language::from_path(&path);
        include_completion(
            line,
            line_no,
            col,
            lang,
            &self.state.header_index,
            &self.state.module_index,
        )
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

        // Hover on an `#include` / `import` directive is answered by freight, not
        // clangd: we show the owning package / stdlib / undeclared status rather
        // than clangd's resolved-path hint. This must run before the indexer and
        // the clangd forward so freight's hover always wins for these lines.
        if let Some(result) = self.include_hover_result(&uri, &msg) {
            return self.respond(msg.get("id").cloned(), result);
        }

        if let Some(result) = self
            .state
            .indexers
            .iter_mut()
            .find_map(|ix| ix.hover(&uri, &msg))
        {
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

    /// Freight-owned hover for an `#include` / `import` directive line. Returns
    /// `None` when the cursor isn't on such a line (so hover falls through to the
    /// language indexer / clangd). Scoped to C-family files, where clangd would
    /// otherwise answer with its own (path-only) hover.
    fn include_hover_result(&self, uri: &str, msg: &Value) -> Option<Value> {
        if !matches!(source_server_for_uri(uri), Some(SourceServer::Clangd)) {
            return None;
        }
        let (line_no, _col) = position(msg)?;
        let path = path_from_uri(uri)?;
        let text = self.doc_text(uri, &path)?;
        let line_text = text.lines().nth(line_no)?;
        let (header, is_system, is_module) = parse_include_header(line_text)?;

        // Undeclared cases keep their actionable warning. Everything else uses
        // the compact hint: **pkg@version**/header, then brief, then author.
        let (title, banner_path): (String, Option<std::path::PathBuf>) = if is_module {
            let entry = self.state.module_index.lookup(&header);
            if entry.is_none()
                && !is_std_module(&header)
                && line_text.contains(';')
                && !matches!(
                    self.undeclared_include_level(),
                    crate::manifest::LintLevel::Allow
                )
            {
                let md = format!(
                    "**⚠ undeclared module** `{header}`\n\nNot provided by any declared \
                     dependency. Add the package that exports it to `[dependencies]` in \
                     `freight.toml`."
                );
                return Some(json!({ "contents": { "kind": "markdown", "value": md } }));
            }
            (
                module_hint_line(&header, entry),
                entry.map(|e| e.interface_path.clone()),
            )
        } else if let Some(spelling) = self
            .state
            .undeclared_includes
            .get(uri)
            .and_then(|v| v.iter().find(|(l, _)| *l as usize == line_no))
            .map(|(_, s)| s.clone())
        {
            let md = format!(
                "**⚠ undeclared include** `{spelling}`\n\nNot provided by any declared \
                 dependency. Add the dependency that provides it to `[dependencies]` in \
                 `freight.toml`."
            );
            return Some(json!({ "contents": { "kind": "markdown", "value": md } }));
        } else if let Some(entry) = self.state.header_index.lookup(&header) {
            (
                include_hint_line(&header, entry),
                Some(entry.full_path.clone()),
            )
        } else if is_system {
            // System-library header (pthread.h, …): report the feature it belongs
            // to, not a stdlib/file label.
            let stubs = crate::toolchain::system_libs::load_system_lib_stubs();
            if let Some(stub) = crate::toolchain::system_libs::find_stub_by_header(&header, &stubs) {
                let os = stub.section_os();
                let status = if self.declared_system_features().contains(&stub.name) {
                    format!("Linked via `[os.{os}] features = [\"{}\"]`.", stub.name)
                } else {
                    format!(
                        "Not linked — add `{}` to `[os.{os}] features` in `freight.toml`.",
                        stub.name
                    )
                };
                let md = format!(
                    "**`{}` system library** · `{header}`\n\n{status}",
                    stub.name
                );
                return Some(json!({ "contents": { "kind": "markdown", "value": md } }));
            }
            // File-based system lookup (e.g. <vector> → the libstdc++ path), or a
            // synthetic stdlib entry so a recognised header still reads as stdlib.
            let entry = self
                .state
                .header_index
                .lookup_system(&header)
                .unwrap_or(HeaderEntry {
                    package_name: "stdlib".to_string(),
                    package_version: None,
                    full_path: std::path::PathBuf::new(),
                    origin: HeaderOrigin::System,
                    dep_key: None,
                    pkg_dir: None,
                });
            let path = (!entry.full_path.as_os_str().is_empty()).then(|| entry.full_path.clone());
            (include_hint_line(&header, &entry), path)
        } else {
            // An unknown quoted header — let clangd answer (it may resolve it).
            return None;
        };

        // Quote include next to the source file (not in the index).
        let banner_path = banner_path
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| {
                let cand = path.parent()?.join(&header);
                cand.is_file().then_some(cand)
            });

        // Compose: title, then the file's `@brief`, then `author (contact)`.
        let mut value = title;
        if let Some(doc) = banner_path.as_deref().and_then(doxygen::file_doc) {
            if let Some(brief) = doc.brief_line() {
                value.push_str("\n\n");
                value.push_str(&brief);
            }
            if let Some(author) = doc.author_line() {
                value.push_str("\n\n");
                value.push_str(&author);
            }
        }

        Some(json!({ "contents": { "kind": "markdown", "value": value } }))
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
        if let Some(location) = self
            .state
            .indexers
            .iter_mut()
            .find_map(|ix| ix.goto_definition(&uri, &msg))
        {
            return self.respond(msg.get("id").cloned(), location);
        }
        self.forward_by_uri(&uri, &msg)
    }

    fn include_definition(&self, uri: &str, line: usize) -> Option<Value> {
        let path = path_from_uri(uri)?;
        let text = self.doc_text(uri, &path)?;
        let line_text = text.lines().nth(line)?;
        let (header, is_system, is_module) = parse_include_header(line_text)?;
        if is_module {
            // A named module: jump to the interface unit that declares it, when
            // a declared package provides one. (std and unknown modules have no
            // openable interface in this project.)
            let entry = self.state.module_index.lookup(&header)?;
            if entry.interface_path.as_os_str().is_empty() || !entry.interface_path.exists() {
                return None;
            }
            return Some(json!({
                "uri": uri_from_path(&entry.interface_path),
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }
            }));
        }

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
        let Some(ref manifest_dir) = self.state.manifest_dir.clone() else {
            return;
        };
        let profile = self.args.profile.clone();
        for ix in &mut self.state.indexers {
            ix.refresh_flags(manifest_dir, &profile);
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
            let Some((header, is_system, is_module)) = parse_include_header(line_text) else {
                continue;
            };
            if is_module {
                continue; // no file to link to
            }

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
            self.state
                .indexers
                .iter_mut()
                .find_map(|ix| ix.document_symbols(u))
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
            self.state
                .indexers
                .iter_mut()
                .find_map(|ix| ix.folding_ranges(u))
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
            self.state
                .indexers
                .iter_mut()
                .find_map(|ix| ix.references(u, &msg))
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
            self.state
                .indexers
                .iter_mut()
                .find_map(|ix| ix.document_highlight(u, &msg))
        });
        if let Some(hls) = result {
            return self.respond(Some(id), Value::Array(hls));
        }
        self.forward_or_null(msg)
    }

    /// `textDocument/semanticTokens/full` — prefer a language indexer, else
    /// forward.  The indexer returns the LSP-encoded `data` array directly.
    ///
    /// Freight's indexers emit tokens against freight's legend, which is only the
    /// advertised global legend when the clang bridge is on (see
    /// `freight_capabilities`). With the bridge off, clangd owns the legend, so we
    /// forward *every* request to it rather than mixing freight-legend tokens
    /// (e.g. from the Fortran indexer) into a clangd-legend stream — that mismatch
    /// is what scrambles highlighting colours.
    fn handle_semantic_tokens(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let result = if self.args.use_clang_bridge {
            let uri = text_document_uri(&msg);
            uri.as_deref().and_then(|u| {
                self.state
                    .indexers
                    .iter_mut()
                    .find_map(|ix| ix.semantic_tokens(u))
            })
        } else {
            None
        };
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
            self.state
                .indexers
                .iter_mut()
                .find_map(|ix| ix.inlay_hints(u, &msg))
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

        // Lines flagged as undeclared by the include-hygiene check (if any).
        let undeclared = self.state.undeclared_includes.get(&uri);
        // Whether undeclared imports should be surfaced at all (lint != allow).
        // Computed once: the per-line loop must not reload the manifest.
        let flag_undeclared = !matches!(
            self.undeclared_include_level(),
            crate::manifest::LintLevel::Allow
        );
        // System-library stubs + already-declared features, loaded once: a
        // system-lib header (pthread.h) is labelled by its feature, not indexed.
        let sys_stubs = crate::toolchain::system_libs::load_system_lib_stubs();
        let declared_feats = self.declared_system_features();

        let mut hints = Vec::new();
        for (idx, line_text) in text.lines().enumerate() {
            if idx < start_line || idx > end_line {
                continue;
            }
            let Some((header, is_system, is_module)) = parse_include_header(line_text) else {
                continue;
            };
            let col = line_text.len();

            // C++20 named module import (`import std;`, `import mylib.core;`).
            if is_module {
                let entry = self.state.module_index.lookup(&header);
                // An undeclared module (not stdlib, not provided by any declared
                // package) gets the same ⚠ marker as an undeclared header — but
                // only on a complete statement and when the lint is active.
                if entry.is_none()
                    && !is_std_module(&header)
                    && flag_undeclared
                    && line_text.contains(';')
                {
                    hints.push(inlay_hint_json(
                        idx,
                        col,
                        "⚠ undeclared",
                        &format!(
                            "Module `{header}` is not provided by any declared dependency.\n\n\
                             Add the package that exports it to `[dependencies]` in `freight.toml`.",
                        ),
                    ));
                } else {
                    hints.push(inlay_hint_json(
                        idx,
                        col,
                        &module_inlay_label_for(&header, entry),
                        &index::module_tooltip(&header, entry),
                    ));
                }
                continue;
            }

            // Undeclared include → a warning marker (takes precedence over the
            // package annotation, which would otherwise mislabel e.g. <pthread.h>
            // as "← stdlib"). Mirrors the `undeclared-include` diagnostic.
            if let Some(spelling) = undeclared
                .and_then(|v| v.iter().find(|(l, _)| *l as usize == idx))
                .map(|(_, s)| s.as_str())
            {
                hints.push(inlay_hint_json(
                    idx,
                    col,
                    "⚠ undeclared",
                    &format!(
                        "`{spelling}` is not provided by any declared dependency.\n\n\
                         Add the dependency that provides it to `[dependencies]` in `freight.toml`.",
                    ),
                ));
                continue;
            }

            // System-library header (pthread.h, …): label it by the feature it
            // belongs to rather than indexing it (it's OS-provided, not a package
            // header — without this it would mislabel as "← stdlib").
            if is_system {
                if let Some(stub) =
                    crate::toolchain::system_libs::find_stub_by_header(&header, &sys_stubs)
                {
                    let os = stub.section_os();
                    let tip = if declared_feats.contains(&stub.name) {
                        format!(
                            "`{header}` is provided by the `{}` system library, \
                             linked via `[os.{os}] features`.",
                            stub.name
                        )
                    } else {
                        format!(
                            "`{header}` is provided by the `{}` system library, but it isn't \
                             linked.\n\nAdd `{}` to `[os.{os}] features` in `freight.toml`.",
                            stub.name, stub.name
                        )
                    };
                    hints.push(inlay_hint_json(idx, col, &format!("← {}", stub.name), &tip));
                    continue;
                }
            }

            let owned;
            let entry: &HeaderEntry = if let Some(e) = self.state.header_index.lookup(&header) {
                e
            } else if is_system {
                // File-based system lookup (e.g. <vector> → /usr/include/c++/.../vector).
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
                        pkg_dir: None,
                    });
                &owned
            } else {
                continue;
            };

            hints.push(inlay_hint_json(
                idx,
                col,
                &include_inlay_label(entry),
                &index::package_tooltip(
                    entry.pkg_dir.as_deref(),
                    &entry.package_name,
                    entry.package_version.as_deref(),
                    &entry.origin,
                ),
            ));
        }
        Some(hints)
    }

    fn handle_signature_help_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if !is_freight_manifest_uri(&uri) {
            if let Some(result) = self
                .state
                .indexers
                .iter_mut()
                .find_map(|ix| ix.signature_help(&uri, &msg))
            {
                return self.respond(msg.get("id").cloned(), result);
            }
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
        if let Some(result) = self
            .state
            .indexers
            .iter_mut()
            .find_map(|ix| ix.code_actions(&uri, &msg))
        {
            return self.respond(msg.get("id").cloned(), Value::Array(result));
        }

        // Freight-native quick-fixes for our own `undeclared-include` diagnostics:
        // "Add dependency `<pkg>` to freight.toml" for each package that owns the
        // header. These are merged with clangd's actions when clangd is serving.
        let mut freight_actions = self.undeclared_include_quickfixes(&uri, &msg);
        freight_actions.extend(self.link_feature_quickfixes(&uri, &msg));

        let goes_to_clangd = matches!(source_server_for_uri(&uri), Some(SourceServer::Clangd));
        if goes_to_clangd && self.state.clangd.is_some() {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            if freight_actions.is_empty() {
                return self.forward_by_uri(&uri, &sanitize_code_action_diagnostics(&msg));
            }
            let orig_id_str = match &id {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => "0".to_string(),
            };
            let rewritten = format!("__freight_codeaction_{orig_id_str}");
            self.state.clangd_pending.lock().unwrap().insert(
                rewritten.clone(),
                PendingClangdRequest::CodeAction {
                    original_id: id,
                    freight_actions,
                },
            );
            let mut fwd = sanitize_code_action_diagnostics(&msg);
            fwd.as_object_mut()
                .unwrap()
                .insert("id".to_string(), json!(rewritten));
            self.forward_to_passthrough(SourceServer::Clangd, &fwd)?;
            return Ok(());
        }

        if !freight_actions.is_empty() {
            return self.respond(msg.get("id").cloned(), Value::Array(freight_actions));
        }
        self.forward_by_uri(&uri, &sanitize_code_action_diagnostics(&msg))
    }

    /// Build "Add dependency `<pkg>` to freight.toml" quick-fixes for every
    /// `undeclared-include` diagnostic in the request's range whose header a
    /// known package owns (Tier A ownership). Returns an empty vec when there is
    /// nothing freight can fix, so the caller can fall through to clangd.
    fn undeclared_include_quickfixes(&self, uri: &str, msg: &Value) -> Vec<Value> {
        use crate::build::header_ownership as ho;

        let diagnostics = msg
            .get("params")
            .and_then(|p| p.get("context"))
            .and_then(|c| c.get("diagnostics"))
            .and_then(Value::as_array);
        let Some(diagnostics) = diagnostics else {
            return Vec::new();
        };

        // Resolve the freight.toml the edit will target, and the buffer/disk
        // content the WorkspaceEdit applies against.
        let Some(manifest_dir) = self.active_manifest_dir() else {
            return Vec::new();
        };
        let manifest_path = manifest_dir.join("freight.toml");
        let manifest_uri = uri_from_path(&manifest_path);
        let Some(manifest_text) = self
            .state
            .docs
            .get(&manifest_uri)
            .cloned()
            .or_else(|| std::fs::read_to_string(&manifest_path).ok())
        else {
            return Vec::new();
        };
        let end_pos = lsp_end_position(&manifest_text);

        let declared = self.declared_dep_names();
        let ownership = ho::load();
        let mut actions = Vec::new();
        for diag in diagnostics {
            if diag.get("source").and_then(Value::as_str) != Some("freight") {
                continue;
            }
            if diag.get("code").and_then(Value::as_str) != Some("undeclared-include") {
                continue; // modules have no header-ownership mapping yet
            }
            // The header spelling lives in the recorded undeclared markers, keyed
            // by the diagnostic's start line.
            let line = diag
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(Value::as_u64)
                .map(|l| l as u32);
            let Some(line) = line else { continue };
            let Some(spelling) = self
                .state
                .undeclared_includes
                .get(uri)
                .and_then(|v| v.iter().find(|(l, _)| *l == line))
                .map(|(_, s)| s.clone())
            else {
                continue;
            };
            let header = spelling.trim_matches(|c| matches!(c, '<' | '>' | '"'));
            for pkg in ownership.candidates_for_header(header) {
                if declared.contains(&pkg) {
                    continue;
                }
                let Some(new_text) = insert_dependency_toml(&manifest_text, &pkg) else {
                    continue;
                };
                actions.push(json!({
                    "title": format!("Add dependency `{pkg}` to freight.toml"),
                    "kind": "quickfix",
                    "diagnostics": [diag],
                    "edit": {
                        "changes": {
                            manifest_uri.clone(): [{
                                "range": {
                                    "start": { "line": 0, "character": 0 },
                                    "end": end_pos,
                                },
                                "newText": new_text,
                            }]
                        }
                    }
                }));
            }
        }
        actions
    }

    /// Build "Add `<feature>` to [os.<os>] features" quick-fixes for every
    /// `link-feature-hint` diagnostic in the request's range. The feature + os
    /// ride along in the diagnostic's `data` field (preserved by the client).
    fn link_feature_quickfixes(&self, _uri: &str, msg: &Value) -> Vec<Value> {
        let diagnostics = msg
            .get("params")
            .and_then(|p| p.get("context"))
            .and_then(|c| c.get("diagnostics"))
            .and_then(Value::as_array);
        let Some(diagnostics) = diagnostics else {
            return Vec::new();
        };
        let Some(manifest_dir) = self.active_manifest_dir() else {
            return Vec::new();
        };
        let manifest_path = manifest_dir.join("freight.toml");
        let manifest_uri = uri_from_path(&manifest_path);
        let Some(manifest_text) = self
            .state
            .docs
            .get(&manifest_uri)
            .cloned()
            .or_else(|| std::fs::read_to_string(&manifest_path).ok())
        else {
            return Vec::new();
        };
        let end_pos = lsp_end_position(&manifest_text);

        let mut actions = Vec::new();
        for diag in diagnostics {
            if diag.get("source").and_then(Value::as_str) != Some("freight")
                || diag.get("code").and_then(Value::as_str) != Some("link-feature-hint")
            {
                continue;
            }
            let data = diag.get("data");
            let (Some(feature), Some(os)) = (
                data.and_then(|d| d.get("feature")).and_then(Value::as_str),
                data.and_then(|d| d.get("os")).and_then(Value::as_str),
            ) else {
                continue;
            };
            let Some(new_text) = insert_os_feature_toml(&manifest_text, os, feature) else {
                continue;
            };
            actions.push(json!({
                "title": format!("Add `{feature}` to [os.{os}] features in freight.toml"),
                "kind": "quickfix",
                "diagnostics": [diag],
                "edit": {
                    "changes": {
                        manifest_uri.clone(): [{
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": end_pos,
                            },
                            "newText": new_text,
                        }]
                    }
                }
            }));
        }
        actions
    }

    fn handle_rename_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if let Some(result) = self
            .state
            .indexers
            .iter_mut()
            .find_map(|ix| ix.rename(&uri, &msg))
        {
            return self.respond(msg.get("id").cloned(), result);
        }
        self.forward_by_uri(&uri, &msg)
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

        // Current sysroot from the active manifest's [compiler] section (owned
        // project model).
        let manifest_dir = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        let current_sysroot: Option<String> = self
            .state
            .active_manifest
            .as_ref()
            .and_then(|m| m.compiler.sysroot.clone());

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

/// Insert `pkg = "<version>"` into the `[dependencies]` table of a `freight.toml`,
/// preserving comments/formatting via `toml_edit`. The quick-fix targets
/// undeclared *system* headers (zlib, openssl, …), so the library is installed —
/// its concrete version comes from pkg-config `--modversion` (freight forbids a
/// bare `*`). Returns the rewritten text, or `None` if the dep is already
/// present, the version can't be determined, or the document won't parse.
fn insert_dependency_toml(text: &str, pkg: &str) -> Option<String> {
    let version = crate::adaptors::pkg_config_version(pkg);
    if version.is_empty() {
        return None; // can't pin a version → don't offer an invalid `*` fix
    }
    insert_dependency_toml_version(text, pkg, &version)
}

/// [`insert_dependency_toml`] with the version supplied explicitly (testable
/// without invoking pkg-config).
fn insert_dependency_toml_version(text: &str, pkg: &str, version: &str) -> Option<String> {
    use toml_edit::{value as tv, DocumentMut, Item, Table};
    let mut doc: DocumentMut = text.parse().ok()?;
    if doc.get("dependencies").is_none() {
        let mut t = Table::new();
        t.set_implicit(false);
        doc["dependencies"] = Item::Table(t);
    }
    let deps = doc.get_mut("dependencies").and_then(Item::as_table_mut)?;
    if deps.contains_key(pkg) {
        return None; // already declared — nothing to add
    }
    deps[pkg] = tv(version);
    Some(doc.to_string())
}

/// Add a system-library `feature` to `[os.<os>] features` (creating the section
/// and/or array as needed), preserving the rest of the document's formatting.
/// Returns `None` if the feature is already present.
fn insert_os_feature_toml(text: &str, os: &str, feature: &str) -> Option<String> {
    use toml_edit::{Array, DocumentMut, Item, Table, Value as TomlValue};
    let mut doc: DocumentMut = text.parse().ok()?;

    if doc.get("os").is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        doc["os"] = Item::Table(t);
    }
    let os_tbl = doc.get_mut("os").and_then(Item::as_table_mut)?;
    if os_tbl.get(os).is_none() {
        os_tbl[os] = Item::Table(Table::new());
    }
    let sec = os_tbl.get_mut(os).and_then(Item::as_table_mut)?;

    if sec.get("features").is_none() {
        sec["features"] = Item::Value(TomlValue::Array(Array::new()));
    }
    let arr = sec.get_mut("features").and_then(Item::as_array_mut)?;
    if arr.iter().any(|v| v.as_str() == Some(feature)) {
        return None; // already declared
    }
    arr.push(feature);
    Some(doc.to_string())
}

/// The LSP `Position` just past the last character of `text` (UTF-16 units), used
/// as the end of a whole-document replace range.
fn lsp_end_position(text: &str) -> Value {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
    }
    json!({ "line": line, "character": col })
}

fn merge_clangd_codeaction_response(
    mut msg: Value,
    original_id: Value,
    mut freight_actions: Vec<Value>,
) -> Value {
    // Freight quick-fixes first (they're the targeted fix for our own
    // diagnostic), then whatever clangd offered for the same range.
    let clangd_actions = msg
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    freight_actions.extend(clangd_actions);
    if let Some(obj) = msg.as_object_mut() {
        obj.insert("id".to_string(), original_id);
        obj.insert("result".to_string(), Value::Array(freight_actions));
    }
    msg
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
        let Some(path) = path_from_uri(uri) else {
            return;
        };
        let flags: Vec<String> = self
            .state
            .indexers
            .iter()
            .flat_map(|ix| ix.flags_for(&path))
            .collect();
        let uri = uri.to_string();
        let out = Arc::clone(&self.out);
        let cache = Arc::clone(&self.state.diag_cache);
        thread::spawn(move || {
            let path_str = path.to_string_lossy().into_owned();
            let flag_refs: Vec<&str> = flags.iter().map(String::as_str).collect();
            let tidy_diags: Vec<Value> = clang_bridge::tidy::run(None, &path_str, None, &flag_refs)
                .filter(|d| d.file == path_str)
                .map(|d| indexers::Clang::diag_to_lsp(&d, "clang-tidy"))
                .collect();
            tracing::debug!(file = %path_str, count = tidy_diags.len(), "clang-tidy done");
            let mut guard = cache.lock().unwrap();
            let entry = guard.entry(uri.clone()).or_default();
            entry.tidy = tidy_diags;
            let merged: Vec<Value> = entry
                .clangd
                .iter()
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
        let Some(path) = path_from_uri(uri) else {
            return;
        };

        // Fast path: if the #include/import directives are unchanged, the
        // diagnostics and undeclared markers are already current — do no work.
        // (Cleared on compile-commands refresh so dep changes still re-check.)
        let directives = ip::parse_includes(text);
        if self.state.last_includes.get(uri) == Some(&directives) {
            return;
        }
        // Named-module imports for the module-undeclared check below; captured
        // before `directives` is moved into the cache.
        let module_imports: Vec<(u32, u32, u32, String)> = directives
            .iter()
            .filter(|d| d.kind == ip::DirectiveKind::Module)
            .map(|d| (d.line, d.start_col, d.end_col, d.name.clone()))
            .collect();
        // Header directives, for the link-feature hint below (captured before the
        // cache move). Only `<…>` system headers can name a system library.
        let header_dirs: Vec<(u32, u32, u32, String)> = directives
            .iter()
            .filter(|d| d.kind == ip::DirectiveKind::Header && d.angled)
            .map(|d| (d.line, d.start_col, d.end_col, d.name.clone()))
            .collect();
        self.state.last_includes.insert(uri.to_string(), directives);

        let mut diags: Vec<Value> = Vec::new();
        let mut undeclared_lines: Vec<(u32, String)> = Vec::new();

        // (1) Undeclared include/module diagnostics — only when the lint is active.
        if let Some(severity) = match self.undeclared_include_level() {
            crate::manifest::LintLevel::Allow => None,
            crate::manifest::LintLevel::Warn => Some(2), // DiagnosticSeverity::Warning
            crate::manifest::LintLevel::Deny => Some(1), // DiagnosticSeverity::Error
        } {
            let file_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let (declared, compiler) = self.cached_declared_dirs(&path);
            let lang = ip::Language::from_path(&path);
            let system = self.cached_system_dirs(compiler.as_deref(), lang);

            let findings = ip::check_includes(text, &file_dir, &declared, &system, lang);

            // Phase 3 ownership: a header a declared package/slot owns (e.g. a
            // declared BLAS provider owns `cblas.h`) is not undeclared. Tier A only
            // here — pkg-config subdir libs already appear in `declared` via the
            // compile command, and the LSP hot path must avoid per-keystroke
            // subprocesses. Suppress owned headers and name candidates for the rest.
            use crate::build::header_ownership as ho;
            let declared_names = self.declared_dep_names();
            let ownership = ho::load();
            let owned_globs = ownership.owned_globs_for(&declared_names);
            let describe = |spelling: &str| -> String {
                let name = spelling.trim_matches(|c| matches!(c, '<' | '>' | '"'));
                let candidates: Vec<String> = ownership
                    .candidates_for_header(name)
                    .into_iter()
                    .filter(|c| !declared_names.contains(c))
                    .collect();
                if candidates.is_empty() {
                    format!(
                        "{spelling} is not provided by any declared dependency; add the \
                         dependency that provides it to [dependencies] in freight.toml"
                    )
                } else {
                    format!(
                        "{spelling} is provided by {} — add one to [dependencies] in freight.toml",
                        candidates.join(", ")
                    )
                }
            };
            let findings: Vec<_> = findings
                .into_iter()
                .filter(|f| {
                    let name = f.spelling.trim_matches(|c| matches!(c, '<' | '>' | '"'));
                    !owned_globs.iter().any(|g| ho::glob_match(g, name))
                })
                .collect();

            for f in &findings {
                undeclared_lines.push((f.line, f.spelling.clone()));
                diags.push(json!({
                    "range": {
                        "start": { "line": f.line, "character": f.start_col },
                        "end":   { "line": f.line, "character": f.end_col }
                    },
                    "severity": severity,
                    "source": "freight",
                    "code": "undeclared-include",
                    "message": describe(&f.spelling),
                }));
            }

            // Named-module imports that no declared package exports (and that aren't
            // standard-library modules) are flagged the same way as headers.
            for (line, start_col, end_col, name) in &module_imports {
                if is_std_module(name) || self.state.module_index.lookup(name).is_some() {
                    continue;
                }
                undeclared_lines.push((*line, format!("module {name}")));
                diags.push(json!({
                    "range": {
                        "start": { "line": line, "character": start_col },
                        "end":   { "line": line, "character": end_col }
                    },
                    "severity": severity,
                    "source": "freight",
                    "code": "undeclared-module",
                    "message": format!(
                        "module {name} is not provided by any declared dependency; add the \
                         package that exports it to [dependencies] in freight.toml"
                    ),
                }));
            }
        }

        // (2) Link-feature hints — independent of the lint level. Including a
        // system-library header (pthread.h, …) whose feature isn't declared in any
        // `[os.*] features` compiles but won't link; offer a hint + quick-fix.
        for h in self.link_feature_hints(&header_dirs) {
            diags.push(json!({
                "range": {
                    "start": { "line": h.line, "character": h.start_col },
                    "end":   { "line": h.line, "character": h.end_col }
                },
                "severity": 4, // DiagnosticSeverity::Hint
                "source": "freight",
                "code": "link-feature-hint",
                "message": format!(
                    "<{}> is the '{}' system library — add `{}` to [os.{}] features in \
                     freight.toml to link it",
                    h.header, h.feature, h.feature, h.os
                ),
                "data": { "feature": h.feature, "os": h.os },
            }));
        }

        self.state
            .undeclared_includes
            .insert(uri.to_string(), undeclared_lines);

        self.set_freight_diags(uri, diags);
    }

    /// A system-library header (`pthread.h`, `winsock2.h`, …) whose feature isn't
    /// declared in any `[os.*]`/`[arch.*] features`. One hint per feature (first
    /// occurrence). Drives the `link-feature-hint` diagnostic + quick-fix.
    fn link_feature_hints(&self, header_dirs: &[(u32, u32, u32, String)]) -> Vec<LinkFeatureHint> {
        use crate::toolchain::system_libs as sl;
        if header_dirs.is_empty() {
            return Vec::new();
        }
        let stubs = sl::load_system_lib_stubs();
        if stubs.is_empty() {
            return Vec::new();
        }
        let declared = self.declared_system_features();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut hints = Vec::new();
        for (line, start_col, end_col, name) in header_dirs {
            let Some(stub) = sl::find_stub_by_header(name, &stubs) else {
                continue;
            };
            if declared.contains(&stub.name) || !seen.insert(stub.name.clone()) {
                continue;
            }
            hints.push(LinkFeatureHint {
                line: *line,
                start_col: *start_col,
                end_col: *end_col,
                header: name.clone(),
                feature: stub.name.clone(),
                os: stub.section_os().to_string(),
            });
        }
        hints
    }

    /// System-library feature names already declared in any `[os.*]`/`[arch.*]`
    /// `features` of the active manifest.
    fn declared_system_features(&self) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        if let Some(m) = &self.state.active_manifest {
            for sec in m.os.values().chain(m.arch.values()) {
                for f in &sec.features {
                    set.insert(f.clone());
                }
            }
        }
        set
    }

    /// Store freight-generated diagnostics for `uri` and re-publish the merged set.
    fn set_freight_diags(&self, uri: &str, diags: Vec<Value>) {
        let merged: Vec<Value> = {
            let mut guard = self.state.diag_cache.lock().unwrap();
            let entry = guard.entry(uri.to_string()).or_default();
            entry.freight = diags;
            entry
                .clangd
                .iter()
                .chain(entry.tidy.iter())
                .chain(entry.freight.iter())
                .cloned()
                .collect()
        };
        let _ = self.publish_diagnostics(uri, merged);
    }

    /// The `[lints].undeclared-include` level for the active project (default warn).
    fn undeclared_include_level(&self) -> crate::manifest::LintLevel {
        self.state
            .active_manifest
            .as_ref()
            .map(|m| m.lints.undeclared_include)
            .unwrap_or_default()
    }

    /// Declared dependency keys for the active project — the freight package
    /// names that legitimise a system header under include hygiene.
    fn declared_dep_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .state
            .active_manifest
            .as_ref()
            .map(|m| m.effective_dependencies().into_keys().collect())
            .unwrap_or_default();
        v.sort();
        v.dedup();
        v
    }

    /// Cached wrapper around [`Self::declared_dirs_and_compiler`]. Loading and
    /// canonicalizing compile_commands.json on every keystroke made the inlay
    /// hints lag; the result only changes when the compile commands are
    /// regenerated, so it's cached per file and invalidated on refresh.
    fn cached_declared_dirs(&mut self, path: &Path) -> (Vec<PathBuf>, Option<String>) {
        if let Some(cached) = self.state.declared_dirs_cache.get(path) {
            return cached.clone();
        }
        let result = self.declared_dirs_and_compiler(path);
        self.state
            .declared_dirs_cache
            .insert(path.to_path_buf(), result.clone());
        result
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
            c.file
                .canonicalize()
                .map(|p| p == target)
                .unwrap_or(c.file == *path)
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
        // Cross build: probe the target sysroot from the active manifest so cross
        // system headers resolve there instead of the host's /usr/include.
        let sysroot: Option<String> = self
            .state
            .active_manifest
            .as_ref()
            .and_then(|m| m.compiler.sysroot.clone());
        if let Some((cached_root, dirs)) = &self.state.system_include_dirs {
            if *cached_root == sysroot {
                return dirs.clone();
            }
        }
        let cc = compiler.unwrap_or("c++");
        let dirs = crate::build::include_policy::system_include_dirs(
            Path::new(cc),
            lang,
            sysroot.as_deref().map(Path::new),
        );
        self.state.system_include_dirs = Some((sysroot, dirs.clone()));
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

    fn publish_indexer_diagnostics(&mut self, uri: &str) -> io::Result<()> {
        let Some(path) = path_from_uri(uri) else {
            return Ok(());
        };
        let mut handled = false;
        let mut diagnostics = Vec::new();
        for ix in &mut self.state.indexers {
            if ix.handles(&path) {
                handled = true;
                diagnostics.extend(ix.diagnostics(uri));
            }
        }
        if handled {
            self.publish_diagnostics(uri, diagnostics)?;
        }
        Ok(())
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

    /// Recompute the owned active-project model — the active package's parsed
    /// manifest and the package layout (project + workspace members + path deps).
    /// Driven from [`Server::refresh_compile_commands`], the convergence point for
    /// every manifest-set change, so per-request handlers read owned state instead
    /// of re-deriving it. `active_manifest` is `None` for a workspace root (no
    /// `[package]`) or before any manifest is located.
    fn refresh_project_model(&mut self) {
        let active_dir = self.active_manifest_dir();
        self.state.active_manifest = active_dir
            .as_deref()
            .and_then(|d| load_manifest_cached(d).ok());
        self.state.package_dirs = active_dir
            .as_deref()
            .map(crate::build::source_package_dirs)
            .unwrap_or_default();
    }

    fn refresh_compile_commands(&mut self) {
        // Refresh the owned project model first so the header index and the
        // manifest-backed read sites see the current package set.
        self.refresh_project_model();
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
        // Compile commands changed: drop the per-file include-dir cache and the
        // include-hygiene fast-path memo so dep/manifest changes re-check.
        self.state.declared_dirs_cache.clear();
        self.state.last_includes.clear();
        self.refresh_indexer_flags();
        self.refresh_header_index();
    }

    fn refresh_header_index(&mut self) {
        let base = self
            .active_manifest_dir()
            .unwrap_or_else(|| self.state.root_dir.clone());
        // Package source dirs come from the owned project model (computed in
        // refresh_project_model); the index just maps them to HeaderDirSpec.
        // `.pkgs/` (fetched) is handled inside the index builder.
        let specs: Vec<HeaderDirSpec<'_>> = self
            .state
            .package_dirs
            .iter()
            .map(|(dir, kind, dep_key)| HeaderDirSpec {
                path: dir.as_path(),
                origin: match kind {
                    crate::build::PackageKind::Own => HeaderOrigin::Own,
                    crate::build::PackageKind::PathDep => HeaderOrigin::PathDep,
                },
                dep_key: dep_key.clone(),
            })
            .collect();
        let pkgs_dir = base.join(".pkgs");
        let pkgs_opt = if pkgs_dir.is_dir() {
            Some(pkgs_dir.as_path())
        } else {
            None
        };
        // One traversal of each package's src/ fills both indexes.
        let indexes = crate::lsp::index::build_source_indexes(&specs, pkgs_opt);
        self.state.header_index = indexes.headers;
        self.state.module_index = indexes.modules;
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
                            Some(PendingClangdRequest::CodeAction {
                                original_id,
                                freight_actions,
                            }) => {
                                let response = merge_clangd_codeaction_response(
                                    msg,
                                    original_id,
                                    freight_actions,
                                );
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
                        let uri = msg["params"]["uri"].as_str().unwrap_or("").to_string();
                        let new_diags = msg["params"]["diagnostics"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        let mut guard = cache.lock().unwrap();
                        let entry = guard.entry(uri.clone()).or_default();
                        entry.clangd = new_diags;
                        let merged: Vec<Value> = entry
                            .clangd
                            .iter()
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
    let manifest = load_manifest_cached(dir).ok()?;
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
    use super::build_workspace_inventory;
    use super::protocol::sanitize_code_action_diagnostics;
    use super::{
        insert_dependency_toml_version, insert_os_feature_toml, lsp_end_position,
        merge_clangd_codeaction_response,
    };
    use serde_json::json;

    #[test]
    fn insert_os_feature_creates_section() {
        let src = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let out = insert_os_feature_toml(src, "unix", "pthread").expect("inserts");
        let m = crate::manifest::load_manifest_str(&out).expect("valid");
        assert!(m
            .os
            .get("unix")
            .is_some_and(|s| s.features.contains(&"pthread".to_string())));
    }

    #[test]
    fn insert_os_feature_appends_to_existing() {
        let src = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[os.unix]\nfeatures = [\"m\"]\n";
        let out = insert_os_feature_toml(src, "unix", "pthread").expect("inserts");
        let feats = crate::manifest::load_manifest_str(&out).unwrap().os["unix"]
            .features
            .clone();
        assert!(feats.contains(&"m".to_string()));
        assert!(feats.contains(&"pthread".to_string()));
    }

    #[test]
    fn insert_os_feature_idempotent() {
        let src = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[os.unix]\nfeatures = [\"pthread\"]\n";
        assert!(insert_os_feature_toml(src, "unix", "pthread").is_none());
    }

    #[test]
    fn insert_dependency_adds_to_dependencies_table() {
        let src = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
                   [dependencies]\nzlib = \"1.3\"\n";
        // The quick-fix pins the pkg-config version; test the version-explicit core.
        let out = insert_dependency_toml_version(src, "openssl", "3.0.2").expect("inserts dep");
        assert!(out.contains("openssl = \"3.0.2\""), "got:\n{out}");
        assert!(out.contains("zlib = \"1.3\""), "keeps existing:\n{out}");
        // Idempotent: re-adding an already-declared dep yields None.
        assert!(insert_dependency_toml_version(&out, "openssl", "3.0.2").is_none());
        // The result is a valid manifest (concrete versions, no bare `*`).
        assert!(crate::manifest::load_manifest_str(&out).is_ok());
    }

    #[test]
    fn insert_dependency_creates_section_when_missing() {
        let src = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let out = insert_dependency_toml_version(src, "fmt", "10.1.0").expect("inserts dep");
        assert!(out.contains("[dependencies]"), "got:\n{out}");
        assert!(out.contains("fmt = \"10.1.0\""), "got:\n{out}");
        // Round-trips back into a valid manifest.
        assert!(crate::manifest::load_manifest_str(&out).is_ok());
    }

    #[test]
    fn end_position_counts_lines_and_utf16() {
        assert_eq!(
            lsp_end_position("a\nbc"),
            json!({"line": 1, "character": 2})
        );
        // Trailing newline → cursor at column 0 of the next line.
        assert_eq!(lsp_end_position("a\n"), json!({"line": 1, "character": 0}));
        // Astral char (emoji) is 2 UTF-16 units.
        assert_eq!(lsp_end_position("x😀"), json!({"line": 0, "character": 3}));
    }

    #[test]
    fn codeaction_merge_puts_freight_actions_first() {
        let clangd = json!({
            "jsonrpc": "2.0", "id": "__freight_codeaction_7",
            "result": [{ "title": "clangd fix" }]
        });
        let freight = vec![json!({ "title": "Add dependency `zlib` to freight.toml" })];
        let merged = merge_clangd_codeaction_response(clangd, json!(7), freight);
        assert_eq!(merged["id"], json!(7));
        let arr = merged["result"].as_array().unwrap();
        assert_eq!(arr[0]["title"], "Add dependency `zlib` to freight.toml");
        assert_eq!(arr[1]["title"], "clangd fix");
    }

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
