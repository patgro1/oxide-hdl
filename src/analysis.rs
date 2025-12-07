use ropey::Rope;
use std::collections::HashMap;
use tower_lsp::lsp_types::{Position, Range, SymbolKind};
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator};

unsafe extern "C" {
    fn tree_sitter_vhdl() -> Language;
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    // Key: Name of the symbol
    // Val: The analyzed symbol
    pub symbols: HashMap<String, Symbol>,
}

impl Analysis {
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
        }
    }

    pub fn extract(root_node: Node, source_code: &str, rope: &Rope) -> Self {
        let mut symbols = HashMap::new();

        // Find all entities declared in the file
        let query_string = "
            (entity_declaration
                name: (identifier) @entity_name
            )
        ";
        let language = unsafe { tree_sitter_vhdl() };
        let query = Query::new(&language, query_string).expect("Invalid query");
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root_node, source_code.as_bytes());
        while let Some(m) = &matches.next() {
            for capture in m.captures.iter() {
                let node = capture.node;
                if let Ok(name_text) = node.utf8_text(source_code.as_bytes()) {
                    let name_text = name_text.to_string();
                    let start_line = node.start_position().row;
                    let start_col = node.start_position().column;
                    let end_line = node.end_position().row;
                    let end_col = node.end_position().column;

                    let range = Range {
                        start: Position {
                            line: start_line as u32,
                            character: start_col as u32,
                        },
                        end: Position {
                            line: end_line as u32,
                            character: end_col as u32,
                        },
                    };
                    let symbol = Symbol {
                        name: name_text.clone(),
                        kind: SymbolKind::CLASS,
                        range,
                    };
                    symbols.insert(name_text, symbol);
                }
            }
        }

        Analysis { symbols }
    }
}
