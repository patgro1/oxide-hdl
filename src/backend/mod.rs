pub mod parser;
pub mod scanner;

use crate::logging::log_crash;

use crate::analysis::Analysis;
use ropey::Rope;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, Semaphore};
use tower_lsp::jsonrpc::Result;
use tree_sitter::Parser;
use walkdir::WalkDir;

pub struct Backend {
    client: Client,
    document_map: Arc<RwLock<HashMap<Url, Rope>>>,
    // parser: Arc<Mutex<Parser>>,
    analysis_map: Arc<RwLock<HashMap<Url, Analysis>>>,
    root_uri: Arc<RwLock<Option<Url>>>,
    // shallow_query: Arc<Query>,
}
use tower_lsp::lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, HoverProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, Location, MessageType, OneOf, Position,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer};

impl Backend {
    pub fn new(client: Client, _parser: Parser) -> Self {
        Backend {
            client,
            document_map: Arc::new(RwLock::new(HashMap::new())),
            analysis_map: Arc::new(RwLock::new(HashMap::new())),
            // parser: Arc::new(Mutex::new(parser)),
            root_uri: Arc::new(RwLock::new(None)),
            // shallow_query: Arc::new(shallow_query),
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
        let client = self.client.clone();
        let analysis_map = self.analysis_map.clone();
        let uri_clone = uri.clone();

        tokio::task::spawn_blocking(move || {
            let builder = std::thread::Builder::new().stack_size(128 * 1024 * 1024);
            let thread_result = builder
                .spawn(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut parser = Parser::new();
                        let language = unsafe { crate::tree_sitter_vhdl() };
                        if let Err(_e) = parser.set_language(&language) {
                            return None;
                        }

                        match parser.parse(&text, None) {
                            Some(tree) => {
                                let analysis = Analysis::extract(tree.root_node(), &text, &rope);
                                let diagnostic = Analysis::get_diagnostics(tree, &text);
                                Some(Box::new((analysis, diagnostic)))
                            }
                            None => None,
                        }
                    }))
                })
                .unwrap()
                .join();
            match thread_result {
                Ok(Ok(Some(boxed_result))) => {
                    let (analysis, diagnostics) = *boxed_result;
                    tokio::spawn(async move {
                        client
                            .publish_diagnostics(uri_clone.clone(), diagnostics, None)
                            .await;
                        client
                            .log_message(MessageType::INFO, format!("Re-indexed {}", uri_clone))
                            .await;
                        let mut map = analysis_map.write().await;
                        map.insert(uri_clone, analysis);
                    });
                }
                Ok(Ok(None)) => log_crash("Parser returned None"),
                Ok(Err(e)) => log_crash(&format!("Panic caught: {:?}", e)),
                Err(e) => log_crash(&format!("Thread join failed: {:?}", e)),
            }
        })
        .await
        .unwrap();
    }

    pub async fn index_workspace(
        client: Client,
        analysis_map: Arc<RwLock<HashMap<Url, Analysis>>>,
        root_uri: Url,
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

        let paths: Vec<std::path::PathBuf> = WalkDir::new(root_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "vhd" || ext == "vhdl")
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
            tokio::spawn(async move { Backend::index_workspace(client, map, uri).await });
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
            let target = word.to_lowercase();
            self.client
                .log_message(
                    MessageType::INFO,
                    format!("Looking up definition for: {}", target),
                )
                .await;
            let map = self.analysis_map.read().await;
            for (file_uri, analysis) in map.iter() {
                if let Some(symbol) = analysis.symbols.get(&target) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: file_uri.clone(),
                        range: symbol.range,
                    })));
                }
            }
        }

        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
