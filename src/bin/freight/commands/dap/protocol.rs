//! DAP framing and I/O helpers.

use std::io::{BufRead, Read, Write};

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
        if trimmed.is_empty() { break; }
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
// stdin reader
// ---------------------------------------------------------------------------

pub fn spawn_dap_reader() -> std::sync::mpsc::Receiver<Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = std::io::BufReader::new(stdin.lock());
        loop {
            match read_dap_frame(&mut reader) {
                Some(body) => {
                    if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                        if tx.send(value).is_err() { return; }
                    }
                }
                None => return,
            }
        }
    });
    rx
}
