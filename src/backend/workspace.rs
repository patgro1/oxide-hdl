// src/backend/workspace.rs

use crate::analysis::{Analysis, OxideSymbolKind, ParseLevel};
use crate::backend::AnalysisMap;
use crate::backend::syntax::{parser, scanner};
use crate::config::OxideConfig;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tower_lsp::Client;
use tower_lsp::lsp_types::Diagnostic;
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

    let matcher = config.build_globset();

    let is_ignored = |entry: &walkdir::DirEntry| -> bool {
        let name = entry.file_name().to_string_lossy();

        // A. HARDCODED SAFETY NET (The Guard Rails)
        // These are checked via string comparison which is faster than Glob matching.
        // It prevents the LSP from crashing even if config is broken/empty.
        if name == ".git" || name == "target" {
            return true;
        }

        // B. USER CONFIG (The Flexibility)
        // Check relative path against oxide.toml patterns
        if let Ok(relative) = entry.path().strip_prefix(&root_path) {
            return matcher.is_match(relative);
        }
        false
    };

    let paths: Vec<std::path::PathBuf> = WalkDir::new(&root_path)
        .into_iter()
        .filter_entry(|e| !is_ignored(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            if let Some(ext) = e.path().extension() {
                let ext_str = ext.to_string_lossy().to_string();
                if !config.extensions.contains(&ext_str) {
                    return false;
                }
            } else {
                return false;
            }
            true
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    client
        .log_message(
            MessageType::WARNING,
            format!("Found {} VHDL files to index", paths.len()),
        )
        .await;

    let max_concurrency = 16;
    let semaphore = Arc::new(Semaphore::new(max_concurrency));
    let mut join_set = JoinSet::new();

    for path in paths {
        let sem_clone = semaphore.clone();
        let path_uri = match Url::from_file_path(&path) {
            Ok(u) => u,
            Err(_) => continue,
        };
        join_set.spawn(async move {
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
    }

    let mut batch = Vec::with_capacity(50);
    while let Some(res) = join_set.join_next().await {
        if let Ok((uri, analysis)) = res {
            batch.push((uri, analysis));
            if batch.len() >= 50 {
                insert_batch(&analysis_map, &mut batch).await;
            }
        }
    }

    if !batch.is_empty() {
        insert_batch(&analysis_map, &mut batch).await;
    }
    let duration = start.elapsed();
    client
        .log_message(
            MessageType::WARNING,
            format!("Inxedx workspace in {:?}", duration),
        )
        .await;
}

/// Inserts a batch of analysis results into the global map with overwrite protection.
///
/// This function implements a "Do No Harm" policy: if a file already has deep analysis
/// data (symbols with children), the shallow regex-based analysis will not overwrite it.
/// This prevents race conditions where the fast indexer might overwrite rich data from
/// an open file.
///
/// # Arguments
/// * `map_lock` - The shared analysis map to insert into.
/// * `batch` - Mutable vector of (URI, Analysis) pairs to insert; drained after insertion.
async fn insert_batch(
    map_lock: &Arc<RwLock<HashMap<Url, Analysis>>>,
    batch: &mut Vec<(Url, Analysis)>,
) {
    let mut map = map_lock.write().await;
    for (uri, analysis) in batch.drain(..) {
        if let Some(existing) = map.get(&uri) {
            let existing_is_deep = existing.symbols.values().any(|s| !s.children.is_empty());
            if existing_is_deep {
                continue;
            }
        }
        map.insert(uri, analysis);
    }
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
/// * `client` - The LSP client handle used to log progress and errors.
/// * `analysis_map` - The global symbol table to update with the new data.
/// * `parser` - The shared, mutex-protected Tree-sitter parser instance.
/// * `uri` - The URI of the file being parsed.
/// * `text` - The full content of the file as an owned String.
/// * `get_diagnostics` - If true, collect diagnostics
///
/// # Returns
/// A Vector containing all collected diagnostics.
pub async fn parse_and_update_document(
    client: &Client,
    analysis_map: Arc<RwLock<AnalysisMap>>,
    parser: Arc<Mutex<crate::backend::Parser>>,
    uri: &Url,
    text: String,
    get_diagnostics: bool,
) -> Vec<Diagnostic> {
    let uri = uri.clone();
    let text_for_diag = text.clone();
    let parser_for_diag = parser.clone();

    // Phase 1: Parse current file, extract analysis and needed packages
    let phase1_result = {
        let parser = parser.clone();
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
                                let root = t.root_node();
                                let analysis = parser::extract_document_symbols(&text, root);
                                let needed_packages: Vec<String> = analysis
                                    .use_clauses
                                    .iter()
                                    .map(|u| u.name.clone())
                                    .collect();
                                Some((analysis, needed_packages))
                            }
                            None => None,
                        }
                    }))
                })
                .unwrap()
                .join();
            match thread_result {
                Ok(Ok(Some(result))) => Some(result),
                _ => None,
            }
        })
        .await
        .unwrap()
    };

    let (analysis, _needed_packages) = match phase1_result {
        Some(r) => r,
        None => return Vec::new(),
    };

    // Phase 2: JIT parse any missing packages
    for clause in &analysis.use_clauses {
        let package_name = &clause.name;
        let library = &clause.library;
        let mut pkg_uri = {
            let map = analysis_map.read().await;
            find_package_file(package_name, &map)
        };
        if pkg_uri.is_none() {
            pkg_uri = crate::backend::features::lookup::resolve_import_uri(library, package_name);
        }

        if let Some(pkg_uri) = pkg_uri {
            ensure_fully_parsed(client, &analysis_map, &parser, &pkg_uri).await;
        }
    }
    // for pkg_name in &needed_packages {
    //     let pkg_uri = {
    //         let map = analysis_map.read().await;
    //         find_package_file(pkg_name, &map)
    //     };
    //     if let Some(pkg_uri) = pkg_uri {
    //         ensure_fully_parsed(client, &analysis_map, &parser_for_diag, &pkg_uri).await;
    //     }
    // }

    // Phase 3: Run diagnostics (now with access to imported packages)
    let diagnostics = {
        if get_diagnostics {
            let analysis_for_diag = analysis.clone();
            let map_ref = analysis_map.clone();
            let uri = uri.clone();
            tokio::task::spawn_blocking(move || {
                let builder = std::thread::Builder::new().stack_size(128 * 1024 * 1024);
                let thread_result = builder
                    .spawn(move || {
                        let map = map_ref.blocking_read();
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let tree = {
                                let mut parser = parser_for_diag.blocking_lock();
                                let language = unsafe { crate::tree_sitter_vhdl() };
                                let _ = parser.set_language(&language);
                                parser.parse(&text_for_diag, None)
                            };
                            match tree {
                                Some(t) => {
                                    let root = t.root_node();
                                    crate::backend::features::diagnostics::collect_all_diagnostics(
                                        root,
                                        &analysis_for_diag,
                                        &text_for_diag,
                                        &map,
                                        &uri,
                                    )
                                }
                                None => Vec::new(),
                            }
                        }))
                    })
                    .unwrap()
                    .join();
                match thread_result {
                    Ok(Ok(diags)) => diags,
                    _ => Vec::new(),
                }
            })
            .await
            .unwrap()
        } else {
            vec![]
        }
    };

    // Phase 4: Store analysis in map
    {
        let mut map = analysis_map.write().await;
        map.insert(uri.clone(), analysis);
    }

    diagnostics
}

/// Finds the file URI containing a package declaration by name.
///
/// Searches through the shallow-indexed symbols to locate which file defines the given
/// package. Used by JIT parsing to find package files that need to be deep-parsed.
///
/// # Arguments
/// * `name` - The package name to search for (case-insensitive).
/// * `map` - The global analysis map to search in.
///
/// # Returns
/// The URI of the file containing the package, or `None` if not found.
fn find_package_file(name: &str, map: &AnalysisMap) -> Option<Url> {
    let name_lc = name.to_lowercase();
    for (uri, analysis) in map.iter() {
        if let Some(symbol) = analysis.symbols.get(&name_lc)
            && symbol.kind == OxideSymbolKind::Package
        {
            return Some(uri.clone());
        }
    }
    None
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
            analysis.parse_level == ParseLevel::Shallow
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
        if analysis.package_scope_trees.is_empty()
            && analysis.entity_scope_trees.is_empty()
            && analysis.scope_trees.is_empty()
        {
            client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "JIT Parse returned no scope trees for {}. Keeping existing index",
                        uri
                    ),
                )
                .await;
            return;
        }
        client
            .log_message(
                MessageType::INFO,
                format!("JIT Parse completed for {}. Updating index.", uri),
            )
            .await;
        let mut map = analysis_map.write().await;
        map.insert(uri.clone(), analysis);
    }
}
