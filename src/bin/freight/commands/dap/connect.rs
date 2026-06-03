//! `freight dap --connect HOST:PORT` — relay a DAP client to an already-running
//! native DAP server (e.g. `gdb --interpreter=dap --listen=:1234`).
//!
//! This is a thin byte-level relay; no parsing or injection is needed because
//! the native adapter handles the full protocol itself.

use std::net::TcpStream;

pub fn connect_relay(addr: &str) -> anyhow::Result<()> {
    let stream = TcpStream::connect(addr)
        .map_err(|e| anyhow::anyhow!("failed to connect to DAP server at {addr}: {e}"))?;
    let stream_to_stdout = stream.try_clone()?;

    // TCP → stdout (background thread).
    std::thread::spawn(move || {
        let mut src = stream_to_stdout;
        let mut dst = std::io::stdout();
        let _ = std::io::copy(&mut src, &mut dst);
    });

    // stdin → TCP (main thread, blocks until connection closes).
    let mut src = std::io::stdin();
    let mut dst = stream;
    std::io::copy(&mut src, &mut dst)?;
    Ok(())
}
