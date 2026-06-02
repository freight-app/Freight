use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use freight_core::build::generate_lsp_compile_commands_at;
use freight_core::manifest::{find_manifest_dir, load_manifest_str, validate, validate_dep_compat};
use freight_core::toolchain::{load_all_templates, CompilerTemplate};
use serde_json::{json, Value};

#[derive(clap::Args)]
pub struct Args {
    /// clangd executable to use for source-file language features.
    #[arg(long, default_value = "clangd")]
    pub clangd: String,
    /// Disable clangd passthrough for C-family source files.
    #[arg(long)]
    pub no_clangd: bool,
    /// fortls executable to use for Fortran language features.
    #[arg(long, default_value = "fortls")]
    pub fortls: String,
    /// Disable fortls passthrough.
    #[arg(long)]
    pub no_fortls: bool,
    /// asm-lsp executable to use for assembly language features.
    #[arg(long, default_value = "asm-lsp")]
    pub asm_lsp: String,
    /// Disable asm-lsp passthrough.
    #[arg(long)]
    pub no_asm_lsp: bool,
    /// Build profile used when refreshing compile_commands.json for clangd.
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

const INTERNAL_ID_PREFIX: &str = "__freight_";
const INTERNAL_CLANGD_INIT_ID: &str = "__freight_clangd_initialize";
const INTERNAL_FORTLS_INIT_ID: &str = "__freight_fortls_initialize";
const INTERNAL_ASM_LSP_INIT_ID: &str = "__freight_asm_lsp_initialize";

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
    templates: Vec<CompilerTemplate>,
    clangd: Option<Passthrough>,
    fortls: Option<Passthrough>,
    asm_lsp: Option<Passthrough>,
}

struct Passthrough {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceServer {
    Clangd,
    Fortls,
    AsmLsp,
}

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
                "shutdown" => {
                    self.handle_shutdown(msg)?;
                }
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
                _ => self.forward_or_null(msg)?,
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
                "serverInfo": {
                    "name": "freight",
                    "version": env!("CARGO_PKG_VERSION")
                }
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
            .filter_map(|change| change.get("uri").and_then(Value::as_str))
            .filter(|uri| is_freight_manifest_uri(uri))
            .map(ToString::to_string)
            .collect();

        if manifest_changes.is_empty() {
            return self.forward_to_all_passthroughs(&msg);
        }

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
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if !is_freight_manifest_uri(&uri) {
            return self.forward_or_null(msg);
        }
        let text = self.manifest_text(&uri);
        let pos = position(&msg);
        let result = completion_result(text.as_deref(), pos);
        self.respond(msg.get("id").cloned(), result)
    }

    fn handle_hover_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if !is_freight_manifest_uri(&uri) {
            return self.forward_or_null(msg);
        }
        let text = self.manifest_text(&uri);
        let pos = position(&msg);
        let result = hover_result(text.as_deref(), pos);
        self.respond(msg.get("id").cloned(), result.unwrap_or(Value::Null))
    }

    fn handle_signature_help_or_forward(&mut self, msg: Value) -> io::Result<()> {
        let Some(uri) = text_document_uri(&msg) else {
            return self.forward_or_null(msg);
        };
        if !is_freight_manifest_uri(&uri) {
            return self.forward_by_uri(&uri, &msg);
        }
        let text = self.manifest_text(&uri);
        let pos = position(&msg);
        let result = signature_help_result(text.as_deref(), pos);
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
        let Some(server) = server else {
            return Ok(false);
        };
        write_lsp_message(&mut *server.stdin.lock().unwrap(), msg)?;
        Ok(true)
    }

    fn forward_to_all_passthroughs(&mut self, msg: &Value) -> io::Result<()> {
        let kinds = [SourceServer::Clangd, SourceServer::Fortls, SourceServer::AsmLsp];
        for kind in kinds {
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
            "params": {
                "registrations": [{
                    "id": "freight-toml-watch",
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": {
                        "watchers": [{
                            "globPattern": "**/freight.toml",
                            "kind": 7
                        }]
                    }
                }]
            }
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
            "params": {
                "uri": uri,
                "diagnostics": diagnostics
            }
        }))
    }

    fn manifest_text(&self, uri: &str) -> Option<String> {
        self.state.docs.get(uri).cloned().or_else(|| {
            path_from_uri(uri).and_then(|path| std::fs::read_to_string(path).ok())
        })
    }

    fn refresh_compile_commands(&mut self) {
        let Some(dir) = self.state.manifest_dir.as_deref() else {
            return;
        };
        if let Ok(compile_commands_dir) = generate_lsp_compile_commands_at(dir, &self.args.profile) {
            self.state.compile_commands_dir = Some(compile_commands_dir);
        }
    }

    fn notify_compile_commands_changed(&mut self) -> io::Result<()> {
        let Some(dir) = self.compile_commands_dir() else {
            return Ok(());
        };
        let uri = uri_from_path(&dir.join("compile_commands.json"));
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWatchedFiles",
            "params": {
                "changes": [{ "uri": uri, "type": 2 }]
            }
        });
        self.forward_to_all_passthroughs(&msg)
    }

    fn start_clangd(&mut self, initialize_msg: &Value) -> Option<Value> {
        let dir = self.compile_commands_dir()?;
        let compile_commands_arg = format!("--compile-commands-dir={}", dir.display());
        let (server, caps) = self.start_passthrough(
            "clangd",
            &self.args.clangd,
            &[
                compile_commands_arg,
                "--background-index=false".to_string(),
                "--header-insertion=never".to_string(),
            ],
            INTERNAL_CLANGD_INIT_ID,
            initialize_msg,
        )?;
        self.state.clangd = Some(server);
        caps
    }

    fn compile_commands_dir(&self) -> Option<PathBuf> {
        self.state.compile_commands_dir.clone()
    }

    fn start_fortls(&mut self, initialize_msg: &Value) -> Option<Value> {
        let (server, caps) = self.start_passthrough(
            "fortls",
            &self.args.fortls,
            &[],
            INTERNAL_FORTLS_INIT_ID,
            initialize_msg,
        )?;
        self.state.fortls = Some(server);
        caps
    }

    fn start_asm_lsp(&mut self, initialize_msg: &Value) -> Option<Value> {
        let (server, caps) = self.start_passthrough(
            "asm-lsp",
            &self.args.asm_lsp,
            &[],
            INTERNAL_ASM_LSP_INIT_ID,
            initialize_msg,
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
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

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
        if let Some(mut server) = self.state.clangd.take() {
            let _ = server.child.kill();
        }
        if let Some(mut server) = self.state.fortls.take() {
            let _ = server.child.kill();
        }
        if let Some(mut server) = self.state.asm_lsp.take() {
            let _ = server.child.kill();
        }
    }
}

fn read_lsp_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_len = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_len = rest.trim().parse::<usize>().ok();
        }
    }

    let Some(len) = content_len else {
        return Ok(None);
    };
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    Ok(Some(value))
}

fn write_lsp_message<W: Write>(writer: &mut W, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn sanitize_code_action_diagnostics(msg: &Value) -> Value {
    let mut sanitized = msg.clone();
    let Some(diagnostics) = sanitized
        .get_mut("params")
        .and_then(|params| params.get_mut("context"))
        .and_then(|context| context.get_mut("diagnostics"))
        .and_then(Value::as_array_mut)
    else {
        return sanitized;
    };

    for diagnostic in diagnostics {
        let Some(obj) = diagnostic.as_object_mut() else {
            continue;
        };
        let Some(code) = obj.get("code").cloned() else {
            continue;
        };
        if code.is_string() {
            continue;
        }
        let replacement = code
            .as_i64()
            .map(|n| n.to_string())
            .or_else(|| code.as_u64().map(|n| n.to_string()))
            .unwrap_or_else(|| code.to_string());
        obj.insert("code".to_string(), Value::String(replacement));
    }

    sanitized
}

fn merged_capabilities(source_caps: Vec<Value>) -> Value {
    let mut caps = json!({});
    for source in source_caps {
        merge_capability_object(&mut caps, &source);
    }
    let freight = freight_capabilities();
    merge_capability_object(&mut caps, &freight);
    caps
}

fn merge_capability_object(into: &mut Value, from: &Value) {
    let Some(into_obj) = into.as_object_mut() else {
        *into = from.clone();
        return;
    };
    let Some(from_obj) = from.as_object() else {
        return;
    };
    for (key, value) in from_obj {
        if key == "completionProvider" {
            merge_completion_provider(into_obj, value);
        } else if key == "signatureHelpProvider" {
            merge_signature_help_provider(into_obj, value);
        } else if key == "hoverProvider" {
            into_obj.insert(key.clone(), json!(true));
        } else if key == "textDocumentSync" {
            into_obj.insert(key.clone(), value.clone());
        } else {
            into_obj.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

fn merge_completion_provider(into_obj: &mut serde_json::Map<String, Value>, from: &Value) {
    let entry = into_obj
        .entry("completionProvider".to_string())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }

    let Some(entry_obj) = entry.as_object_mut() else {
        return;
    };
    if let Some(from_obj) = from.as_object() {
        for (key, value) in from_obj {
            if key == "triggerCharacters" {
                let mut triggers = entry_obj
                    .get("triggerCharacters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for item in value.as_array().into_iter().flatten() {
                    if !triggers.iter().any(|existing| existing == item) {
                        triggers.push(item.clone());
                    }
                }
                entry_obj.insert(key.clone(), Value::Array(triggers));
            } else {
                entry_obj.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    } else {
        *entry = from.clone();
    }
}

fn merge_signature_help_provider(into_obj: &mut serde_json::Map<String, Value>, from: &Value) {
    let entry = into_obj
        .entry("signatureHelpProvider".to_string())
        .or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }

    let Some(entry_obj) = entry.as_object_mut() else {
        return;
    };
    if let Some(from_obj) = from.as_object() {
        for key in ["triggerCharacters", "retriggerCharacters"] {
            if let Some(value) = from_obj.get(key) {
                let mut chars = entry_obj
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for item in value.as_array().into_iter().flatten() {
                    if !chars.iter().any(|existing| existing == item) {
                        chars.push(item.clone());
                    }
                }
                entry_obj.insert(key.to_string(), Value::Array(chars));
            }
        }
    } else {
        *entry = from.clone();
    }
}

fn freight_capabilities() -> Value {
    json!({
        "positionEncoding": "utf-16",
        "textDocumentSync": {
            "openClose": true,
            "change": 2,
            "save": { "includeText": true }
        },
        "completionProvider": {
            "triggerCharacters": ["[", ".", "=", "\"", " "]
        },
        "signatureHelpProvider": {
            "triggerCharacters": ["{", "=", ","],
            "retriggerCharacters": [","]
        },
        "hoverProvider": true
    })
}

fn manifest_diagnostics(text: &str, dir: &Path, templates: &[CompilerTemplate]) -> Vec<Value> {
    let manifest = match load_manifest_str(text) {
        Ok(manifest) => manifest,
        Err(e) => {
            let (line, character) = parse_line_col(&e.to_string()).unwrap_or((0, 0));
            return vec![diagnostic(
                line,
                character,
                "freight.toml could not be parsed",
                &e.to_string(),
            )];
        }
    };

    let mut errors = validate(&manifest, templates);
    errors.extend(validate_dep_compat(&manifest, dir, templates));
    errors
        .into_iter()
        .map(|e| {
            let line = line_for_context(text, &e.context);
            diagnostic(line, 0, &e.context, &e.message)
        })
        .collect()
}

fn diagnostic(line: usize, character: usize, code: &str, message: &str) -> Value {
    json!({
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character.saturating_add(1) }
        },
        "severity": 1,
        "source": "freight",
        "code": code,
        "message": message
    })
}

fn line_for_context(text: &str, context: &str) -> usize {
    let section = context
        .split_whitespace()
        .next()
        .unwrap_or(context)
        .trim();
    text.lines()
        .position(|line| line.trim() == section)
        .unwrap_or(0)
}

fn completion_result(text: Option<&str>, pos: Option<(usize, usize)>) -> Value {
    let section = text
        .zip(pos)
        .and_then(|(text, (line, _))| current_section(text, line))
        .unwrap_or_default();
    let labels: Vec<(&str, &str, &str)> = if section == "package" {
        vec![
            ("name", "Package name", "Registry and build identity for this package."),
            ("version", "SemVer package version", "Version published to the Freight registry."),
            ("authors", "Package authors", "Array of author names or contacts."),
            ("description", "Short package description", "Shown in registry/package help surfaces."),
            ("license", "SPDX license", "Use an SPDX expression such as MIT or Apache-2.0."),
            ("readme", "README path", "Relative path to package README content."),
            ("repository", "Source repository URL", "Project homepage or source repository."),
            ("supports", "Boolean platform support expression", "Gate the package before build resolution."),
            ("keywords", "Registry search keywords", "Terms used by the package registry."),
            ("provides", "Virtual slots", "Slots such as blas or cxx-stdlib used for conflict checks."),
        ]
    } else if section == "compiler" {
        vec![
            ("backend", "Compiler backend", "auto, gcc, clang, clang++, hipcc, or a custom template name."),
            ("warnings", "Warning level", "none, default, all, or error."),
            ("opt-level", "Optimization level", "Integer optimization level from 0 through 3."),
            ("debug", "Emit debug info", "Boolean debug-symbol toggle."),
            ("defines", "Project-wide preprocessor defines", "Array of defines injected into every compile."),
            ("flags", "Project-wide compiler flags", "Extra flags injected into every compile."),
            ("includes", "Project include directories", "Include directories added to every compile."),
            ("pch", "Precompiled header", "Header path compiled once and injected into supported languages."),
            ("unity", "Unity build toggle", "Combine C-family sources by language for faster full builds."),
        ]
    } else if section.contains("dependencies") {
        vec![
            ("name = \"*\"", "Version dependency", "Resolve an explicitly named package from configured resolvers."),
            ("name = { path = \"../lib\" }", "Local path dependency", "Include one local Freight package by manifest path."),
            ("name = { git = \"https://example/lib.git\" }", "Git dependency", "Fetch and build an explicitly named git package."),
            ("name = { url = \"https://example/lib.tar.gz\", sha256 = \"...\" }", "URL archive dependency", "Fetch and verify an explicitly named source archive."),
            ("name = { version = \"1.0\", repo = \"pkg-config\" }", "Pinned resolver dependency", "Use a specific resolver or registry channel."),
            ("features", "Dependency features", "Activate named features on this dependency."),
            ("default-features", "Default feature toggle", "Disable default dependency features when false."),
            ("optional", "Optional dependency", "Only active when selected by a feature."),
            ("os", "OS filter", "Include this dependency only on matching OS/family keys."),
            ("arch", "Architecture filter", "Include this dependency only on matching CPU architectures."),
            ("targets", "Target triple filter", "Include this prebuilt dependency only for matching triples."),
            ("type", "Foreign build type", "cmake, make, meson, autotools, scons, bazel, or none."),
            ("include", "Exported include dirs", "Include dirs exposed by a foreign dependency."),
            ("cmake-args", "CMake configure args", "Extra args passed to cmake configure."),
            ("patches", "Patch files", "Patch files applied after fetching."),
            ("channel", "Registry channel", "Fetch this dependency from a named channel."),
        ]
    } else if section.starts_with("language.") {
        vec![
            ("std", "Language standard", "Standard such as c17, c++20, f2018, or a compiler-template value."),
            ("stdlib", "C++ standard library selection", "libc++, libstdc++, or none for C++."),
        ]
    } else if section == "lib" {
        vec![
            ("type", "Library type", "static, shared, or header."),
            ("srcs", "Library sources", "Source path or array of source paths for this library target."),
            ("hdrs", "Public headers", "Headers whose parent dirs are exported to dependents."),
            ("link", "Prebuilt link name", "System/prebuilt library name passed to the linker."),
        ]
    } else if section == "bin" {
        vec![
            ("name", "Binary name", "Executable target name."),
            ("src", "Binary entry source", "Entry-point source file for this executable."),
        ]
    } else if section.starts_with("profile.") {
        vec![
            ("inherits", "Parent profile", "Inherit unset values from another named profile."),
            ("opt-level", "Optimization level", "Integer optimization level from 0 through 3."),
            ("debug", "Debug info toggle", "Emit debug information for this profile."),
            ("lto", "Link-time optimization", "Enable or disable LTO."),
            ("strip", "Strip symbols", "Strip final artifacts when true."),
            ("sanitize", "Sanitizers", "Array of sanitizer names for this profile."),
            ("features", "Profile features", "Features activated automatically for this profile."),
        ]
    } else if section == "target" {
        vec![
            ("arch", "CPU architecture", "Override host CPU architecture for target-specific settings."),
            ("cpu-extensions", "CPU extensions", "Array of CPU feature flags such as avx2 or fma."),
        ]
    } else if section == "formatter" || section == "linter" {
        vec![
            ("name", "Tool name", "Pin a formatter/linter instead of auto-detecting."),
            ("style", "Formatter style", "Common formatter setting resolved through the tool template."),
            ("checks", "Linter checks", "Common linter setting resolved through the tool template."),
        ]
    } else if section == "workspace" {
        vec![("members", "Workspace members", "Relative paths to package directories.")]
    } else if section.starts_with("os.") || section.starts_with("arch.") {
        vec![
            ("srcs", "Conditional sources", "Glob patterns included only when this OS/arch is active."),
            ("defines", "Conditional defines", "Defines injected only when this OS/arch is active."),
            ("flags", "Conditional flags", "Compiler flags injected only when this OS/arch is active."),
            ("includes", "Conditional includes", "Include paths injected only when this OS/arch is active."),
            ("dependencies", "Conditional dependencies", "Dependencies included only when this OS/arch is active."),
            ("language", "Conditional language settings", "Language overrides active only for this OS/arch."),
        ]
    } else {
        vec![
            ("[workspace]", "Workspace root", "Declare workspace member package paths."),
            ("[package]", "Package metadata", "Name, version, registry metadata, and package support gates."),
            ("[language.c]", "C language settings", "C standard and template-defined options."),
            ("[language.cpp]", "C++ language settings", "C++ standard, stdlib, and template-defined options."),
            ("[language.fortran]", "Fortran language settings", "Fortran standard and template-defined options."),
            ("[language.asm]", "Assembly language settings", "Assembler template-defined options."),
            ("[language.cuda]", "CUDA language settings", "CUDA standard/options when using CUDA sources."),
            ("[language.hip]", "HIP language settings", "HIP standard/options when using HIP sources."),
            ("[language.objc]", "Objective-C language settings", "Objective-C standard/options."),
            ("[language.objcpp]", "Objective-C++ language settings", "Objective-C++ standard/options."),
            ("[[bin]]", "Binary target", "Executable target entry point."),
            ("[lib]", "Library target", "Library artifact and exported headers."),
            ("[dependencies]", "Runtime dependencies", "Packages explicitly included in this build."),
            ("[build-dependencies]", "Build-time dependencies", "Tools fetched before regular build steps."),
            ("[dev-dependencies]", "Dev dependencies", "Debug/test-only dependencies."),
            ("[compiler]", "Compiler settings", "Backend, warnings, flags, includes, PCH, and unity settings."),
            ("[profile.dev]", "Debug profile", "Debug profile overrides."),
            ("[profile.release]", "Release profile", "Release profile overrides."),
            ("[features]", "Feature graph", "Feature names and dependency feature activation."),
            ("[target]", "CPU target settings", "Architecture and CPU extension settings."),
            ("[formatter]", "Formatter settings", "Project formatter requirements."),
            ("[linter]", "Linter settings", "Project linter requirements."),
            ("[os.linux]", "Linux-only settings", "Sources, defines, includes, deps, and language overrides for Linux."),
            ("[arch.x86_64]", "x86_64-only settings", "Sources, defines, includes, deps, and language overrides for x86_64."),
        ]
    };

    let items: Vec<Value> = labels
        .into_iter()
        .map(|(label, detail, docs)| {
            json!({
                "label": label,
                "kind": 10,
                "detail": detail,
                "documentation": {
                    "kind": "markdown",
                    "value": docs
                },
                "insertText": label
            })
        })
        .collect();
    json!({ "isIncomplete": false, "items": items })
}

fn hover_result(text: Option<&str>, pos: Option<(usize, usize)>) -> Option<Value> {
    let (text, (line, character)) = text.zip(pos)?;
    let section = current_section(text, line).unwrap_or_default();
    let line_text = text.lines().nth(line).unwrap_or("").trim();
    let key = key_at_position(line_text, character);
    let value = if let Some(value) = key.and_then(hover_for_key) {
        value
    } else if section.contains("dependencies") || line_text == "[dependencies]" {
        "Dependencies are explicit in Freight. Headers and link flags are included only when the package is listed in `freight.toml` and active for the current OS, architecture, target, profile, and feature set."
    } else if section == "lib" || line_text == "[lib]" {
        "`[lib]` declares this package's library artifact. Dependents only see headers listed in `hdrs` or discovered include/inc directories from this package."
    } else if line_text == "[[bin]]" || section == "bin" {
        "`[[bin]]` declares an executable entry point. Freight links shared project sources, but avoids linking another target's `main`."
    } else if section.starts_with("language.") {
        "`[language.*]` configures a language that Freight detects from source extensions. Standards are checked against detected compiler templates."
    } else if section.starts_with("profile.") {
        "`[profile.*]` overrides compiler/build settings for one named profile. Custom profiles can inherit from `dev`, `release`, or another custom profile."
    } else if section == "target" {
        "`[target]` controls CPU architecture and extension settings used by compiler templates and assembly output."
    } else if section == "formatter" {
        "`[formatter]` pins project formatting requirements while still allowing tool templates to define supported settings."
    } else if section == "linter" {
        "`[linter]` pins project lint requirements while still allowing tool templates to define supported settings."
    } else if section == "workspace" {
        "`[workspace]` marks this manifest as a workspace root. It contains member package paths and no `[package]` section."
    } else if section.starts_with("os.") || section.starts_with("arch.") {
        "`[os.*]` and `[arch.*]` sections add sources, flags, includes, language overrides, and dependencies only when that platform key is active."
    } else if section == "compiler" {
        "`[compiler]` controls backend selection, warnings, cross-compilation target/sysroot, defines, includes, and extra flags."
    } else if line_text == "[package]" || section == "package" {
        "`[package]` names the package, version, metadata, and optional `supports` expression used before building."
    } else {
        return None;
    };

    Some(json!({
        "contents": {
            "kind": "markdown",
            "value": value
        }
    }))
}

fn key_at_position(line_text: &str, character: usize) -> Option<&str> {
    let before_comment = line_text.split('#').next()?.trim();
    let key = before_comment.split('=').next()?.trim();
    if key.is_empty() || character > before_comment.len().saturating_add(1) {
        return None;
    }
    Some(key.trim_matches('"'))
}

fn hover_for_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "name" => "Name field. In `[package]` this is the package identity; in `[[bin]]` it is the executable target name.",
        "version" => "Version requirement or package SemVer version, depending on context.",
        "path" => "`path` dependencies include exactly the local Freight package at that path. Freight does not scan sibling directories automatically.",
        "git" => "`git` dependencies fetch exactly this repository. Use `branch`, `tag`, or `rev` to control the checked-out ref.",
        "url" => "`url` dependencies fetch exactly this archive. Pair with `sha256` for reproducible fetches.",
        "sha256" => "Expected SHA-256 for a URL archive. Freight rejects the archive when the digest does not match.",
        "repo" => "Resolver override for a version dependency, such as `pkg-config`, `system`, or a named registry.",
        "features" => "Feature list. In a dependency this activates dependency features; in a profile it activates project features for that profile.",
        "default-features" => "Set to `false` to disable a dependency's default feature set.",
        "optional" => "Optional dependencies are available to features but are not included unless selected.",
        "os" => "OS/family filter for a dependency. Supported family keys include `unix`, `bsd`, `linux`, `windows`, and `macos`.",
        "arch" => "CPU architecture filter or target override. Values mirror Rust target architecture names such as `x86_64` and `aarch64`.",
        "targets" => "Target triple allowlist for a dependency, mainly for prebuilts and cross-compilation.",
        "type" => "Artifact or foreign build type depending on context: library `static/shared/header`, or dependency `cmake/make/meson/autotools/scons/bazel/none`.",
        "include" => "Include directories exported by a foreign dependency, relative to that dependency's source root.",
        "includes" => "Include directories added to compiler invocations in the current section.",
        "cmake-args" => "Arguments forwarded to `cmake -S ... -B ...` for this dependency.",
        "patches" => "Patch files applied after fetching a dependency, in order.",
        "channel" => "Registry channel used for this dependency, such as `stable` or `experimental`.",
        "std" => "Language standard checked against the detected compiler template before compilation.",
        "stdlib" => "C++ standard library selection. Supported values are `libc++`, `libstdc++`, and `none`.",
        "srcs" => "Source files or glob patterns, depending on the section.",
        "hdrs" => "Public headers exported by a library target to packages that explicitly depend on it.",
        "link" => "Prebuilt or system library name passed to the linker.",
        "backend" => "Compiler backend selection. `auto` lets Freight choose the first available matching template.",
        "warnings" => "Warning policy: `none`, `default`, `all`, or `error`.",
        "opt-level" => "Optimization level from 0 through 3.",
        "debug" => "Emit debug information when true.",
        "defines" => "Preprocessor defines injected in the current section.",
        "flags" => "Compiler flags injected in the current section.",
        "pch" => "Header path to precompile once and inject into supported language compilations.",
        "unity" => "Enable or disable C-family unity builds.",
        "inherits" => "Parent profile whose unset values are inherited by this profile.",
        "lto" => "Enable or disable link-time optimization.",
        "strip" => "Strip symbols from final artifacts when true.",
        "sanitize" => "Sanitizer names enabled for this profile.",
        "cpu-extensions" => "CPU extensions such as `avx2` or `fma` converted through compiler templates.",
        "members" => "Workspace member directories relative to the workspace root.",
        _ => return None,
    })
}

fn signature_help_result(text: Option<&str>, pos: Option<(usize, usize)>) -> Option<Value> {
    let (text, (line, character)) = text.zip(pos)?;
    let section = current_section(text, line).unwrap_or_default();
    let full_line = text.lines().nth(line).unwrap_or("");
    let line_until_pos = full_line.get(..character).unwrap_or(full_line);
    let spec = signature_spec_for_context(&section, line_until_pos)?;
    let active_parameter = active_parameter_for_signature(line_until_pos, &spec.params);
    Some(signature_help(&spec.label, spec.params, active_parameter, spec.documentation))
}

struct SignatureSpec {
    label: &'static str,
    params: &'static [(&'static str, &'static str)],
    documentation: &'static str,
}

const PACKAGE_PARAMS: &[(&str, &str)] = &[
    ("name", "Package name used by builds, dependencies, and the registry."),
    ("version", "SemVer package version."),
    ("authors", "Package authors."),
    ("description", "Short registry/package description."),
    ("license", "SPDX license expression."),
    ("readme", "Relative README path."),
    ("repository", "Source repository URL."),
    ("keywords", "Registry search keywords."),
    ("supports", "Boolean platform support expression."),
    ("provides", "Virtual slots this package fills."),
];

const DEPENDENCY_PARAMS: &[(&str, &str)] = &[
    ("version", "Version requirement resolved from pkg-config, system stubs, or a registry."),
    ("path", "Explicit local Freight package path."),
    ("git", "Explicit git repository URL."),
    ("branch", "Git branch to check out."),
    ("tag", "Git tag to check out."),
    ("rev", "Pinned git revision."),
    ("url", "Explicit source archive URL."),
    ("sha256", "Expected SHA-256 digest for a URL archive."),
    ("repo", "Resolver override such as pkg-config, system, or a named registry."),
    ("features", "Dependency features to activate."),
    ("default-features", "Whether default dependency features are active."),
    ("optional", "Whether this dependency is only enabled through features."),
    ("os", "OS or OS-family allowlist."),
    ("arch", "CPU architecture allowlist."),
    ("targets", "Target triple allowlist."),
    ("type", "Foreign build type."),
    ("include", "Foreign dependency include dirs exported to dependents."),
    ("cmake-args", "Extra CMake configure arguments."),
    ("patches", "Patch files applied after fetching."),
    ("unity", "Override unity builds for this dependency."),
    ("channel", "Registry channel to use."),
];

const LANGUAGE_PARAMS: &[(&str, &str)] = &[
    ("std", "Language standard checked against the active compiler template."),
    ("stdlib", "C++ standard library selection."),
];

const LIB_PARAMS: &[(&str, &str)] = &[
    ("type", "Library artifact type: static, shared, or header."),
    ("srcs", "Library source file or source list."),
    ("hdrs", "Public headers exported to dependents."),
    ("link", "Prebuilt or system library name passed to the linker."),
];

const BIN_PARAMS: &[(&str, &str)] = &[
    ("name", "Executable target name."),
    ("src", "Executable entry source."),
];

const COMPILER_PARAMS: &[(&str, &str)] = &[
    ("backend", "Compiler backend name or auto."),
    ("opt-level", "Optimization level from 0 through 3."),
    ("debug", "Emit debug information."),
    ("warnings", "Warning policy."),
    ("defines", "Project-wide defines."),
    ("flags", "Project-wide compiler flags."),
    ("includes", "Project-wide include paths."),
    ("pch", "Precompiled header path."),
    ("unity", "Enable C-family unity builds."),
];

const PROFILE_PARAMS: &[(&str, &str)] = &[
    ("inherits", "Parent profile for inherited unset values."),
    ("opt-level", "Optimization level from 0 through 3."),
    ("debug", "Emit debug information."),
    ("lto", "Enable link-time optimization."),
    ("strip", "Strip final artifacts."),
    ("sanitize", "Sanitizers enabled for this profile."),
    ("features", "Features activated by this profile."),
];

const TARGET_PARAMS: &[(&str, &str)] = &[
    ("arch", "Target CPU architecture."),
    ("cpu-extensions", "CPU extensions such as avx2 or fma."),
];

const CONDITIONAL_PARAMS: &[(&str, &str)] = &[
    ("srcs", "Platform-specific source globs."),
    ("defines", "Platform-specific defines."),
    ("flags", "Platform-specific compiler flags."),
    ("includes", "Platform-specific include paths."),
    ("dependencies", "Platform-specific dependency table."),
    ("language", "Platform-specific language overrides."),
];

const WORKSPACE_PARAMS: &[(&str, &str)] = &[("members", "Relative member package paths.")];

const TOOL_PARAMS: &[(&str, &str)] = &[
    ("name", "Tool name to prefer."),
    ("style", "Formatter style setting."),
    ("checks", "Linter checks setting."),
];

fn signature_spec_for_context(section: &str, line_until_pos: &str) -> Option<SignatureSpec> {
    if section.contains("dependencies") || inline_table_key(line_until_pos).is_some() {
        return Some(SignatureSpec {
            label: "freight::dependency { semver version, path path, url git, string branch, string tag, string rev, url url, sha256 sha256, resolver repo, string[] features, bool default-features, bool optional, os[] os, arch[] arch, triple[] targets, build type, path[] include, string[] cmake-args, path[] patches, bool unity, string channel }",
            params: DEPENDENCY_PARAMS,
            documentation: "Freight dependency table. Only explicitly listed, active dependencies contribute headers and link flags.",
        });
    }

    let (label, params, documentation) = if section == "package" {
        (
            "freight::package { string name, semver version, string[] authors, string description, spdx license, path readme, url repository, string[] keywords, expr supports, string[] provides }",
            PACKAGE_PARAMS,
            "Package metadata used by builds and the registry.",
        )
    } else if section.starts_with("language.") {
        (
            "freight::language { standard std, cxx-stdlib stdlib }",
            LANGUAGE_PARAMS,
            "Language settings for the active compiler template.",
        )
    } else if section == "lib" {
        (
            "freight::lib { lib-kind type, path[] srcs, path[] hdrs, string link }",
            LIB_PARAMS,
            "Library target declaration.",
        )
    } else if section == "bin" {
        (
            "freight::bin { string name, path src }",
            BIN_PARAMS,
            "Executable target declaration.",
        )
    } else if section == "compiler" {
        (
            "freight::compiler { backend backend, int opt-level, bool debug, warning-level warnings, string[] defines, string[] flags, path[] includes, path pch, bool unity }",
            COMPILER_PARAMS,
            "Compiler settings applied before profile and platform overlays.",
        )
    } else if section.starts_with("profile.") {
        (
            "freight::profile { string inherits, int opt-level, bool debug, bool lto, bool strip, string[] sanitize, string[] features }",
            PROFILE_PARAMS,
            "Build profile overrides.",
        )
    } else if section == "target" {
        (
            "freight::target { arch arch, string[] cpu-extensions }",
            TARGET_PARAMS,
            "CPU target settings.",
        )
    } else if section.starts_with("os.") || section.starts_with("arch.") {
        (
            "freight::platform { path[] srcs, string[] defines, string[] flags, path[] includes, table dependencies, table language }",
            CONDITIONAL_PARAMS,
            "OS or architecture conditional overlay.",
        )
    } else if section == "workspace" {
        (
            "freight::workspace { path[] members }",
            WORKSPACE_PARAMS,
            "Workspace root manifest.",
        )
    } else if section == "formatter" || section == "linter" {
        (
            "freight::tool { string name, string style, string checks }",
            TOOL_PARAMS,
            "Formatter or linter settings resolved through tool templates.",
        )
    } else {
        return None;
    };

    Some(SignatureSpec {
        label,
        params,
        documentation,
    })
}

fn signature_help(
    label: &str,
    params: &[(&str, &str)],
    active_parameter: usize,
    documentation: &str,
) -> Value {
    let parameters: Vec<Value> = params
        .iter()
        .map(|(name, doc)| {
            let range = parameter_label_range(label, name)
                .map(|(start, end)| json!([start, end]))
                .unwrap_or_else(|| json!(name));
            json!({
                "label": range,
                "documentation": {
                    "kind": "markdown",
                    "value": *doc
                }
            })
        })
        .collect();

    json!({
        "signatures": [{
            "label": label,
            "documentation": {
                "kind": "markdown",
                "value": documentation
            },
            "parameters": parameters,
            "activeParameter": active_parameter.min(params.len().saturating_sub(1))
        }],
        "activeSignature": 0,
        "activeParameter": active_parameter.min(params.len().saturating_sub(1))
    })
}

fn parameter_label_range(label: &str, param: &str) -> Option<(usize, usize)> {
    let start = label.find(param)?;
    Some((start, start + param.len()))
}

fn active_parameter_for_signature(line_until_pos: &str, params: &[(&str, &str)]) -> usize {
    if let Some(key) = inline_table_key(line_until_pos) {
        return params
            .iter()
            .position(|(name, _)| *name == key)
            .unwrap_or_else(|| comma_count_after_open_brace(line_until_pos));
    }
    let key = line_until_pos
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    params
        .iter()
        .position(|(name, _)| *name == key)
        .unwrap_or(0)
}

fn inline_table_key(line_until_pos: &str) -> Option<&str> {
    let open = line_until_pos.rfind('{')?;
    let segment = &line_until_pos[open + 1..];
    let key = segment
        .rsplit(',')
        .next()
        .unwrap_or("")
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn comma_count_after_open_brace(line_until_pos: &str) -> usize {
    let Some(open) = line_until_pos.rfind('{') else {
        return 0;
    };
    line_until_pos[open + 1..]
        .chars()
        .filter(|ch| *ch == ',')
        .count()
}

fn current_section(text: &str, line: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().take(line + 1).collect();
    lines.into_iter().rev().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with("[[") && trimmed.ends_with("]]") {
            return Some(trimmed.trim_matches(['[', ']']).to_string());
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return Some(trimmed.trim_matches(['[', ']']).to_string());
        }
        None
    })
}

fn root_from_initialize(msg: &Value) -> Option<PathBuf> {
    let params = msg.get("params")?;
    params
        .get("rootUri")
        .and_then(Value::as_str)
        .and_then(path_from_uri)
        .or_else(|| params.get("rootPath").and_then(Value::as_str).map(PathBuf::from))
}

fn opened_text(msg: &Value) -> Option<(String, String)> {
    let doc = msg.get("params")?.get("textDocument")?;
    Some((
        doc.get("uri")?.as_str()?.to_string(),
        doc.get("text")?.as_str()?.to_string(),
    ))
}

fn changed_full_text(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("contentChanges")?
        .as_array()?
        .last()?
        .get("text")?
        .as_str()
        .map(ToString::to_string)
}

fn text_document_uri(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(ToString::to_string)
}

fn position(msg: &Value) -> Option<(usize, usize)> {
    let pos = msg.get("params")?.get("position")?;
    Some((
        pos.get("line")?.as_u64()? as usize,
        pos.get("character")?.as_u64()? as usize,
    ))
}

fn is_freight_manifest_uri(uri: &str) -> bool {
    path_from_uri(uri)
        .and_then(|p| p.file_name().map(|n| n == "freight.toml"))
        .unwrap_or(false)
}

fn source_server_for_uri(uri: &str) -> Option<SourceServer> {
    let path = path_from_uri(uri)?;
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "f" | "for" | "f90" | "f95" | "f03" | "f08" => Some(SourceServer::Fortls),
        "asm" | "nasm" | "s" => Some(SourceServer::AsmLsp),
        "c" | "h" | "cc" | "hh" | "cpp" | "hpp" | "cxx" | "hxx" | "c++" | "h++" | "cppm"
        | "ixx" | "mpp" | "cu" | "cuh" | "hip" | "m" | "mm" => Some(SourceServer::Clangd),
        _ => None,
    }
}

fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let a = chars.next()?;
            let b = chars.next()?;
            let hex = format!("{a}{b}");
            let byte = u8::from_str_radix(&hex, 16).ok()?;
            out.push(byte as char);
        } else {
            out.push(ch);
        }
    }
    Some(PathBuf::from(out))
}

fn uri_from_path(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", abs.to_string_lossy())
}

fn parse_line_col(msg: &str) -> Option<(usize, usize)> {
    let line_idx = msg.find("line ")? + "line ".len();
    let rest = &msg[line_idx..];
    let line: usize = rest.split(',').next()?.trim().parse().ok()?;
    let col_idx = msg.find("column ")? + "column ".len();
    let col: usize = msg[col_idx..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((line.saturating_sub(1), col.saturating_sub(1)))
}

fn is_internal_passthrough_response(msg: &Value) -> bool {
    msg.get("id")
        .and_then(Value::as_str)
        .map(|id| id.starts_with(INTERNAL_ID_PREFIX))
        .unwrap_or(false)
}

fn is_internal_client_response(msg: &Value) -> bool {
    msg.get("id")
        .and_then(Value::as_str)
        .map(|id| id.starts_with("__freight_client_"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::sanitize_code_action_diagnostics;
    use serde_json::json;

    #[test]
    fn code_action_diagnostic_codes_are_sanitized_to_strings() {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "textDocument/codeAction",
            "params": {
                "context": {
                    "diagnostics": [
                        { "code": 123, "message": "numeric" },
                        { "code": { "value": "x", "target": "https://example.test" }, "message": "object" },
                        { "code": "already-string", "message": "string" }
                    ]
                }
            }
        });

        let sanitized = sanitize_code_action_diagnostics(&msg);
        let diagnostics = sanitized["params"]["context"]["diagnostics"]
            .as_array()
            .unwrap();
        assert_eq!(diagnostics[0]["code"], json!("123"));
        assert!(diagnostics[1]["code"].is_string());
        assert_eq!(diagnostics[2]["code"], json!("already-string"));
    }
}
