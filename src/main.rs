mod analysis;
use std::collections::HashMap;
use tokio::sync::{Mutex, RwLock};
use tree_sitter::{Language, Parser};

use ropey::Rope;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, Location, MessageType, OneOf, Position,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::analysis::Analysis;

unsafe extern "C" {
    fn tree_sitter_vhdl() -> Language;
}

pub struct Backend {
    client: Client,
    document_map: RwLock<HashMap<Url, Rope>>,
    parser: Mutex<Parser>,
    analysis_map: RwLock<HashMap<Url, Analysis>>,
}

impl Backend {
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
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
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
            .log_message(MessageType::INFO, "Oxide HDL is initializing!")
            .await;
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

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut parser = Parser::new();
    let language = unsafe { tree_sitter_vhdl() };

    parser
        .set_language(&language)
        .expect("Error loading VHDL grammar");

    let (lsp_service, socket) = LspService::new(|client| Backend {
        client,
        document_map: RwLock::new(HashMap::new()),
        analysis_map: RwLock::new(HashMap::new()),
        parser: Mutex::new(parser),
    });

    Server::new(stdin, stdout, socket).serve(lsp_service).await;
}
