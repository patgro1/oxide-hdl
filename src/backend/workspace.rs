// src/backend/workspace.rs

use crate::analysis::{Analysis, OxideSymbolKind, ParseLevel};
use crate::backend::AnalysisMap;
use crate::backend::syntax::{parser, scanner};
use crate::config::{LibraryMatcher, OxideConfig};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::task::JoinSet;
use tower_lsp::Client;
use tower_lsp::lsp_types::Diagnostic;
use tower_lsp::lsp_types::{MessageType, Url};
use walkdir::WalkDir;

/// Shallow-scans one file's text and stamps the library it belongs to.
///
/// Factored out of `index_workspace` so the scan-and-stamp step is unit-testable:
/// `index_workspace` itself needs a live `Client` and a real directory tree, but
/// this does not. Pure — no I/O, no locks.
///
/// # Arguments
/// * `text` - File contents.
/// * `path` - Path on disk, used only to resolve the library.
/// * `matcher` - Compiled `[libraries]` globs.
pub fn analysis_for_file(text: &str, path: &Path, matcher: &LibraryMatcher) -> Analysis {
    let mut analysis = Analysis::new();
    analysis.library = matcher.library_for(path);
    for s in scanner::scan_fast(text) {
        analysis.symbols.insert(s.name.clone().to_lowercase(), s);
    }
    analysis
}

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
#[tracing::instrument(skip_all)]
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
    let lib_matcher = Arc::new(LibraryMatcher::from_config(&config));

    let mut paths: Vec<std::path::PathBuf> = Vec::new();

    // Process main workspace
    {
        let is_ignored = |entry: &walkdir::DirEntry| -> bool {
            let name = entry.file_name().to_string_lossy();

            // A. HARDCODED SAFETY NET (The Guard Rails)
            if name == ".git" || name == "target" {
                return true;
            }

            // B. USER CONFIG (The Flexibility)
            if let Ok(relative) = entry.path().strip_prefix(&root_path) {
                return matcher.is_match(relative);
            }
            false
        };

        let root_paths: Vec<std::path::PathBuf> = WalkDir::new(&root_path)
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
        paths.extend(root_paths);
    }

    // Process included workspaces
    for inc in &config.include_workspace {
        let inc_path = root_path.join(inc);
        let is_ignored = |entry: &walkdir::DirEntry| -> bool {
            let name = entry.file_name().to_string_lossy();

            if name == ".git" || name == "target" {
                return true;
            }

            if let Ok(relative) = entry.path().strip_prefix(&inc_path) {
                return matcher.is_match(relative);
            }
            false
        };

        let inc_paths: Vec<std::path::PathBuf> = WalkDir::new(&inc_path)
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

        if inc_paths.is_empty() {
            client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "Included workspace folder '{}' did not resolve to any indexed files",
                        inc
                    ),
                )
                .await;
        } else {
            paths.extend(inc_paths);
        }
    }

    paths.sort();
    paths.dedup();

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
        let lib_clone = lib_matcher.clone();
        let path_uri = match Url::from_file_path(&path) {
            Ok(u) => u,
            Err(_) => continue,
        };
        join_set.spawn(async move {
            let _permit = sem_clone.acquire_owned().await.unwrap();
            tokio::task::spawn_blocking(move || {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                (path_uri, analysis_for_file(&text, &path, &lib_clone))
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
            format!("Indexed workspace in {:?}", duration),
        )
        .await;
}

/// Decides whether a freshly parsed [`Analysis`] should replace the stored one.
///
/// # Why this exists
///
/// The deep-parse path runs on every keystroke (with the default `on_save`
/// diagnostics setting, `did_change` awaits `on_change` directly). While the user
/// is typing, the buffer is frequently unparseable for a few keystrokes at a time
/// — an `if` without its `end if;`, an unclosed `process`, a file not yet
/// terminated with `end architecture;`. Tree-sitter cannot build an
/// `architecture_definition` in that state, so the resulting `Analysis` has no
/// scope trees whatsoever.
///
/// Writing that into the map used to destroy the perfectly good analysis from the
/// previous keystroke, taking completion, hover and go-to-definition down with it
/// until the construct was closed. Holding the last good analysis instead leaves
/// the data a few lines stale, which is dramatically better than leaving it blank.
///
/// [`insert_batch`] and [`ensure_fully_parsed`] already make this same trade.
///
/// # Arguments
/// * `previous` - The analysis currently stored for this file, if any.
/// * `fresh` - The analysis just produced from the current buffer text.
///
/// # Returns
/// `false` only when `fresh` is degenerate and `previous` holds real content.
pub fn should_replace(previous: Option<&Analysis>, fresh: &Analysis) -> bool {
    match previous {
        Some(prev) => !(fresh.has_no_scope_trees() && !prev.has_no_scope_trees()),
        None => true,
    }
}

/// Stores a freshly parsed [`Analysis`], unless doing so would destroy good data.
///
/// Delegates the decision to [`should_replace`]; see that function for why a
/// mid-edit buffer must not be allowed to blank out the map.
///
/// # Arguments
/// * `analysis_map` - The global symbol table.
/// * `uri` - File whose analysis is being stored.
/// * `analysis` - The freshly parsed analysis.
pub async fn store_analysis(
    analysis_map: &Arc<RwLock<AnalysisMap>>,
    uri: &Url,
    analysis: Analysis,
) {
    let mut map = analysis_map.write().await;
    if should_replace(map.get(uri), &analysis) {
        map.insert(uri.clone(), analysis);
    }
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
#[tracing::instrument(skip_all, fields(uri = %uri, get_diagnostics))]
pub async fn parse_and_update_document(
    client: &Client,
    analysis_map: Arc<RwLock<AnalysisMap>>,
    parser: Arc<Mutex<crate::backend::Parser>>,
    uri: &Url,
    text: String,
    get_diagnostics: bool,
    config: OxideConfig,
) -> Vec<Diagnostic> {
    let uri = uri.clone();
    let lib_matcher = LibraryMatcher::from_config(&config);
    let text_for_diag = text.clone();
    let parser_for_diag = parser.clone();

    // Phase 1: Parse current file, extract analysis and needed packages
    let phase1_result = {
        let parser = parser.clone();
        let phase1_span = tracing::info_span!("phase1_tree_sitter_parse");
        tokio::task::spawn_blocking(move || {
            let _enter = phase1_span.enter();
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
            ensure_fully_parsed(client, &analysis_map, &parser, &pkg_uri, &lib_matcher).await;
        }
    }

    // JIT parse packages referenced in inner scope use_clauses (generates, blocks)
    let inner_clauses: Vec<crate::analysis::UseClause> = analysis
        .scope_trees
        .iter()
        .flat_map(|t| t.collect_all_use_clauses())
        .collect();
    for clause in &inner_clauses {
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
            ensure_fully_parsed(client, &analysis_map, &parser, &pkg_uri, &lib_matcher).await;
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

    // Phase 3: Stamp the library, then store (the store is skipped if the buffer is
    // mid-edit and momentarily unparseable — see `store_analysis`). A fresh Analysis
    // defaults to "work" and would otherwise clobber what the indexer resolved.
    let mut analysis = analysis;
    if let Ok(path) = uri.to_file_path() {
        analysis.library = lib_matcher.library_for(&path);
    }
    store_analysis(&analysis_map, &uri, analysis.clone()).await;

    // Phase 4: Run diagnostics (now with access to imported packages)

    if get_diagnostics {
        let analysis_for_diag = analysis;
        let map_ref = analysis_map.clone();
        let uri = uri.clone();
        let phase4_span = tracing::info_span!("phase4_diagnostics");
        tokio::task::spawn_blocking(move || {
            let _enter = phase4_span.enter();
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
                                    &config,
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
}

/// Finds the file URI containing a package declaration by name.
///
/// Searches both shallow-indexed `symbols` and deep-parsed `package_scope_trees`
/// to locate which file defines the given package. This ensures packages can be
/// found regardless of their current parse level.
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
            && (symbol.kind == OxideSymbolKind::Package
                || symbol.kind == OxideSymbolKind::PackageBody)
        {
            return Some(uri.clone());
        }
        if analysis
            .package_declaration_scope_trees
            .contains_key(&name_lc)
            || analysis.package_body_scope_trees.contains_key(&name_lc)
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
/// * `matcher` - Compiled `[libraries]` globs, used to re-stamp the upgraded analysis.
#[tracing::instrument(skip_all, fields(uri = %uri))]
pub async fn ensure_fully_parsed(
    client: &Client,
    analysis_map: &Arc<RwLock<AnalysisMap>>,
    parser: &Arc<Mutex<crate::backend::Parser>>,
    uri: &Url,
    matcher: &LibraryMatcher,
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
    let jit_span = tracing::info_span!("jit_tree_sitter_parse");
    let result = tokio::task::spawn_blocking(move || {
        let _enter = jit_span.enter();
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
        if analysis.has_no_scope_trees() {
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
        // A JIT upgrade replaces the shallow Analysis wholesale, so the library must
        // be recomputed — the fresh Analysis defaults to "work" and would otherwise
        // silently clobber what the indexer resolved for this file.
        let mut analysis = analysis;
        if let Ok(path) = uri.to_file_path() {
            analysis.library = matcher.library_for(&path);
        }
        let mut map = analysis_map.write().await;
        map.insert(uri.clone(), analysis);
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::Analysis;
    use crate::config::LibraryMatcher;
    use std::path::PathBuf;

    /// Parses VHDL into an Analysis, the way the deep-parse path does.
    fn analyze(code: &str) -> Analysis {
        let tree = crate::backend::test_utils::parse_text(code);
        crate::backend::syntax::parser::extract_document_symbols(code, tree.root_node())
    }

    const GOOD: &str = "architecture rtl of top is\n  signal a : bit;\nbegin\n  b <= a;\nend architecture;\n";
    // Mid-keystroke: the `if` has no `end if;` yet, which collapses the whole tree.
    const MID_TYPING: &str = "architecture rtl of top is\n  signal a : bit;\nbegin\n  process(a)\n  begin\n    if a = '1' then\n  end process;\nend architecture;\n";

    #[test]
    fn test_replaces_when_nothing_stored_yet() {
        let fresh = analyze(GOOD);
        assert!(super::should_replace(None, &fresh));
    }

    #[test]
    fn test_replaces_good_with_good() {
        let previous = analyze(GOOD);
        let fresh = analyze(GOOD);
        assert!(super::should_replace(Some(&previous), &fresh));
    }

    #[test]
    fn test_keeps_previous_when_buffer_becomes_unparseable() {
        // THE BUG: typing `if ... then` wiped the stored analysis, killing
        // completion/hover/goto until `end if;` was typed.
        let previous = analyze(GOOD);
        let fresh = analyze(MID_TYPING);
        assert!(
            !previous.has_no_scope_trees(),
            "fixture invalid: previous should have content"
        );
        assert!(
            fresh.has_no_scope_trees(),
            "fixture invalid: mid-typing text should collapse the tree"
        );
        assert!(
            !super::should_replace(Some(&previous), &fresh),
            "must not overwrite a good analysis with an empty one"
        );
    }

    #[test]
    fn test_replaces_when_previous_was_also_empty() {
        // Nothing worth preserving — keep refreshing so symbols/use_clauses stay current.
        let previous = analyze(MID_TYPING);
        let fresh = analyze(MID_TYPING);
        assert!(super::should_replace(Some(&previous), &fresh));
    }

    #[test]
    fn test_recovers_once_the_construct_is_closed() {
        // After the guard held the old analysis, closing the construct must let
        // the new one through — otherwise edits would never land.
        let previous = analyze(GOOD);
        let fresh = analyze(
            "architecture rtl of top is\n  signal a : bit;\n  signal z : bit;\nbegin\n  z <= a;\nend architecture;\n",
        );
        assert!(super::should_replace(Some(&previous), &fresh));
    }

    #[test]
    fn test_entity_only_file_replaces_normally() {
        // Entity files populate entity_scope_trees, not scope_trees; they must not
        // be mistaken for unparseable buffers and skipped.
        let previous = analyze("entity top is\n  port (clk : in bit);\nend entity;\n");
        let fresh = analyze("entity top is\n  port (clk : in bit; rst : in bit);\nend entity;\n");
        assert!(super::should_replace(Some(&previous), &fresh));
    }

    #[tokio::test]
    async fn test_store_analysis_writes_when_map_is_empty() {
        let map: std::sync::Arc<tokio::sync::RwLock<crate::backend::AnalysisMap>> =
            std::sync::Arc::new(tokio::sync::RwLock::new(crate::backend::AnalysisMap::new()));
        let uri = tower_lsp::lsp_types::Url::parse("file:///top.vhd").unwrap();

        super::store_analysis(&map, &uri, analyze(GOOD)).await;

        let guard = map.read().await;
        assert!(!guard.get(&uri).unwrap().has_no_scope_trees());
    }

    #[tokio::test]
    async fn test_store_analysis_preserves_good_data_against_unparseable_buffer() {
        // End-to-end on the real write path: store a good analysis, then simulate the
        // next keystroke leaving the buffer unparseable. The good one must survive.
        let map: std::sync::Arc<tokio::sync::RwLock<crate::backend::AnalysisMap>> =
            std::sync::Arc::new(tokio::sync::RwLock::new(crate::backend::AnalysisMap::new()));
        let uri = tower_lsp::lsp_types::Url::parse("file:///top.vhd").unwrap();

        super::store_analysis(&map, &uri, analyze(GOOD)).await;
        super::store_analysis(&map, &uri, analyze(MID_TYPING)).await;

        let guard = map.read().await;
        let stored = guard.get(&uri).unwrap();
        assert!(
            !stored.has_no_scope_trees(),
            "an unparseable keystroke wiped the stored analysis"
        );
        assert!(
            stored
                .scope_trees
                .iter()
                .any(|t| t.declarations.iter().any(|d| d.name == "a")),
            "signal `a` should still be visible to completion while typing"
        );
    }

    #[tokio::test]
    async fn test_store_analysis_accepts_edits_once_parseable_again() {
        let map: std::sync::Arc<tokio::sync::RwLock<crate::backend::AnalysisMap>> =
            std::sync::Arc::new(tokio::sync::RwLock::new(crate::backend::AnalysisMap::new()));
        let uri = tower_lsp::lsp_types::Url::parse("file:///top.vhd").unwrap();

        super::store_analysis(&map, &uri, analyze(GOOD)).await;
        super::store_analysis(&map, &uri, analyze(MID_TYPING)).await;
        // User finishes typing `end if;` — the new signal must now land.
        super::store_analysis(
            &map,
            &uri,
            analyze("architecture rtl of top is\n  signal a : bit;\n  signal z : bit;\nbegin\n  z <= a;\nend architecture;\n"),
        )
        .await;

        let guard = map.read().await;
        assert!(
            guard.get(&uri).unwrap().scope_trees.iter()
                .any(|t| t.declarations.iter().any(|d| d.name == "z")),
            "the guard must not block a recovered parse from landing"
        );
    }

    #[tokio::test]
    async fn test_completion_survives_an_unclosed_if_while_typing() {
        // The user-visible symptom this guard exists to fix: open a healthy file,
        // start typing `if ... then` inside a process, and every completion in the
        // file used to vanish until `end if;` was typed.
        use tower_lsp::lsp_types::Position;

        let map: std::sync::Arc<tokio::sync::RwLock<crate::backend::AnalysisMap>> =
            std::sync::Arc::new(tokio::sync::RwLock::new(crate::backend::AnalysisMap::new()));
        let uri = tower_lsp::lsp_types::Url::parse("file:///top.vhd").unwrap();

        // 1. File is healthy on open.
        let healthy = "architecture rtl of top is\n  signal sig_alpha : bit;\n  signal sig_beta : bit;\nbegin\n  process(sig_alpha)\n  begin\n  end process;\nend architecture;\n";
        super::store_analysis(&map, &uri, analyze(healthy)).await;

        // 2. User types `if sig_alpha = '1' then` — buffer is now unparseable.
        let typing = "architecture rtl of top is\n  signal sig_alpha : bit;\n  signal sig_beta : bit;\nbegin\n  process(sig_alpha)\n  begin\n    if sig_alpha = '1' then\n      \n  end process;\nend architecture;\n";
        super::store_analysis(&map, &uri, analyze(typing)).await;

        // 3. Completion inside the half-written if-body must still offer the signals.
        let guard = map.read().await;
        let tree = crate::backend::test_utils::parse_text(typing);
        let root = tree.root_node();
        let pos = Position { line: 7, character: 6 };
        let ctx = crate::backend::features::completion::get_completion_context(typing, root, pos);
        let items = crate::backend::features::completion::complete_scope(
            &guard, &uri, &ctx, pos, typing, root,
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

        assert!(
            labels.contains(&"sig_alpha") && labels.contains(&"sig_beta"),
            "signals vanished from completion while typing an unclosed if; got {labels:?}"
        );
    }

    fn matcher() -> LibraryMatcher {
        LibraryMatcher::new(
            vec![("rtl_lib".to_string(), vec!["rtl/**/*.vhd".to_string()])],
            PathBuf::from("/ws"),
        )
    }

    #[test]
    fn test_analysis_for_file_stamps_matched_library() {
        let src = "entity uart_tx is\nend entity;\n";
        let a = super::analysis_for_file(src, &PathBuf::from("/ws/rtl/uart_tx.vhd"), &matcher());
        assert_eq!(a.library, "rtl_lib");
    }

    #[test]
    fn test_analysis_for_file_defaults_unmatched_to_work() {
        let src = "entity uart_tx is\nend entity;\n";
        let a = super::analysis_for_file(src, &PathBuf::from("/ws/tb/uart_tb.vhd"), &matcher());
        assert_eq!(a.library, "work");
    }

    #[test]
    fn test_analysis_for_file_still_populates_symbols() {
        // The library stamp must not disturb the shallow scan it wraps.
        let src = "entity uart_tx is\nend entity;\n";
        let a = super::analysis_for_file(src, &PathBuf::from("/ws/rtl/uart_tx.vhd"), &matcher());
        assert!(
            a.symbols.contains_key("uart_tx"),
            "shallow scan results lost, got keys: {:?}",
            a.symbols.keys().collect::<Vec<_>>()
        );
    }
}
