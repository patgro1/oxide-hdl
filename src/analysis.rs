use ropey::Rope;
use std::collections::HashMap;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, SymbolKind};
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator, Tree};

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

    pub fn get_diagnostics(tree: Tree, source_code: &str) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        Self::collect_errors(tree.root_node(), &mut diagnostics);

        diagnostics
    }

    fn collect_errors(node: Node, diagnostics: &mut Vec<Diagnostic>) {
        if node.is_error() || node.is_missing() {
            let start = node.start_position();
            let end = node.end_position();

            let range = Range {
                start: Position {
                    line: start.row as u32,
                    character: start.column as u32,
                },
                end: Position {
                    line: end.row as u32,
                    character: end.column as u32,
                },
            };

            let message = if node.is_missing() {
                format!("Missing syntax: expected something her")
            } else {
                format!("Syntax error: Unexpected token")
            };
            diagnostics.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::ERROR),
                code: None,
                code_description: None,
                source: Some("oxide-hdl".to_string()),
                message,
                related_information: None,
                tags: None,
                data: None,
            });
        }
        if node.has_error() {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                Self::collect_errors(child, diagnostics);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn setup_parser() -> Parser {
        let mut parser = Parser::new();
        let language = unsafe { crate::tree_sitter_vhdl() };
        parser
            .set_language(&language)
            .expect("Error loading grammar");
        parser
    }

    #[test]
    fn test_extract_entity() {
        let code = "
            entity my_cpu is
                port (clk : in std_logic);
            end my_cpy
        ";
        let mut parser = setup_parser();
        let tree = parser.parse(code, None).unwrap();
        let rope = Rope::from_str(code);

        let analysis = Analysis::extract(tree.root_node(), code, &rope);

        assert_eq!(analysis.symbols.len(), 1);
        assert!(analysis.symbols.contains_key("my_cpu"));
        let symbol = analysis.symbols.get("my_cpu").unwrap();
        assert_eq!(symbol.kind, SymbolKind::CLASS);
        assert_eq!(symbol.range.start.line, 1);
    }

    #[test]
    fn test_detect_syntax_error() {
        let code = "entity broken is port (";
        let mut parser = setup_parser();
        let tree = parser.parse(code, None).unwrap();
        let diagnostics = Analysis::get_diagnostics(tree, code);
        assert!(!diagnostics.is_empty());
    }
}
