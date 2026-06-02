//! Pure DAP framing and GDB/MI2 protocol helpers — no DapServer state.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

// ---------------------------------------------------------------------------
// DAP framing
// ---------------------------------------------------------------------------

pub fn write_dap(out: &mut impl Write, msg: &Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    write!(out, "Content-Length: {}\r\n\r\n", bytes.len())?;
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

pub fn read_dap_frame(reader: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok().filter(|n| *n > 0).is_none() {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

// ---------------------------------------------------------------------------
// GDB/MI2 parsers
// ---------------------------------------------------------------------------

pub fn mi_parse_list_value(s: &str, key: &str) -> Vec<HashMap<String, String>> {
    let mut result = Vec::new();
    let needle_brace = format!("{key}={{");
    let needle_quote = format!("{key}=\"");

    let mut search = s;
    while let Some(pos) = search.find(&needle_brace) {
        let after = &search[pos + needle_brace.len()..];
        let (record, consumed) = parse_mi_record(after);
        result.push(record);
        search = &search[pos + needle_brace.len() + consumed..];
    }

    if result.is_empty() {
        let mut search2 = s;
        while let Some(pos) = search2.find(&needle_quote) {
            let after = &search2[pos + needle_quote.len()..];
            if let Some(end) = find_string_end(after) {
                let val = mi_c_string(&format!("\"{}\"", &after[..end]));
                let mut map = HashMap::new();
                map.insert(key.to_string(), val);
                result.push(map);
                search2 = &search2[pos + needle_quote.len() + end + 1..];
            } else {
                break;
            }
        }
    }
    result
}

pub fn parse_mi_record(s: &str) -> (HashMap<String, String>, usize) {
    let mut map = HashMap::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'}' {
            i += 1;
            break;
        }
        let eq = match s[i..].find('=') {
            Some(p) => i + p,
            None => break,
        };
        let field_name = s[i..eq].trim().to_string();
        i = eq + 1;
        if i >= bytes.len() {
            break;
        }
        let value = if bytes[i] == b'"' {
            i += 1;
            let end = find_string_end(&s[i..]).unwrap_or(0);
            let val = mi_c_string(&format!("\"{}\"", &s[i..i + end]));
            i += end + 1;
            val
        } else if bytes[i] == b'{' {
            let depth_start = i;
            i += 1;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    b'"' => {
                        i += 1;
                        let end = find_string_end(&s[i..]).unwrap_or(0);
                        i += end + 1;
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
            s[depth_start..i].to_string()
        } else {
            let end = s[i..].find(|c| c == ',' || c == '}').unwrap_or(s[i..].len());
            let val = s[i..i + end].to_string();
            i += end;
            val
        };
        if !field_name.is_empty() {
            map.insert(field_name, value);
        }
        if i < bytes.len() && bytes[i] == b',' {
            i += 1;
        }
    }
    (map, i)
}

pub fn find_string_end(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if escaped { escaped = false; continue; }
        if ch == '\\' { escaped = true; continue; }
        if ch == '"' { return Some(idx); }
    }
    None
}

pub fn parse_mi_result(line: &str) -> Option<(u64, &str)> {
    let caret = line.find('^')?;
    let token = line[..caret].parse().ok()?;
    let rest = &line[caret + 1..];
    let end = rest.find(',').unwrap_or(rest.len());
    Some((token, &rest[..end]))
}

pub fn mi_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("{field}=\"");
    let start = text.find(&needle)? + needle.len();
    let end = find_string_end(&text[start..])?;
    Some(mi_c_string(&format!("\"{}\"", &text[start..start + end])))
}

pub fn mi_c_string(value: &str) -> String {
    serde_json::from_str::<String>(value)
        .unwrap_or_else(|_| value.trim_matches('"').to_string())
}

pub fn mi_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

// ---------------------------------------------------------------------------
// Output parsers
// ---------------------------------------------------------------------------

pub fn parse_raw_variables(line: &str) -> Vec<(String, String, String)> {
    mi_parse_list_value(line, "variable")
        .into_iter()
        .filter_map(|var| {
            let name = var.get("name")?.clone();
            let value = var.get("value").cloned()
                .unwrap_or_else(|| "<unavailable>".to_string());
            let type_ = var.get("type").cloned().unwrap_or_default();
            Some((name, value, type_))
        })
        .collect()
}

pub fn parse_stack_frames(line: &str) -> Vec<serde_json::Value> {
    use serde_json::json;
    mi_parse_list_value(line, "frame")
        .into_iter()
        .enumerate()
        .map(|(idx, frame)| {
            let file = frame.get("fullname").or_else(|| frame.get("file")).cloned();
            json!({
                "id": idx + 1,
                "name": frame.get("func").cloned()
                    .unwrap_or_else(|| "<unknown>".to_string()),
                "source": file.as_ref().map(|f| json!({
                    "name": path_base_name(f),
                    "path": f
                })),
                "line": frame.get("line")
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0),
                "column": 1,
            })
        })
        .collect()
}

pub fn stopped_reason(reason: &str) -> &'static str {
    if reason.contains("breakpoint") || reason.contains("watchpoint") {
        "breakpoint"
    } else if reason.contains("end-stepping") || reason.contains("function-finished") {
        "step"
    } else if reason.contains("signal") {
        "exception"
    } else {
        "pause"
    }
}

pub fn path_base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

// ---------------------------------------------------------------------------
// Debugger version probe
// ---------------------------------------------------------------------------

pub fn gdb_version(path: &Path) -> (u32, u32) {
    let out = Command::new(path).arg("--version").output().ok();
    let text = out
        .as_ref()
        .and_then(|o| std::str::from_utf8(&o.stdout).ok().map(str::to_string))
        .unwrap_or_default();
    let first_line = text.lines().next().unwrap_or("");
    for token in first_line.split_whitespace().rev() {
        let mut parts = token.split('.');
        if let (Some(maj), Some(min)) = (parts.next(), parts.next()) {
            if let (Ok(major), Ok(minor)) = (maj.parse::<u32>(), min.parse::<u32>()) {
                return (major, minor);
            }
        }
    }
    (0, 0)
}

// ---------------------------------------------------------------------------
// I/O helpers
// ---------------------------------------------------------------------------

pub fn spawn_dap_reader() -> std::sync::mpsc::Receiver<serde_json::Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = std::io::BufReader::new(stdin.lock());
        loop {
            match read_dap_frame(&mut reader) {
                Some(body) => {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) {
                        if tx.send(value).is_err() { return; }
                    }
                }
                None => return,
            }
        }
    });
    rx
}

pub fn spawn_gdb_reader(stdout: impl Read + Send + 'static)
    -> std::sync::mpsc::Receiver<String>
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
            if !line.trim().is_empty() && tx.send(line.trim().to_string()).is_err() {
                break;
            }
        }
    });
    rx
}

pub fn spawn_process_output(
    stdout: impl Read + Send + 'static,
    category: &'static str,
    tx: std::sync::mpsc::Sender<(String, String)>,
) {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
            if tx.send((category.to_string(), format!("{line}\n"))).is_err() {
                break;
            }
        }
    });
}
