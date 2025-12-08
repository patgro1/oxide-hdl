mod analysis;
mod backend;
mod logging;
use tree_sitter::{Language, Parser};

use tower_lsp::{LspService, Server};

use crate::backend::Backend;

unsafe extern "C" {
    fn tree_sitter_vhdl() -> Language;
}

pub fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let path = "/tmp/oxide_crash.log";
        let msg = match panic_info.payload().downcast_ref::<&str>() {
            Some(s) => *s,
            None => match panic_info.payload().downcast_ref::<String>() {
                Some(s) => &**s,
                None => "Box<Any>",
            },
        };
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let _ = std::fs::write(path, format!("CRASH: {} at {}\n", msg, location));
    }));
}

#[tokio::main]
async fn main() {
    setup_panic_hook();
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
