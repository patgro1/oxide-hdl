//! * [`features`]: All functions related to feature support (completion, hover etc)
//! * [`syntax`]: Everything needed to parsed files
//! * [`workspace`]: Function related to the workspace
pub mod features;
pub mod syntax;
pub mod workspace;

use crate::config::OxideConfig;
use features::hover;
use syntax::utils::get_word_at_pos;

use crate::analysis::{self, Analysis, OxideSymbolKind, Symbol};
use ropey::Rope;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tree_sitter::Parser;

use tower_lsp::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, Location, MessageType, OneOf, Position,
    ServerCapabilities, SymbolInformation, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
    WorkspaceSymbolParams,
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
}

// Debugger helper function
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
        Backend {
            client,
            config: Arc::new(RwLock::new(None)),
            document_map: Arc::new(RwLock::new(HashMap::new())),
            analysis_map: Arc::new(RwLock::new(HashMap::new())),
            parser: Arc::new(Mutex::new(parser)),
            root_uri: Arc::new(RwLock::new(None)),
            // shallow_query: Arc::new(shallow_query),
        }
    }

    // Debugger helper function
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
    async fn on_change(&self, uri: Url, text: String) {
        let diagnostics = workspace::parse_and_update_document(
            self.analysis_map.clone(),
            self.parser.clone(),
            &uri,
            text,
        )
        .await;
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
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
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
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
            tokio::spawn(async move { workspace::index_workspace(client, map, uri, config).await });
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
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
        self.on_change(uri, text).await;
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
            self.on_change(uri, text).await;
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
                let locations = features::goto::lookup_definition(target, &uri, &map);
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
    /// 2. Resolves candidates using `features::hover::resolve_rich_hover`.
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

        let rope = {
            let map = self.document_map.read().await;
            match map.get(&uri) {
                Some(r) => r.clone(),
                None => return Ok(None),
            }
        };

        if let Some(word) = get_word_at_pos(&rope, position) {
            let target = word.to_lowercase();

            // Fast track using local scope tree
            {
                let map = self.analysis_map.read().await;
                if let Some(analysis) = map.get(&uri)
                    && let Some(decl) = analysis.find_declaration_at(&target, &position)
                {
                    return Ok(Some(self.markup(hover::format_declaration_hover(decl))));
                }
            }

            let candidates = {
                let map = self.analysis_map.read().await;
                hover::resolve_rich_hover(&target, &uri, &map)
            };

            let mut fallback_markdown: Option<String> = None;

            for resolution in candidates {
                // JIT parse if we have a separate def uri
                if let Some(def_uri) = resolution.definition_uri {
                    workspace::ensure_fully_parsed(
                        &self.client,
                        &self.analysis_map,
                        &self.parser,
                        &def_uri,
                    )
                    .await;

                    // Fetch data and render
                    let map = self.analysis_map.read().await;
                    if let Some(analysis) = map.get(&def_uri)
                        && let Some(deep_sym) = analysis.find_symbol(&target)
                    {
                        let is_rich = !deep_sym.children.is_empty() || deep_sym.detail.is_some();
                        let markdown = if deep_sym.kind == OxideSymbolKind::Function {
                            hover::format_function_hover(deep_sym)
                        } else {
                            hover::format_instantiation_hover(&resolution.symbol.name, deep_sym)
                        };
                        if is_rich {
                            return Ok(Some(self.markup(markdown)));
                        } else if fallback_markdown.is_none() {
                            fallback_markdown = Some(markdown)
                        }
                    }
                } else {
                    let markdown = hover::format_basic(&resolution.symbol);
                    return Ok(Some(self.markup(markdown)));
                }
            }
            if let Some(md) = fallback_markdown {
                return Ok(Some(self.markup(md)));
            }
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

        let context = {
            let mut parser = self.parser.lock().await;
            let lang = unsafe { crate::tree_sitter_vhdl() };
            let _ = parser.set_language(&lang);

            let text = rope.to_string();
            let tree = parser.parse(&text, None).unwrap();

            features::completion::get_completion_context(&text, tree.root_node(), position)
        };

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
        let items = features::completion::complete_scope(&map, &uri, &context, position);
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
        }
    }

    /// Create a ScopeTree with declarations and auto-built index
    pub fn make_scope(kind: ScopeKind, range: Range, declarations: Vec<Declaration>) -> ScopeTree {
        let mut scope = ScopeTree {
            kind,
            range,
            name: None,
            entity: None,
            declarations,
            local_usage: HashSet::new(),
            children: vec![],
            decl_index: HashMap::new(),
        };
        scope.rebuild_index();
        scope
    }
}
