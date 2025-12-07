mod analysis;
mod backend;
use tree_sitter::{Language, Parser};

use tower_lsp::{LspService, Server};

use crate::backend::Backend;

unsafe extern "C" {
    fn tree_sitter_vhdl() -> Language;
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut parser = Parser::new();
    let language = unsafe { tree_sitter_vhdl() };

    parser
        .set_language(&language)
        .expect("Error loading VHDL grammar");

    let (lsp_service, socket) = LspService::new(|client| Backend::new(client, parser));

    Server::new(stdin, stdout, socket).serve(lsp_service).await;
}
