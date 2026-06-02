//! `freight dap` — Debug Adapter Protocol server.
//!
//! Default mode: act as a DAP adapter over stdin/stdout, building the project
//! and driving GDB or LLDB via MI2 (or native DAP passthrough for GDB≥14 /
//! lldb-dap).
//!
//! `--connect HOST:PORT`: relay VS Code directly to an already-running native
//! DAP server (e.g. `gdb --interpreter=dap --listen=:1234`).

mod connect;
mod protocol;
mod server;

pub use server::DapServer;

#[derive(clap::Args)]
pub struct Args {
    /// Connect to an already-running native DAP server and relay traffic.
    /// Example: `gdb --interpreter=dap --listen=:1234` then
    /// `freight dap --connect localhost:1234`.
    #[arg(long, value_name = "HOST:PORT")]
    pub connect: Option<String>,
}

impl Args {
    pub fn run(self) {
        if let Some(ref addr) = self.connect {
            if let Err(e) = connect::connect_relay(addr) {
                eprintln!("freight dap: {e}");
            }
            return;
        }
        if let Err(e) = DapServer::new().run() {
            eprintln!("freight dap: {e}");
        }
    }
}
