use crate::analysis::Analysis;
use ropey::Rope;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::jsonrpc::Result;
use tree_sitter::Parser;
use walkdir::WalkDir;

pub struct Backend {
    client: Client,
    document_map: Arc<RwLock<HashMap<Url, Rope>>>,
    parser: Arc<Mutex<Parser>>,
    analysis_map: Arc<RwLock<HashMap<Url, Analysis>>>,
    root_uri: Arc<RwLock<Option<Url>>>,
}
use tower_lsp::lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, Location, MessageType, OneOf, Position,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer};

impl Backend {
    pub fn new(client: Client, parser: Parser) -> Self {
        Backend {
            client,
            document_map: Arc::new(RwLock::new(HashMap::new())),
            analysis_map: Arc::new(RwLock::new(HashMap::new())),
            parser: Arc::new(Mutex::new(parser)),
            root_uri: Arc::new(RwLock::new(None)),
        }
    }
    fn get_word_at_pos(&self, rope: &Rope, position: Position) -> Option<String> {
        let line_idx = rope.try_line_to_char(position.line as usize).ok()?;
        let char_idx = line_idx + position.character as usize;

        if char_idx >= rope.len_chars() {
            return None;
        }

        // Find the start of the word
        let mut start = char_idx;
        while start > 0 {
            let c = rope.char(start - 1);
            if !c.is_alphanumeric() && c != '_' {
                break;
            }
            start -= 1;
        }

        let mut end = char_idx;
        while end < rope.len_chars() {
            let c = rope.char(end);
            if !c.is_alphanumeric() && c != '_' {
                break;
            }
            end += 1;
        }

        if start < end {
            Some(rope.slice(start..end).to_string())
        } else {
            None
        }
    }

    async fn on_change(&self, uri: Url, text: String, rope: Rope) {
        let mut parser = self.parser.lock().await;

        if let Some(tree) = parser.parse(&text, None) {
            let analysis = Analysis::extract(tree.root_node(), &text, &rope);

            let diagnostics = Analysis::get_diagnostics(tree, &text);
            self.client
                .publish_diagnostics(uri.clone(), diagnostics, None)
                .await;
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Indexed {} symbols in {}", analysis.symbols.len(), uri),
                )
                .await;

            let mut map = self.analysis_map.write().await;
            map.insert(uri, analysis);
        }
    }

    pub async fn index_workspace(
        client: Client,
        parser: Arc<Mutex<Parser>>,
        analysis_map: Arc<RwLock<HashMap<Url, Analysis>>>,
        root_uri: Url,
    ) {
        let root_path = match root_uri.to_file_path() {
            Ok(p) => p,
            Err(_) => return,
        };

        let start_time = Instant::now();
        client
            .log_message(
                MessageType::INFO,
                format!("Starting index of: {:?}", root_path),
            )
            .await;
        let mut count = 0;
        for entry in WalkDir::new(root_path).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path
                .extension()
                .map_or(false, |ext| ext == "vhd" || ext == "vhdl")
            {
                if let Ok(text) = std::fs::read_to_string(path) {
                    let rope = Rope::from_str(&text);
                    if let Ok(uri) = Url::from_file_path(path) {
                        let mut parser_guard = parser.lock().await;

                        if let Some(tree) = parser_guard.parse(&text, None) {
                            let analysis = Analysis::extract(tree.root_node(), &text, &rope);
                            drop(parser_guard);
                            let mut map = analysis_map.write().await;
                            map.insert(uri, analysis);
                            count += 1;
                        }
                    }
                }
            }
        }
        let duration = start_time.elapsed();
        client
            .log_message(
                MessageType::INFO,
                format!("Finished indexing {} files in : {:?}", count, duration),
            )
            .await;
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
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions::default()),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Oxide HDL initialized!")
            .await;
        let mut root_uri = {
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
            let parser = self.parser.clone();
            let map = self.analysis_map.clone();
            tokio::spawn(async move { Backend::index_workspace(client, parser, map, uri).await });
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
        self.on_change(uri, text, rope).await;
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
            let rope_clone = rope.clone();
            let text = rope.to_string();

            drop(map);
            self.on_change(uri, text, rope_clone).await;
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

        if let Some(word) = self.get_word_at_pos(&rope, position) {
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Looking up definition for: {}", word),
                )
                .await;
            let map = self.analysis_map.read().await;
            if let Some(analysis) = map.get(&uri)
                && let Some(symbol) = analysis.symbols.get(&word)
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: uri.clone(),
                    range: symbol.range,
                })));
            }
        }

        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
