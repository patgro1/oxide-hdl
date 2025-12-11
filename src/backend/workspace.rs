// src/backend/workspace.rs

use crate::analysis::Analysis;
use crate::backend::AnalysisMap;
use crate::backend::syntax::{parser, scanner};
use crate::config::OxideConfig;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tower_lsp::Client;
use tower_lsp::lsp_types::{MessageType, Url};
use walkdir::WalkDir;

/// Scans the entire workspace using a fast, multi-threaded Regex scanner.
///
/// # The Hybrid Architecture (Phase 1)
/// This function represents the "Cold Start" phase of indexing. Instead of running the heavy
/// Tree-sitter parser on every file (which would take ~60s and require a global mutex),
/// this function uses **Regex** to find top-level symbols (Entities, Packages) in milliseconds.
///
/// * **Speed:** ~100ms for 3,000 files.
/// * **Concurrency:** 16 Threads (Safe because Regex is pure Rust).
/// * **Safety:** It implements **Overwrite Protection**. If a file is already open
///   in the editor (and thus has a deep Tree-sitter parse), this scanner skips it
///   to avoid overwriting rich data with shallow data.
///
/// # Arguments
///
/// * `client` - The LSP client handle for logging progress.
/// * `analysis_map` - The global symbol table to populate.
/// * `root_uri` - The workspace root directory.
/// * `config` - Configuration for file extensions and ignore patterns.
pub async fn index_workspace(
    client: Client,
    analysis_map: Arc<RwLock<HashMap<Url, Analysis>>>,
    root_uri: Url,
    config: OxideConfig,
) {
    let root_path = match root_uri.to_file_path() {
        Ok(p) => p,
        Err(_) => return,
    };

    let start = Instant::now();
    client
        .log_message(MessageType::INFO, "Starting indexing...")
        .await;

    let max_concurrency = 16;
    let semaphone = Arc::new(Semaphore::new(max_concurrency));
    let matcher = config.build_globset();

    let paths: Vec<std::path::PathBuf> = WalkDir::new(&root_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            // Filter 1 : extension
            if let Some(ext) = e.path().extension() {
                let ext_str = ext.to_string_lossy().to_string();
                if !config.extensions.contains(&ext_str) {
                    return false;
                }
            } else {
                return false;
            }

            // Filter 2: Ignore list
            if let Ok(relative) = e.path().strip_prefix(&root_path)
                && matcher.is_match(relative)
            {
                return false;
            }

            true
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    let mut handles = Vec::new();

    for path in paths {
        let path_uri = match Url::from_file_path(&path) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let sem_clone = semaphone.clone();
        let handle = tokio::task::spawn(async move {
            let _permit = sem_clone.acquire_owned().await.unwrap();
            tokio::task::spawn_blocking(move || {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let symbols = scanner::scan_fast(&text);
                let mut analysis = Analysis::new();
                for s in symbols {
                    analysis.symbols.insert(s.name.clone().to_lowercase(), s);
                }
                (path_uri, analysis)
            })
            .await
            .unwrap()
        });
        handles.push(handle);
    }
    for handle in handles {
        if let Ok((uri, analysis)) = handle.await {
            let mut map = analysis_map.write().await;
            // NOTE:We need to check if better data is available to prevent
            // a race condition where we open a file, it gets parse and then
            // the fast indexer reaches that file and overwrite good data
            // with shallow data.
            if let Some(exisiting_analysis) = map.get(&uri) {
                let existing_is_deep = exisiting_analysis
                    .symbols
                    .values()
                    .any(|s| !s.children.is_empty());
                if existing_is_deep {
                    continue;
                }
            }
            map.insert(uri, analysis);
        }
    }
    let duration = start.elapsed();
    client
        .log_message(
            MessageType::INFO,
            format!("Inxedx workspace in {:?}", duration),
        )
        .await;
}

/// Parses a single document using the full Tree-sitter grammar and updates the global map.
///
/// # The Deep Parse (Phase 2)
/// This function is called when a file is opened or edited. It performs a full, recursive
/// AST walk to extract detailed structure (Ports, Signals, Nested Processes).
///
/// # Concurrency Strategy
/// * Uses `spawn_blocking` to offload CPU-intensive work.
/// * Acquires a **Global Mutex** on the `tree_sitter::Parser` because the underlying
///   C-library is not thread-safe.
/// * Uses a 128MB stack to handle deep recursion in complex VHDL files.
///
/// # Arguments
///
/// * `analysis_map` - The global symbol table to update with the new data.
/// * `parser` - The shared, mutex-protected Tree-sitter parser instance.
/// * `uri` - The URI of the file being parsed.
/// * `text` - The full content of the file as an owned String.
pub async fn parse_and_update_document(
    analysis_map: Arc<RwLock<AnalysisMap>>,
    parser: Arc<Mutex<crate::backend::Parser>>,
    uri: &Url,
    text: String,
) {
    let uri = uri.clone();
    tokio::task::spawn_blocking(move || {
        let builder = std::thread::Builder::new().stack_size(128 * 1024 * 1024);
        let thread_result = builder
            .spawn(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let tree = {
                        let mut parser = parser.blocking_lock();
                        let language = unsafe { crate::tree_sitter_vhdl() };
                        let _ = parser.set_language(&language);
                        parser.parse(&text, None)
                    };
                    match tree {
                        Some(t) => {
                            let analysis = parser::extract_document_symbols(&text, t.root_node());

                            // TODO: Add diagnostics
                            let diagnostics: Vec<u8> = vec![];

                            Some(Box::new((analysis, diagnostics)))
                        }
                        None => None,
                    }
                }))
            })
            .unwrap()
            .join();
        if let Ok(Ok(Some(boxed_result))) = thread_result {
            let (analysis, _) = *boxed_result;
            tokio::spawn(async move {
                let mut map = analysis_map.write().await;
                map.insert(uri.clone(), analysis);
            });
        }
    })
    .await
    .unwrap();
}

/// Checks if a file has only been Shallow-Indexed and performs a JIT upgrade if needed.
///
/// # Just-In-Time (JIT) Parsing
/// When `index_workspace` runs, it only captures top-level names using Regex. It does not
/// capture Ports, Generics, or Function Signatures to save time (~100ms startup).
///
/// When a user hovers over a symbol defined in another file (e.g., `u_tx : uart_tx`),
/// the LSP checks that target file. If it finds it is "Shallow" (symbols have no children),
/// it calls this function to parse it *immediately* using Tree-sitter to retrieve the
/// rich interface details.
///
/// # Concurrency & Safety
/// * **Blocking:** Spawns a `tokio::task::spawn_blocking` thread to handle the CPU-heavy parsing.
/// * **Locking:** Acquires the Global Parser Mutex to prevent C-Library race conditions.
/// * **Integrity:** Includes a "Do No Harm" check—if the JIT parse returns 0 symbols (failure),
///   it aborts the update to prevent overwriting the valid Regex index with empty data.
///
/// # Arguments
///
/// * `client` - The LSP client handle used to log progress and errors.
/// * `analysis_map` - The global symbol table to check and update.
/// * `parser` - The shared, mutex-protected Tree-sitter parser instance.
/// * `uri` - The URI of the file that needs to be checked and potentially upgraded.
pub async fn ensure_fully_parsed(
    client: &Client,
    analysis_map: &Arc<RwLock<AnalysisMap>>,
    parser: &Arc<Mutex<crate::backend::Parser>>,
    uri: &Url,
) {
    // Check if the file was shallow index or fully parsed
    let needs_parsing = {
        let map = analysis_map.read().await;
        if let Some(analysis) = map.get(uri) {
            // NOTE: They heuristic used to decide on the shallow parse is that
            // no symbol has any children
            analysis.symbols.values().all(|s| s.children.is_empty())
        } else {
            true
        }
    };

    if !needs_parsing {
        return;
    }

    // Now we force a parse on the current file

    let path = match uri.to_file_path() {
        Ok(t) => t,
        Err(_) => return,
    };

    let parser_arc = parser.clone();
    let result = tokio::task::spawn_blocking(move || {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return None,
        };

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let tree = {
                let mut parser = parser_arc.blocking_lock();
                let language = unsafe { crate::tree_sitter_vhdl() };
                let _ = parser.set_language(&language);
                parser.parse(&text, None)
            };
            tree.map(|t| parser::extract_document_symbols(&text, t.root_node()))
        }))
        .unwrap_or(None)
    })
    .await
    .unwrap();
    if let Some(analysis) = result {
        if analysis.symbols.is_empty() {
            client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "JIT Parse returned 0 symbols for {}. Keeping existing index",
                        uri
                    ),
                )
                .await;
            return;
        }
        client
            .log_message(
                MessageType::INFO,
                format!(
                    "JIT Parse returned {} symbols for {}. Updating index.",
                    analysis.symbols.len(),
                    uri
                ),
            )
            .await;
        let mut map = analysis_map.write().await;
        map.insert(uri.clone(), analysis);
    }
}
