pub mod parser;
pub mod scanner;

use crate::config::OxideConfig;

use crate::analysis::{Analysis, Symbol};
use ropey::Rope;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tower_lsp::jsonrpc::Result;
use tree_sitter::Parser;
use walkdir::WalkDir;

pub struct Backend {
    client: Client,
    config: Arc<RwLock<Option<OxideConfig>>>,
    document_map: Arc<RwLock<HashMap<Url, Rope>>>,
    parser: Arc<Mutex<Parser>>,
    analysis_map: Arc<RwLock<HashMap<Url, Analysis>>>,
    root_uri: Arc<RwLock<Option<Url>>>,
    // shallow_query: Arc<Query>,
}
use tower_lsp::lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, Location, MessageType, OneOf, Position, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer};

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

    fn format_instantiation_hover(&self, instance_name: &str, definition: &Symbol) -> String {
        use crate::analysis::OxideSymbolKind;

        let mut md = String::new();
        // Title: "inst_ent (Instaance of entity)"
        md.push_str(&format!(
            "**{}** (Instance of `{}`)\n\n",
            instance_name, definition.name
        ));
        md.push_str("```vhdl\n");
        // Pseudo header "entity ent is"
        md.push_str(&format!("entity {} is\n", definition.name));

        // Generics
        let generics: Vec<&Symbol> = definition
            .children
            .iter()
            .filter(|c| c.kind == OxideSymbolKind::Generic || c.kind == OxideSymbolKind::Constant)
            .collect();
        if !generics.is_empty() {
            md.push_str("generics (\n");
            for (i, g) in generics.iter().enumerate() {
                let type_info = g.detail.as_deref().unwrap_or("?");
                let sep = if i < generics.len() - 1 { ";" } else { "" };
                md.push_str(&format!("    {} : {}{}\n", g.name, type_info, sep));
            }
            md.push_str(");\n");
        }
        // Ports
        let ports: Vec<&Symbol> = definition
            .children
            .iter()
            .filter(|c| c.kind == OxideSymbolKind::Port)
            .collect();
        if !ports.is_empty() {
            md.push_str("ports (\n");
            for (i, p) in ports.iter().enumerate() {
                let type_info = p.detail.as_deref().unwrap_or("?");
                let sep = if i < generics.len() - 1 { ";" } else { "" };
                md.push_str(&format!("    {} : {}{}\n", p.name, type_info, sep));
            }
            md.push_str(");\n");
        }

        md.push_str("end entity;\n");
        md.push_str("```");
        md
    }

    async fn on_change(&self, uri: Url, text: String) {
        let analysis_map = self.analysis_map.clone();
        let uri_clone = uri.clone();
        let parser_arc = self.parser.clone();

        tokio::task::spawn_blocking(move || {
            let builder = std::thread::Builder::new().stack_size(128 * 1024 * 1024);
            let thread_result = builder
                .spawn(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let tree = {
                            let mut parser = parser_arc.blocking_lock();
                            let language = unsafe { crate::tree_sitter_vhdl() };
                            let _ = parser.set_language(&language);
                            parser.parse(&text, None)
                        };
                        match tree {
                            Some(t) => {
                                let analysis =
                                    parser::extract_document_symbols(&text, t.root_node());

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
                    map.insert(uri_clone, analysis);

                    //client.publish_diagnostics(uri, diags, version)
                });
            }
        })
        .await
        .unwrap();
    }

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

    async fn ensure_fully_parsed(&self, uri: &Url) {
        // Check if the file was shallow index or fully parsed
        let needs_parsing = {
            let map = self.analysis_map.read().await;
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
        self.client
            .log_message(
                MessageType::INFO,
                format!(" JIT Parsing (Rich Hover): {}", uri),
            )
            .await;

        let path = match uri.to_file_path() {
            Ok(t) => t,
            Err(_) => return,
        };

        let parser_arc = self.parser.clone();
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
            let mut map = self.analysis_map.write().await;
            map.insert(uri.clone(), analysis);
        }
    }

    fn markup(&self, text: String) -> Result<Option<tower_lsp::lsp_types::Hover>> {
        Ok(Some(tower_lsp::lsp_types::Hover {
            contents: tower_lsp::lsp_types::HoverContents::Markup(
                tower_lsp::lsp_types::MarkupContent {
                    kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                    value: text.to_string(),
                },
            ),
            range: None,
        }))
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
            tokio::spawn(async move { Backend::index_workspace(client, map, uri, config).await });
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

        if let Some(word) = self.get_word_at_pos(&rope, position) {
            let target = word.to_lowercase();
            self.client
                .log_message(MessageType::INFO, format!("Looking for: '{}'", target))
                .await;

            let map = self.analysis_map.read().await;
            if let Some(analysis) = map.get(&uri)
                && let Some(sym) = analysis.find_symbol(&target)
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: uri.clone(),
                    range: sym.range,
                })));
            }
            for (file_uri, analysis) in map.iter() {
                if let Some(symbol) = analysis.symbols.get(&target) {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: file_uri.clone(),
                        range: symbol.range,
                    })));
                }
            }
            // Print the first 10 keys in the database to see what IS there
            let mut all_keys: Vec<String> = Vec::new();
            for analysis in map.values() {
                all_keys.extend(analysis.symbols.keys().take(3).cloned());
                if all_keys.len() > 20 {
                    break;
                }
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

        if let Some(word) = self.get_word_at_pos(&rope, position) {
            let target = word.to_lowercase();

            let mut found_symbol: Option<Symbol> = None;
            let mut found_uri: Option<Url> = None;

            let map_guard = self.analysis_map.read().await;
            // Local indexing
            if let Some(analysis) = map_guard.get(&uri)
                && let Some(sym) = analysis.find_symbol(&target)
            {
                found_symbol = Some(sym.clone());
                found_uri = Some(uri.clone());
            }
            // Global indexing
            if found_symbol.is_none() {
                for (f_uri, analysis) in map_guard.iter() {
                    if let Some(sym) = analysis.symbols.get(&target) {
                        found_symbol = Some(sym.clone());
                        found_uri = Some(f_uri.clone());
                        break;
                    }
                }
            }
            drop(map_guard);

            if let Some(sym) = found_symbol {
                // Hovering the instance name
                if sym.kind == crate::analysis::OxideSymbolKind::ComponentInstantiation
                    && let Some(target_name) = &sym.detail
                {
                    let def_key = target_name.to_lowercase();

                    // Find definition location
                    let mut def_uri: Option<Url> = None;
                    {
                        let map = self.analysis_map.read().await;
                        for (f_uri, f_analysis) in map.iter() {
                            if f_analysis.symbols.contains_key(&def_key) {
                                def_uri = Some(f_uri.clone());
                                break;
                            }
                        }
                    }
                    // Validate if we already have fully parsed the file,
                    // if not do it (JIT parsing)
                    if let Some(d_uri) = def_uri {
                        self.ensure_fully_parsed(&d_uri).await;

                        let map = self.analysis_map.read().await;
                        if let Some(analysis) = map.get(&d_uri)
                            && let Some(def_sym) = analysis.symbols.get(&def_key)
                        {
                            let hover_text = self.format_instantiation_hover(&sym.name, def_sym);
                            return self.markup(hover_text);
                        }
                    }
                }
                // Hovering the instance entity
                if (sym.kind == crate::analysis::OxideSymbolKind::Entity
                    || sym.kind == crate::analysis::OxideSymbolKind::Component)
                    && let Some(def_uri) = found_uri
                {
                    self.ensure_fully_parsed(&def_uri).await;

                    let map = self.analysis_map.read().await;
                    if let Some(analysis) = map.get(&def_uri) {
                        let lookup_key = sym.name.to_lowercase();
                        if let Some(rich_sym) = analysis.symbols.get(&lookup_key) {
                            let hover_text =
                                self.format_instantiation_hover(&rich_sym.name, rich_sym);
                            return self.markup(hover_text);
                        }
                    }
                }
                let type_info = sym.detail.as_deref().unwrap_or("");
                let markdown = format!(
                    "**{}**\n\n```vhdl\n{}  :  {}\n```",
                    sym.name, sym.kind, type_info
                );
                return self.markup(markdown);
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
