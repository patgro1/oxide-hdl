pub mod features;
pub mod syntax;
pub mod workspace;

use crate::config::OxideConfig;
use features::hover;
use syntax::utils::get_word_at_pos;

use crate::analysis::{Analysis, OxideSymbolKind, Symbol};
use ropey::Rope;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tree_sitter::Parser;

use tower_lsp::lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MessageType, OneOf, Position, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer};

pub type AnalysisMap = HashMap<Url, Analysis>;

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

pub fn to_document_symbol(sym: &crate::analysis::Symbol) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: sym.name.clone(),
        detail: sym.detail.clone(),
        kind: sym.kind.into(),
        tags: None,
        deprecated: None,
        range: sym.range,
        selection_range: sym.range,
        children: if sym.children.is_empty() {
            None
        } else {
            let mut children_list: Vec<DocumentSymbol> =
                sym.children.iter().map(to_document_symbol).collect();
            children_list.sort_by(|a, b| a.range.start.cmp(&b.range.start));
            Some(children_list)
        },
    }
}

impl Backend {
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

    async fn on_change(&self, uri: Url, text: String) {
        workspace::parse_and_update_document(
            self.analysis_map.clone(),
            self.parser.clone(),
            &uri,
            text,
        )
        .await;
    }

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
                completion_provider: Some(CompletionOptions::default()),
                // Document symbol
                document_symbol_provider: Some(OneOf::Left(true)),
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
            let locations = features::goto::lookup_definition(&word, &uri, &map);
            if !locations.is_empty() {
                return Ok(Some(GotoDefinitionResponse::Array(locations)));
            }
        }

        Ok(None)
    }

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

            let candidates = {
                let map = self.analysis_map.read().await;
                hover::resolve_rich_hover(&target, &uri, &map)
            };

            let mut fallback_markdown: Option<String> = None;

            for resolution in candidates {
                // JIT parse if we have a separate def uri
                if let Some(def_uri) = resolution.definition_uri {
                    self.ensure_fully_parsed(&def_uri).await;

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

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let map = self.analysis_map.read().await;

        if let Some(analysis) = map.get(&uri) {
            let mut symbols = Vec::new();
            for sym in analysis.symbols.values() {
                symbols.push(to_document_symbol(sym))
            }
            symbols.sort_by(|a, b| a.range.start.cmp(&b.range.start));
            return Ok(Some(DocumentSymbolResponse::Nested(symbols)));
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
