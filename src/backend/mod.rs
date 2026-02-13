//! Backend module for the Oxide HDL Language Server.
//!
//! This module contains the core LSP implementation, including:
//! * [`features`]: All functions related to feature support (completion, hover etc)
//! * [`syntax`]: Everything needed to parse files
//! * [`workspace`]: Functions related to the workspace
pub mod features;
pub mod syntax;
pub mod workspace;

use crate::backend::features::lookup;
use crate::config::{DiagnosticTrigger, OxideConfig};
use features::hover;
use syntax::utils::get_word_at_pos;
use tokio::time::Instant;

use crate::analysis::{Analysis, BuiltinManager, OxideSymbolKind, Symbol};
use ropey::Rope;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, watch};
use tower_lsp::jsonrpc::Result;
use tree_sitter::Parser;

use tower_lsp::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, Location,
    MessageType, OneOf, Position, SaveOptions, ServerCapabilities, SymbolInformation,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Url, WorkspaceSymbolParams,
};
use tower_lsp::{Client, LanguageServer};

pub type AnalysisMap = HashMap<Url, Analysis>;

/// The main language server controller.
///
/// This struct holds the shared state of the server, including the document cache,
/// the symbol table (`analysis_map`), and the configuration. It implements the
/// `tower_lsp::LanguageServer` trait to handle incoming JSON-RPC requests.
///
/// # Thread Safety
/// * `parser`: Protected by a `Mutex` because the underlying C-library is not thread-safe.
/// * `analysis_map`: Protected by an `RwLock` to allow massive concurrent reads (e.g. 16 regex threads)
///   while ensuring safe writes during parsing.
pub struct Backend {
    client: Client,
    config: Arc<RwLock<Option<OxideConfig>>>,
    document_map: Arc<RwLock<HashMap<Url, Rope>>>,
    parser: Arc<Mutex<Parser>>,
    analysis_map: Arc<RwLock<AnalysisMap>>,
    root_uri: Arc<RwLock<Option<Url>>>,
    indexing_tx: Arc<watch::Sender<bool>>,
    indexing_rx: watch::Receiver<bool>,
}

/// Recursively dumps a symbol hierarchy to a string for debugging purposes.
///
/// Formats each symbol with indentation based on depth, showing the kind and name.
/// Useful for visualizing the nested structure of VHDL symbols.
///
/// # Arguments
/// * `sym` - The symbol to dump.
/// * `depth` - Current indentation depth (start with 0).
/// * `output` - Mutable string to append the formatted output to.
#[allow(dead_code)]
pub fn dump_symbol_recursive(sym: &Symbol, depth: usize, output: &mut String) {
    let indent = "  ".repeat(depth);
    output.push_str(&format!("{}{:?} {}\n", indent, sym.kind, sym.name));
    for child in &sym.children {
        dump_symbol_recursive(child, depth + 1, output);
    }
}

impl Backend {
    /// Creates a new instance of the Backend.
    ///
    /// # Arguments
    /// * `client` - The handle to the LSP client.
    /// * `parser` - An initialized Tree-sitter parser with the VHDL language set.
    pub fn new(client: Client, parser: Parser) -> Self {
        let builtins = BuiltinManager::new();
        builtins.initialize();
        let (indexing_tx, indexing_rx) = watch::channel(true);
        Backend {
            client,
            config: Arc::new(RwLock::new(None)),
            document_map: Arc::new(RwLock::new(HashMap::new())),
            analysis_map: Arc::new(RwLock::new(HashMap::new())),
            parser: Arc::new(Mutex::new(parser)),
            root_uri: Arc::new(RwLock::new(None)),
            indexing_tx: Arc::new(indexing_tx),
            indexing_rx,
        }
    }

    /// Dumps the entire analysis symbol tree to a string for debugging.
    ///
    /// # Arguments
    /// * `analysis` - The analysis containing symbols to dump.
    ///
    /// # Returns
    /// A formatted string showing all symbols and their hierarchy.
    #[allow(dead_code)]
    fn dump_analysis_tree(&self, analysis: &Analysis) -> String {
        let mut output = String::new();
        for sym in analysis.symbols.values() {
            dump_symbol_recursive(sym, 0, &mut output);
        }
        output
    }

    /// Orchestrates the full parsing of a document when it is opened or changed.
    ///
    /// This function delegates the heavy lifting to the workspace module to ensure
    /// proper threading and stack management.
    ///
    /// # Arguments
    /// * `uri` - The URI of the document being updated.
    /// * `text` - The full text content of the document.
    /// * `publish_diagnostics` - If true, will send the collected diagnostics
    async fn on_change(&self, uri: Url, text: String, publish_diagnostics: bool) {
        let config_guard = self.config.read().await;
        let config = config_guard.clone().unwrap_or_else(OxideConfig::default);
        let diagnostics = workspace::parse_and_update_document(
            &self.client,
            self.analysis_map.clone(),
            self.parser.clone(),
            &uri,
            text,
            publish_diagnostics,
            config,
        )
        .await;
        self.ensure_dependencies_loaded(&uri).await;
        if publish_diagnostics {
            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;
        }
    }

    /// Helper to construct a consistent `Hover` response object.
    ///
    /// # Arguments
    /// * `text` - The Markdown string content to display.
    ///
    /// # Returns
    /// A `tower_lsp::lsp_types::Hover` object containing the markdown content.
    fn markup(&self, text: String) -> tower_lsp::lsp_types::Hover {
        tower_lsp::lsp_types::Hover {
            contents: tower_lsp::lsp_types::HoverContents::Markup(
                tower_lsp::lsp_types::MarkupContent {
                    kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                    value: text.to_string(),
                },
            ),
            range: None,
        }
    }

    /// Checks the dependencies of a parsed file and lazy-loads them if missing.
    ///
    /// Scans the file's `use` clauses to identify imported packages, then checks
    /// if each dependency is already in the analysis map. Missing dependencies
    /// (typically standard library packages like IEEE) are JIT-loaded from the cache.
    ///
    /// # Arguments
    /// * `uri` - The URI of the file whose dependencies should be checked.
    async fn ensure_dependencies_loaded(&self, uri: &Url) {
        // 1. Get the analysis of the current file to find what it "uses"
        let deps_to_load = {
            let map = self.analysis_map.read().await;
            let analysis = match map.get(uri) {
                Some(a) => a,
                None => return,
            };

            let mut missing_deps = Vec::new();

            for clause in &analysis.use_clauses {
                // CALL THE INTERCEPTOR!
                if let Some(dep_uri) = lookup::resolve_import_uri(&clause.library, &clause.name)
                    && !map.contains_key(&dep_uri)
                {
                    missing_deps.push(dep_uri);
                }
            }
            missing_deps
        }; // Lock dropped here

        // 2. Load the missing files (JIT)
        for dep_uri in deps_to_load {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("JIT Loading dependency: {}", dep_uri),
                )
                .await;

            if let Ok(path) = dep_uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(path).await
            {
                let config_guard = self.config.read().await;
                let config = config_guard.clone().unwrap_or_else(OxideConfig::default);
                workspace::parse_and_update_document(
                    &self.client,
                    self.analysis_map.clone(),
                    self.parser.clone(),
                    &dep_uri,
                    text,
                    false,
                    config,
                )
                .await;
            }
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut chosen_uri = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .map(|f| f.uri.clone());
        if chosen_uri.is_none() {
            chosen_uri = params.root_uri;
        }
        if let Some(uri) = chosen_uri {
            let mut root = self.root_uri.write().await;
            *root = Some(uri);
        } else {
            eprintln!("Oxide HDL: No root URI or Workspace Folder found initialization.");
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        will_save: None,
                        will_save_wait_until: None,
                    },
                )),
                // Goto def
                definition_provider: Some(OneOf::Left(true)),
                // Hover
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Completion
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        ">".to_string(),
                        "(".to_string(),
                        ",".to_string(),
                        ":".to_string(),
                    ]),
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    ..Default::default()
                }),
                // Document symbol
                document_symbol_provider: Some(OneOf::Left(true)),
                // Workspace symbol
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let _ = self.indexing_tx.send(false);
        self.client
            .log_message(MessageType::INFO, "Oxide HDL initialized!")
            .await;
        let root_uri = {
            let read_lock = self.root_uri.read().await;
            read_lock.clone()
        };
        self.client
            .log_message(
                MessageType::WARNING,
                format!("Current Root URI is {:?}", root_uri),
            )
            .await;
        if let Some(uri) = root_uri {
            let client = self.client.clone();
            let map = self.analysis_map.clone();
            let root_path = uri.to_file_path().unwrap();
            let config = OxideConfig::load(&root_path);
            client
                .log_message(
                    MessageType::INFO,
                    format!("Loaded config with {} ignore patterns", config.ignore.len()),
                )
                .await;

            {
                let mut w = self.config.write().await;
                *w = Some(config.clone());
            }
            let tx = self.indexing_tx.clone();
            tokio::spawn(async move {
                workspace::index_workspace(client, map, uri, config).await;
                let _ = tx.send(true);
            });
        } else {
            // No workspace to index, mark as done
            let _ = self.indexing_tx.send(true);
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let start_time = Instant::now();
        {
            let mut rx = self.indexing_rx.clone();
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.client
            .log_message(MessageType::INFO, format!("Opened file {}", uri))
            .await;

        let rope = Rope::from_str(&text);
        {
            let mut map = self.document_map.write().await;
            map.insert(uri.clone(), rope.clone());
        }
        self.on_change(uri.clone(), text, true).await;
        let duration = start_time.elapsed();
        self.client
            .log_message(
                MessageType::WARNING,
                format!("did_open for {} took {:?}", uri, duration),
            )
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let mut map = self.document_map.write().await;

        if let Some(rope) = map.get_mut(&uri) {
            for change in params.content_changes {
                // We only got incremental change
                if let Some(range) = change.range {
                    let start_idx = rope
                        .try_line_to_char(range.start.line as usize)
                        .unwrap_or(0)
                        + range.start.character as usize;
                    let end_idx = rope.try_line_to_char(range.end.line as usize).unwrap_or(0)
                        + range.end.character as usize;

                    if start_idx <= end_idx && end_idx <= rope.len_chars() {
                        rope.remove(start_idx..end_idx);
                        rope.insert(start_idx, &change.text);
                    }
                }
                // For some reason the client sent the full file so lets update the rope completely
                else {
                    *rope = Rope::from_str(&change.text)
                }
            }
            let text = rope.to_string();
            drop(map);
            let config_guard = self.config.read().await;
            let should_publish = config_guard
                .as_ref()
                .map(|c| c.diagnostics == DiagnosticTrigger::OnChange)
                .unwrap_or(false);
            self.on_change(uri, text, should_publish).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Ok(path) = uri.to_file_path() {
            if let Ok(text) = tokio::fs::read_to_string(path).await {
                // 3. Optional: Sync your memory rope with what's on disk
                // This fixes 'hover' if did_change somehow got desynced
                {
                    let mut map = self.document_map.write().await;
                    map.insert(uri.clone(), Rope::from_str(&text));
                }
                // 4. Run Analysis
                self.on_change(uri, text, true).await;
            } else {
                self.client
                    .log_message(MessageType::ERROR, "Failed to read file from disk")
                    .await;
            }
        }
    }

    /// Handles the "Go to Definition" request.
    ///
    /// # Logic
    /// 1. Identifies the word under the cursor using `get_word_at_pos`.
    /// 2. Delegates the lookup strategy to `features::goto::lookup_definition`.
    /// 3. Returns a list of `Location` candidates (supporting overloads/multiple matches).
    ///
    /// # Arguments
    /// * `params` - Contains the cursor position and text document URI.
    ///
    /// # Returns
    /// * `Ok(Some(Array))` - A list of locations where the symbol is defined.
    /// * `Ok(None)` - If no definition is found.
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Get context
        let rope = {
            let map = self.document_map.read().await;
            match map.get(&uri) {
                Some(r) => r.clone(),
                None => return Ok(None),
            }
        };

        if let Some(word) = get_word_at_pos(&rope, position) {
            let map = self.analysis_map.read().await;

            self.client
                .log_message(MessageType::INFO, format!("Looking for: '{}'", word))
                .await;
            let target = &word.to_lowercase();
            if let Some(analysis) = map.get(&uri)
                && let Some(decl) = analysis.find_declaration_at(target, &position)
            {
                return Ok(Some(GotoDefinitionResponse::Array(
                    [Location {
                        uri,
                        range: decl.selection_range,
                    }]
                    .to_vec(),
                )));
            } else {
                let locations = features::goto::lookup_definition(target, &uri, &map, position);
                if !locations.is_empty() {
                    return Ok(Some(GotoDefinitionResponse::Array(locations)));
                }
            }
        }

        Ok(None)
    }

    /// Handles the "Hover" request to show documentation/type info.
    ///
    /// # Logic
    /// 1. Identifies the word under the cursor.
    /// 2. Resolves candidates using `features::hover::resolve_hover`.
    /// 3. **JIT Parsing:** If a candidate points to a file that is only "Shallowly" indexed
    ///    (Regex scan), it triggers `workspace::ensure_fully_parsed` to parse it immediately.
    /// 4. Re-fetches the rich data and formats it using `features::hover` formatters.
    ///
    /// # Arguments
    /// * `params` - Contains the cursor position and text document URI.
    ///
    /// # Returns
    /// * `Ok(Some(Hover))` - The markdown formatted documentation.
    /// * `Ok(None)` - If no symbol or documentation is found.
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let (text, _) = {
            let map = self.document_map.read().await;
            match map.get(&uri) {
                Some(r) => (r.to_string(), r.clone()),
                None => return Ok(None),
            }
        };

        let tree = {
            let mut parser = self.parser.lock().await;
            let lang = unsafe { crate::tree_sitter_vhdl() };
            let _ = parser.set_language(&lang);
            parser.parse(&text, None).unwrap()
        };
        let root_node = tree.root_node();

        let (mut candidates, needs_jit) = {
            let map = self.analysis_map.read().await;
            // let results = hover::resolve_rich_hover(&target, &uri, &map, position);
            let results = hover::resolve_hover(&map, &uri, &text, root_node, position);
            let mut to_upgrade = Vec::new();
            for res in &results {
                if let Some(def_uri) = &res.definition_uri
                    && let Some(analysis) = map.get(def_uri)
                    && analysis.parse_level == crate::analysis::ParseLevel::Shallow
                {
                    to_upgrade.push(def_uri.clone());
                }
            }
            (results, to_upgrade)
        };

        if !needs_jit.is_empty() {
            for def_uri in needs_jit {
                workspace::ensure_fully_parsed(
                    &self.client,
                    &self.analysis_map,
                    &self.parser,
                    &def_uri,
                )
                .await;
            }
            let map = self.analysis_map.read().await;
            candidates = hover::resolve_hover(&map, &uri, &text, root_node, position);
        }
        if let Some(match_res) = candidates.first() {
            let markdown = hover::format_hover_result(match_res);
            return Ok(Some(self.markup(markdown)));
        }
        Ok(None)
    }

    /// Handles the "Document Symbols" request (Outline View / Breadcrumbs).
    ///
    /// Returns the hierarchical symbol tree for the current file, converted into
    /// LSP `DocumentSymbol` types. This relies on the Deep Parse having run successfully
    /// during `did_open` or `did_change`.
    ///
    /// # Arguments
    /// * `params` - Contains the text document URI.
    ///
    /// # Returns
    /// * `Ok(Some(Nested))` - A tree of document symbols.
    /// * `Ok(None)` - If the file has not been parsed or has no symbols.
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let map = self.analysis_map.read().await;

        if let Some(analysis) = map.get(&uri) {
            let mut symbols = features::symbol::collect_document_symbol(analysis);
            symbols.sort_by(|a, b| a.range.start.cmp(&b.range.start));
            return Ok(Some(DocumentSymbolResponse::Nested(symbols)));
        }
        Ok(None)
    }

    /// Handles the "Workspace Symbols" request (Outline View / Breadcrumbs).
    ///
    /// Returns the hierarchical symbol tree for the current file, converted into
    /// LSP `DocumentSymbol` types. This relies on the Deep Parse having run successfully
    /// during `did_open` or `did_change`.
    ///
    /// # Arguments
    /// * `params` - Contains the text document URI.
    ///
    /// # Returns
    /// * `Ok(Some(Nested))` - A tree of document symbols.
    /// * `Ok(None)` - If the file has not been parsed or has no symbols.
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let map = self.analysis_map.read().await;
        let query = params.query;
        let symbols = features::symbol::collect_workspace_symb(&map, &query);
        return Ok(Some(symbols));
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let rope = {
            let map = self.document_map.read().await;
            match map.get(&uri) {
                Some(r) => r.clone(),
                None => return Ok(None),
            }
        };

        let text = rope.to_string();
        let tree = {
            let mut parser = self.parser.lock().await;
            let lang = unsafe { crate::tree_sitter_vhdl() };
            let _ = parser.set_language(&lang);

            parser.parse(&text, None).unwrap()
        };

        let context =
            features::completion::get_completion_context(&text, tree.root_node(), position);

        self.client
            .log_message(MessageType::INFO, format!("Context: {:?}", context))
            .await;

        if let features::completion::CompletionContext::PortMapLhs(ref comp_name)
        | features::completion::CompletionContext::GenericMapLhs(ref comp_name) = context
        {
            let def_uri = {
                let map = self.analysis_map.read().await;
                let mut target_uri = None;

                for (u, analysis) in map.iter() {
                    if analysis
                        .symbols
                        .values()
                        .any(|s| s.name == *comp_name && s.kind == OxideSymbolKind::Entity)
                    {
                        target_uri = Some(u.clone());
                        break;
                    }
                }
                target_uri
            };

            if let Some(def_uri) = def_uri {
                workspace::ensure_fully_parsed(
                    &self.client,
                    &self.analysis_map,
                    &self.parser,
                    &def_uri,
                )
                .await;
            }
        }
        let map = self.analysis_map.read().await;
        let items = features::completion::complete_scope(
            &map,
            &uri,
            &context,
            position,
            &text,
            tree.root_node(),
        );
        return Ok(Some(CompletionResponse::Array(items)));
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub mod test_utils {
    use lazy_static::lazy_static;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;
    use tower_lsp::lsp_types::{Position, Range};
    use tree_sitter::Parser;

    use crate::analysis::{DeclType, Declaration, ScopeKind, ScopeTree, TypeInfo};

    lazy_static! {
        pub static ref SHARED_PARSER_LOCK: Mutex<()> = Mutex::new(());
    }

    /// Shared VHDL parsing helper for tests.
    /// Uses the shared parser lock to ensure thread safety.
    pub fn parse_text(code: &str) -> tree_sitter::Tree {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        parser.parse(code, None).unwrap()
    }

    /// Create a Range with full position control
    pub fn make_range(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: start_char,
            },
            end: Position {
                line: end_line,
                character: end_char,
            },
        }
    }

    /// Create a simple Range using only line numbers (character = 0)
    pub fn make_line_range(start_line: u32, end_line: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: end_line,
                character: 0,
            },
        }
    }

    /// Create a Position
    pub fn make_pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    /// Create a Declaration with minimal fields
    pub fn make_decl(name: &str, decl_type: DeclType) -> Declaration {
        Declaration {
            name: name.to_lowercase(),
            decl_type,
            range: Range::default(),
            selection_range: Range::default(),
            type_info: TypeInfo::new(),
            default_value: None,
            doc_comment: None,
            parameters: None,
        }
    }

    /// Create a Declaration with a specific range
    pub fn make_decl_with_range(name: &str, decl_type: DeclType, range: Range) -> Declaration {
        Declaration {
            name: name.to_string(),
            decl_type,
            range,
            selection_range: Range::default(),
            type_info: TypeInfo::new(),
            default_value: None,
            doc_comment: None,
            parameters: None,
        }
    }

    /// Create a ScopeTree with declarations and auto-built index
    pub fn make_scope(kind: ScopeKind, range: Range, declarations: Vec<Declaration>) -> ScopeTree {
        let mut scope = ScopeTree {
            kind,
            range,
            name: None,
            entity: None,
            package: None,
            declarations,
            local_usage: HashSet::new(),
            children: vec![],
            decl_index: HashMap::new(),
            instantiations: Vec::new(),
        };
        scope.rebuild_index();
        scope
    }
}
