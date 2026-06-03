//! `freight dap` — build and exec into the native DAP adapter.
//!
//! Default mode: build the project, then exec GDB/LLDB with the binary path.
//! The editor connects directly to the adapter — no proxy.
//!
//! `--connect HOST:PORT`: relay a DAP client to an already-running adapter
//! (e.g. `gdb --interpreter=dap --listen=:1234`).

mod connect;
mod server;

#[derive(clap::Args)]
pub struct Args {
    /// Connect to an already-running native DAP server and relay traffic.
    /// Example: `gdb --interpreter=dap --listen=:1234` then
    /// `freight dap --connect localhost:1234`.
    #[arg(long, value_name = "HOST:PORT")]
    pub connect: Option<String>,

    /// Attach to a running process instead of launching (skips build).
    #[arg(long)]
    pub attach: bool,
}

impl Args {
    pub fn run(self) {
        if let Some(ref addr) = self.connect {
            if let Err(e) = connect::connect_relay(addr) {
                eprintln!("freight dap: {e}");
            }
            return;
        }
        let config = serde_json::json!({});
        if let Err(e) = server::launch_dap(&config, self.attach) {
            eprintln!("freight dap: {e}");
        }
    }
}
