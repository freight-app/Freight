//! LSP framing, capability merging, message helpers, and URI utilities.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::SourceServer;
use super::INTERNAL_ID_PREFIX;

// ---------------------------------------------------------------------------
// LSP framing
// ---------------------------------------------------------------------------

pub fn read_lsp_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
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

pub fn write_lsp_message<W: Write>(writer: &mut W, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

// ---------------------------------------------------------------------------
// Code action sanitization
// ---------------------------------------------------------------------------

pub fn sanitize_code_action_diagnostics(msg: &Value) -> Value {
    let mut sanitized = msg.clone();
    let Some(diagnostics) = sanitized
        .get_mut("params")
        .and_then(|p| p.get_mut("context"))
        .and_then(|c| c.get_mut("diagnostics"))
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

// ---------------------------------------------------------------------------
// Capability merging
// ---------------------------------------------------------------------------

pub fn merged_capabilities(source_caps: Vec<Value>, use_clang_bridge: bool) -> Value {
    let mut caps = json!({});
    for source in source_caps {
        merge_capability_object(&mut caps, &source);
    }
    merge_capability_object(&mut caps, &freight_capabilities(use_clang_bridge));
    caps
}

pub fn merge_capability_object(into: &mut Value, from: &Value) {
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
        } else if key == "hoverProvider"
            || key == "definitionProvider"
            || key == "declarationProvider"
            || key == "documentLinkProvider"
            || key == "inlayHintProvider"
            // freight owns these via clang-bridge; its provider (and, for
            // semantic tokens, its legend) must win over a forwarded server's.
            || key == "documentSymbolProvider"
            || key == "foldingRangeProvider"
            || key == "referencesProvider"
            || key == "documentHighlightProvider"
            || key == "semanticTokensProvider"
        {
            into_obj.insert(key.clone(), value.clone());
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
                    if !triggers.iter().any(|e| e == item) {
                        triggers.push(item.clone());
                    }
                }
                entry_obj.insert(key.clone(), Value::Array(triggers));
            } else {
                entry_obj
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
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
                    if !chars.iter().any(|e| e == item) {
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

pub fn freight_capabilities(use_clang_bridge: bool) -> Value {
    let mut caps = json!({
        "positionEncoding": "utf-16",
        "textDocumentSync": {
            "openClose": true,
            "change": 1,
            "save": { "includeText": true }
        },
        "completionProvider": {
            "triggerCharacters": ["[", ".", "=", "\"", " "]
        },
        "signatureHelpProvider": {
            "triggerCharacters": ["{", "=", ","],
            "retriggerCharacters": [","]
        },
        "hoverProvider": true,
        "inlayHintProvider": true,
        "definitionProvider": true,
        "declarationProvider": true,
        "documentLinkProvider": { "resolveProvider": false }
    });

    // The clang-bridge-owned C/C++ providers are only advertised by freight when
    // the bridge is enabled. When it is off these requests forward to clangd, so
    // clangd's own capabilities (notably the semantic-token *legend*, which must
    // match the token indices the responder emits) are the ones the merge keeps.
    if use_clang_bridge {
        if let Some(obj) = caps.as_object_mut() {
            obj.insert("documentSymbolProvider".into(), json!(true));
            obj.insert("foldingRangeProvider".into(), json!(true));
            obj.insert("referencesProvider".into(), json!(true));
            obj.insert("documentHighlightProvider".into(), json!(true));
            obj.insert(
                "semanticTokensProvider".into(),
                json!({ "legend": super::index::semantic_tokens_legend(), "full": true }),
            );
        }
    }
    caps
}

// ---------------------------------------------------------------------------
// Message field extractors
// ---------------------------------------------------------------------------

pub fn root_from_initialize(msg: &Value) -> Option<PathBuf> {
    let params = msg.get("params")?;
    params
        .get("rootUri")
        .and_then(Value::as_str)
        .and_then(path_from_uri)
        .or_else(|| {
            params
                .get("rootPath")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
}

pub fn opened_text(msg: &Value) -> Option<(String, String)> {
    let doc = msg.get("params")?.get("textDocument")?;
    Some((
        doc.get("uri")?.as_str()?.to_string(),
        doc.get("text")?.as_str()?.to_string(),
    ))
}

pub fn changed_full_text(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("contentChanges")?
        .as_array()?
        .last()?
        .get("text")?
        .as_str()
        .map(ToString::to_string)
}

pub fn text_document_uri(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(ToString::to_string)
}

pub fn position(msg: &Value) -> Option<(usize, usize)> {
    let pos = msg.get("params")?.get("position")?;
    Some((
        pos.get("line")?.as_u64()? as usize,
        pos.get("character")?.as_u64()? as usize,
    ))
}

// ---------------------------------------------------------------------------
// URI / path helpers
// ---------------------------------------------------------------------------

pub fn is_freight_manifest_uri(uri: &str) -> bool {
    path_from_uri(uri)
        .and_then(|p| p.file_name().map(|n| n == "freight.toml"))
        .unwrap_or(false)
}

pub fn source_server_for_uri(uri: &str) -> Option<SourceServer> {
    let path = path_from_uri(uri)?;
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        // Fortran: free-form and fixed-form, with and without preprocessor suffix.
        "f" | "for" | "ftn" | "f90" | "f95" | "f03" | "f08" | "f18" | "f77" | "f66" => {
            Some(SourceServer::Fortls)
        }
        // Assembly: GAS (.s/.S), NASM (.asm/.nasm), Intel HEX (.asm).
        "asm" | "nasm" | "s" => Some(SourceServer::AsmLsp),
        // C-family: C, C++, CUDA, HIP, Objective-C — all handled by clangd.
        "c" | "h" | "cc" | "hh" | "cpp" | "hpp" | "cxx" | "hxx" | "c++" | "h++" | "cppm"
        | "ixx" | "mpp" | "cu" | "cuh" | "hip" | "m" | "mm" | "cl" | "ispc" => {
            Some(SourceServer::Clangd)
        }
        _ => None,
    }
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let a = chars.next()?;
            let b = chars.next()?;
            let byte = u8::from_str_radix(&format!("{a}{b}"), 16).ok()?;
            out.push(byte as char);
        } else {
            out.push(ch);
        }
    }
    Some(PathBuf::from(out))
}

pub fn uri_from_path(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", abs.to_string_lossy())
}

pub fn parse_line_col(msg: &str) -> Option<(usize, usize)> {
    let line_idx = msg.find("line ")? + "line ".len();
    let line: usize = msg[line_idx..].split(',').next()?.trim().parse().ok()?;
    let col_idx = msg.find("column ")? + "column ".len();
    let col: usize = msg[col_idx..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((line.saturating_sub(1), col.saturating_sub(1)))
}

pub fn is_internal_passthrough_response(msg: &Value) -> bool {
    msg.get("id")
        .and_then(Value::as_str)
        .map(|id| id.starts_with(INTERNAL_ID_PREFIX))
        .unwrap_or(false)
}

pub fn is_internal_client_response(msg: &Value) -> bool {
    msg.get("id")
        .and_then(Value::as_str)
        .map(|id| id.starts_with("__freight_client_"))
        .unwrap_or(false)
}
