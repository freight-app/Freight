//! Language Server for `freight.toml`.
//!
//! Speaks LSP 3.17 over stdio via `tower-lsp`. The backend leans on `freight-core`
//! for manifest parsing + validation; mapping errors back to source positions
//! happens in `position.rs` (there's no span info in the parsed AST).

pub mod backend;
pub mod completion;
pub mod docs;
pub mod position;

use tower_lsp::{LspService, Server};

/// Run the language server on stdio. Call this from the `freight lsp` subcommand
/// or the `freight-lsp` binary.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(backend::Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
