//! # Oxide HDL
//!
//! `oxide-hdl` is a high-performance Language Server Protocol (LSP) server for VHDL,
//! written in Rust.
//!
//! ## Architecture
//!
//! It uses a **Hybrid Architecture** to balance speed and accuracy:
//!
//! * **Startup:** Uses a multi-threaded Regex scanner (see [`backend::workspace::index_workspace`])
//!   to index thousands of files in milliseconds.
//! * **Editing:** Uses a mutex-protected Tree-sitter parser (see [`backend::syntax::parser`])
//!   to provide deep, hierarchical analysis of the active file.
//!
//! ## Modules
//!
//! * [`backend`]: The core LSP logic and controller.
//! * [`analysis`]: The data models for Symbols and hierarchies.
//! * [`config`]: Configuration loading (`oxide.toml`).

mod analysis;
mod backend;
mod config;
mod logging;
mod utils;
use tree_sitter::{Language, Parser as TsParser};

use clap::Parser;
use tower_lsp::{LspService, Server};

use crate::backend::Backend;

#[derive(Parser)]
#[command(name = "oxide-hdl", about = "VHDL Language Server")]
struct Cli {
    /// Record a Chrome-format performance trace to /tmp/oxide_trace.json.
    /// Open the file in chrome://tracing or <https://ui.perfetto.dev> after
    /// the server exits to analyse where time is being spent.
    #[arg(long)]
    record_trace: bool,
}

unsafe extern "C" {
    /// External declaration for the tree-sitter-vhdl language function.
    /// This is required to initialize the Tree-sitter parser with the VHDL grammar.
    fn tree_sitter_vhdl() -> Language;
}

/// Configures a global panic hook to capture crash details in a file.
///
/// # Purpose
/// In an LSP environment, standard error (`stderr`) is often captured by the client
/// (e.g., Neovim/VS Code) or used for JSON-RPC transport, making it difficult to
/// see panic messages when the server crashes.
///
/// This hook intercepts panics and writes the panic message and location to
/// `/tmp/oxide_crash.log`, ensuring that crash details are preserved for debugging
/// even if the editor closes the connection immediately.
///
/// # Behavior
/// * Intercepts the panic payload (supporting both `&str` and `String`).
/// * Extracts the file path and line number where the panic occurred.
/// * Overwrites `/tmp/oxide_crash.log` with the crash details.
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

/// The entry point for the Oxide HDL Language Server.
///
/// # Startup Sequence
///
/// 1. **Argument Parsing:** Parses command-line arguments (e.g., `--stdio`, `--version`) using [`clap`].
/// 2. **Logging Initialization:** Sets up the logging infrastructure (e.g., writing to stderr or a file).
/// 3. **Server Instantiation:** Creates an instance of the [`Backend`] struct.
/// 4. **Transport Setup:** Connects the server to the client via Stdin/Stdout.
/// 5. **Event Loop:** Starts the `tower_lsp` event loop to handle JSON-RPC messages.
///
/// # Panics
///
/// This function will panic if:
/// * The logging subsystem fails to initialize.
/// * The binding to Stdin/Stdout fails.
///
/// # Example Usage
///
/// This binary is typically invoked by an editor client (like Neovim or VS Code), not manually.
///
/// ```bash
/// # Standard invocation by a client
/// oxide-hdl --stdio
/// ```
#[tokio::main]
async fn main() {
    setup_panic_hook();
    let args = Cli::parse();

    // Hold the guard for the entire lifetime of the process.  It flushes the
    // trace file to disk on drop, which happens when the LSP server exits.
    let _trace_guard = if args.record_trace {
        let (chrome_layer, guard) = tracing_chrome::ChromeLayerBuilder::new()
            .file("/tmp/oxide_trace.json")
            .build();
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry().with(chrome_layer).init();
        eprintln!("oxide-hdl: tracing enabled → /tmp/oxide_trace.json");
        Some(guard)
    } else {
        None
    };

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut parser = TsParser::new();
    let language = unsafe { tree_sitter_vhdl() };

    parser
        .set_language(&language)
        .expect("Error loading VHDL grammar");

    let (lsp_service, socket) = LspService::new(|client| Backend::new(client, parser));

    Server::new(stdin, stdout, socket).serve(lsp_service).await;
}
