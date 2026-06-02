//! `freight lsp` — Language Server Protocol multiplexer for freight.toml and
//! source files (clangd, fortls, asm-lsp passthroughs).

mod manifest;
mod protocol;

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use freight_core::build::generate_lsp_compile_commands_at;
use freight_core::manifest::find_manifest_dir;
use freight_core::toolchain::load_all_templates;
use serde_json::{json, Value};

use manifest::{completion_result, hover_result, manifest_diagnostics, signature_help_result};
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
        if let Err(e) = Server::new(self).run() {
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
    templates: Vec<freight_core::toolchain::CompilerTemplate>,
    clangd: Option<Passthrough>,
    fortls: Option<Passthrough>,
    asm_lsp: Option<Passthrough>,
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
    fn new(args: Args) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let manifest_dir = find_manifest_dir(&cwd);
        let root_dir = manifest_dir.clone().unwrap_or(cwd);
        Self {
            args,
            out: Arc::new(Mutex::new(io::stdout())),
            state: ServerState {
                root_dir,
                manifest_dir,
                compile_commands_dir: None,
                docs: HashMap::new(),
                templates: load_all_templates(),
                clangd: None,
                fortls: None,
                asm_lsp: None,
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
                "textDocument/didOpen"             => self.handle_did_open(msg)?,
                "textDocument/didChange"           => self.handle_did_change(msg)?,
                "textDocument/didSave"             => self.handle_did_save(msg)?,
                "textDocument/didClose"            => self.handle_did_close(msg)?,
                "workspace/didChangeWatchedFiles"  => self.handle_watched_files_changed(msg)?,
                "textDocument/completion"          => self.handle_completion_or_forward(msg)?,
                "textDocument/hover"               => self.handle_hover_or_forward(msg)?,
                "textDocument/signatureHelp"       => self.handle_signature_help_or_forward(msg)?,
                "textDocument/codeAction"          => self.handle_code_action_or_forward(msg)?,
                _                                  => self.forward_or_null(msg)?,
            }
        }
        self.kill_passthroughs();
        Ok(())
    }

    fn handle_initialize(&mut self, msg: Value) -> io::Result<()> {
        if let Some(root) = root_from_initialize(&msg) {
            self.state.root_dir = root;
            self.state.manifest_dir = find_manifest_dir(&self.state.root_dir);
        }
        self.refresh_compile_commands();
        let mut source_caps = Vec::new();
        if !self.args.no_clangd {
            if let Some(caps) = self.start_clangd(&msg) { source_caps.push(caps); }
        }
        if !self.args.no_fortls {
            if let Some(caps) = self.start_fortls(&msg) { source_caps.push(caps); }
        }
        if !self.args.no_asm_lsp {
            if let Some(caps) = self.start_asm_lsp(&msg) { source_caps.push(caps); }
        }
        let capabilities = merged_capabilities(source_caps);
        self.respond(msg.get("id").cloned(), json!({
            "capabilities": capabilities,
            "serverInfo": { "name": "freight", "version": env!("CARGO_PKG_VERSION") }
        }))
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
        }
        self.forward_by_text_document(&msg)
    }

    fn handle_did_change(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_to_all_passthroughs(&msg);
        };
        if !is_freight_manifest_uri(&uri) { return self.forward_by_uri(&uri, &msg); }
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
        if !is_freight_manifest_uri(&uri) { return self.forward_by_uri(&uri, &msg); }
        if let Some(text) = msg.get("params").and_then(|p| p.get("text")).and_then(Value::as_str) {
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
            .get("params").and_then(|p| p.get("changes")).and_then(Value::as_array)
            .into_iter().flatten()
            .filter_map(|c| c.get("uri").and_then(Value::as_str))
            .filter(|uri| is_freight_manifest_uri(uri))
            .map(ToString::to_string).collect();
        if manifest_changes.is_empty() { return self.forward_to_all_passthroughs(&msg); }
        for uri in manifest_changes {
            if let Some(path) = path_from_uri(&uri) {
                if let Some(parent) = path.parent() {
                    self.state.manifest_dir = Some(parent.to_path_buf());
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    self.state.docs.insert(uri.clone(), text);
                }
            }
            self.publish_manifest_diagnostics(&uri)?;
        }
        self.refresh_compile_commands();
        self.notify_compile_commands_changed()
    }

    fn handle_completion_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else { return self.forward_or_null(msg); };
        if !is_freight_manifest_uri(&uri) { return self.forward_or_null(msg); }
        let text = self.manifest_text(&uri);
        let result = completion_result(text.as_deref(), position(&msg));
        self.respond(msg.get("id").cloned(), result)
    }

    fn handle_hover_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else { return self.forward_or_null(msg); };
        if !is_freight_manifest_uri(&uri) { return self.forward_or_null(msg); }
        let text = self.manifest_text(&uri);
        let result = hover_result(text.as_deref(), position(&msg));
        self.respond(msg.get("id").cloned(), result.unwrap_or(Value::Null))
    }

    fn handle_signature_help_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else { return self.forward_or_null(msg); };
        if !is_freight_manifest_uri(&uri) { return self.forward_by_uri(&uri, &msg); }
        let text = self.manifest_text(&uri);
        let result = signature_help_result(text.as_deref(), position(&msg));
        self.respond(msg.get("id").cloned(), result.unwrap_or(Value::Null))
    }

    fn handle_code_action_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else { return self.forward_or_null(msg); };
        if is_freight_manifest_uri(&uri) { return self.respond(msg.get("id").cloned(), json!([])); }
        self.forward_by_uri(&uri, &sanitize_code_action_diagnostics(&msg))
    }

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
        self.forward_to_passthrough(kind, msg)
            .and_then(|sent| if sent { Ok(()) } else { self.respond_null_if_request(msg) })
    }

    fn forward_to_passthrough(&mut self, kind: SourceServer, msg: &Value) -> io::Result<bool> {
        let server = match kind {
            SourceServer::Clangd => self.state.clangd.as_ref(),
            SourceServer::Fortls => self.state.fortls.as_ref(),
            SourceServer::AsmLsp => self.state.asm_lsp.as_ref(),
        };
        let Some(server) = server else { return Ok(false); };
        write_lsp_message(&mut *server.stdin.lock().unwrap(), msg)?;
        Ok(true)
    }

    fn forward_to_all_passthroughs(&mut self, msg: &Value) -> io::Result<()> {
        for kind in [SourceServer::Clangd, SourceServer::Fortls, SourceServer::AsmLsp] {
            let _ = self.forward_to_passthrough(kind, msg);
        }
        Ok(())
    }

    fn respond_null_if_request(&self, msg: &Value) -> io::Result<()> {
        if msg.get("id").is_some() { self.respond(msg.get("id").cloned(), Value::Null) }
        else { Ok(()) }
    }

    fn respond(&self, id: Option<Value>, result: Value) -> io::Result<()> {
        let Some(id) = id else { return Ok(()); };
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
        self.state.docs.get(uri).cloned()
            .or_else(|| path_from_uri(uri).and_then(|p| std::fs::read_to_string(p).ok()))
    }

    fn refresh_compile_commands(&mut self) {
        let Some(dir) = self.state.manifest_dir.as_deref() else { return; };
        if let Ok(dir) = generate_lsp_compile_commands_at(dir, &self.args.profile) {
            self.state.compile_commands_dir = Some(dir);
        }
    }

    fn notify_compile_commands_changed(&mut self) -> io::Result<()> {
        let Some(dir) = self.compile_commands_dir() else { return Ok(()); };
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

    fn start_clangd(&mut self, initialize_msg: &Value) -> Option<Value> {
        let dir = self.compile_commands_dir()?;
        let compile_commands_arg = format!("--compile-commands-dir={}", dir.display());
        let (server, caps) = self.start_passthrough(
            "clangd", &self.args.clangd,
            &[compile_commands_arg, "--background-index=false".to_string(), "--header-insertion=never".to_string()],
            INTERNAL_CLANGD_INIT_ID, initialize_msg,
        )?;
        self.state.clangd = Some(server);
        caps
    }

    fn start_fortls(&mut self, initialize_msg: &Value) -> Option<Value> {
        let (server, caps) = self.start_passthrough(
            "fortls", &self.args.fortls, &[], INTERNAL_FORTLS_INIT_ID, initialize_msg,
        )?;
        self.state.fortls = Some(server);
        caps
    }

    fn start_asm_lsp(&mut self, initialize_msg: &Value) -> Option<Value> {
        let (server, caps) = self.start_passthrough(
            "asm-lsp", &self.args.asm_lsp, &[], INTERNAL_ASM_LSP_INIT_ID, initialize_msg,
        )?;
        self.state.asm_lsp = Some(server);
        caps
    }

    fn start_passthrough(
        &self,
        _name: &str,
        command: &str,
        args: &[String],
        init_id: &str,
        initialize_msg: &Value,
    ) -> Option<(Passthrough, Option<Value>)> {
        let mut child = Command::new(command)
            .args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().ok()?;
        let mut child_stdin = child.stdin.take()?;
        let child_stdout = child.stdout.take()?;
        let mut init = initialize_msg.clone();
        if let Some(obj) = init.as_object_mut() { obj.insert("id".to_string(), json!(init_id)); }
        write_lsp_message(&mut child_stdin, &init).ok()?;
        let mut reader = BufReader::new(child_stdout);
        let init_response = loop {
            let msg = read_lsp_message(&mut reader).ok()??;
            if msg.get("id").and_then(Value::as_str) == Some(init_id) { break msg; }
        };
        let caps = init_response.get("result").and_then(|r| r.get("capabilities")).cloned();
        let out = Arc::clone(&self.out);
        thread::spawn(move || {
            while let Ok(Some(msg)) = read_lsp_message(&mut reader) {
                if is_internal_passthrough_response(&msg) { continue; }
                let _ = write_lsp_message(&mut *out.lock().unwrap(), &msg);
            }
        });
        Some((Passthrough { child, stdin: Arc::new(Mutex::new(child_stdin)) }, caps))
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
        if let Some(mut s) = self.state.clangd.take() { let _ = s.child.kill(); }
        if let Some(mut s) = self.state.fortls.take() { let _ = s.child.kill(); }
        if let Some(mut s) = self.state.asm_lsp.take() { let _ = s.child.kill(); }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::protocol::sanitize_code_action_diagnostics;
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
        let diagnostics = sanitized["params"]["context"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics[0]["code"], json!("123"));
        assert!(diagnostics[1]["code"].is_string());
        assert_eq!(diagnostics[2]["code"], json!("already-string"));
    }
}
