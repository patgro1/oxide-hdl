use ropey::Rope;
use std::collections::HashMap;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, SymbolKind};
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator, Tree};

unsafe extern "C" {
    fn tree_sitter_vhdl() -> Language;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OxideSymbolKind {
    Entity,
    Package,
    Component,
    Port,
    Generic,
    Architecture,
    Process,
    Block,
    Generate,
    Struct,
    Constant,
    Function,
    Signal,
}

impl From<OxideSymbolKind> for SymbolKind {
    fn from(kind: OxideSymbolKind) -> Self {
        match kind {
            OxideSymbolKind::Entity => SymbolKind::CLASS,
            OxideSymbolKind::Package => SymbolKind::MODULE,
            OxideSymbolKind::Component => SymbolKind::INTERFACE,
            OxideSymbolKind::Port => SymbolKind::FIELD,
            OxideSymbolKind::Generic => SymbolKind::CONSTANT,
            OxideSymbolKind::Constant => SymbolKind::CONSTANT,
            OxideSymbolKind::Architecture => SymbolKind::CLASS,
            OxideSymbolKind::Block => SymbolKind::NAMESPACE,
            OxideSymbolKind::Generate => SymbolKind::NAMESPACE,
            OxideSymbolKind::Process => SymbolKind::METHOD,
            OxideSymbolKind::Function => SymbolKind::FUNCTION,
            OxideSymbolKind::Struct => SymbolKind::STRUCT,
            OxideSymbolKind::Signal => SymbolKind::VARIABLE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: OxideSymbolKind,
    pub detail: Option<String>, // i.e std_logic_vector(31 downto 0)
    pub range: Range,
    pub children: Vec<Symbol>, // Ports/generics go here
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

    pub fn find_symbol(&self, name: &str) -> Option<&Symbol> {
        let target = name.to_lowercase();
        if let Some(s) = self.symbols.get(&target) {
            return Some(s);
        }
        for s in self.symbols.values() {
            if let Some(found) = self.find_recursive(s, &target) {
                return Some(found);
            }
        }
        None
    }

    fn find_recursive<'a>(&self, parent: &'a Symbol, target: &str) -> Option<&'a Symbol> {
        for child in &parent.children {
            if child.name.to_lowercase() == *target {
                return Some(child);
            }
            if let Some(found) = self.find_recursive(child, target) {
                return Some(found);
            }
        }
        None
    }

    pub fn extract(root_node: Node, source_code: &str, rope: &Rope) -> Self {
        let mut symbols = HashMap::new();

        // Find all entities declared in the file
        let query_string = "
            (entity_declaration
                entity: (identifier) @entity_name
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
                        kind: OxideSymbolKind::Entity,
                        detail: None,
                        range,
                        children: Vec::new(),
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
    use tower_lsp::lsp_types::{Position, Range}; // Used only for Symbol struct creation

    // --- SETUP HELPERS ---

    fn dummy_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        }
    }

    fn make_symbol(name: &str, kind: OxideSymbolKind, children: Vec<Symbol>) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind,
            range: dummy_range(),
            detail: None,
            children,
        }
    }

    /// Creates a structure like:
    /// Analysis ->
    ///   +-- Key: "top_ent" (Entity)
    ///       +-- Child: "port_a" (Port)
    ///       +-- Child: "port_b" (Port)
    ///   +-- Key: "arch_rtl" (Architecture)
    ///       +-- Child: "sig_x" (Signal)
    ///           +-- Grandchild: "var_z" (Signal)
    fn setup_test_analysis() -> Analysis {
        let mut analysis = Analysis::new();

        // 1. Nested Signal/Variable (Arch -> Signal -> Variable)
        let var_z = make_symbol("var_z", OxideSymbolKind::Signal, vec![]);
        let sig_x = make_symbol("sig_x", OxideSymbolKind::Signal, vec![var_z]);

        // 2. Entity Ports
        let port_a = make_symbol("port_a", OxideSymbolKind::Port, vec![]);
        let port_b = make_symbol("port_b", OxideSymbolKind::Port, vec![]);

        // 3. Architecture (Top Level)
        let arch = make_symbol("Arch_RTL", OxideSymbolKind::Architecture, vec![sig_x]);

        // 4. Entity (Top Level)
        let ent = make_symbol("Top_Ent", OxideSymbolKind::Entity, vec![port_a, port_b]);

        // Insert into the map (keys MUST be lowercase)
        analysis.symbols.insert("arch_rtl".to_string(), arch);
        analysis.symbols.insert("top_ent".to_string(), ent);

        analysis
    }

    // --- TEST CASES ---

    #[test]
    fn test_01_find_top_level() {
        let analysis = setup_test_analysis();

        // Lookup using lowercase target
        let sym = analysis
            .find_symbol("top_ent")
            .expect("Should find Entity at root");

        // Check symbol name (it should preserve original casing)
        assert_eq!(sym.name, "Top_Ent");
        assert_eq!(sym.kind, OxideSymbolKind::Entity);
    }

    #[test]
    fn test_02_find_nested_child() {
        let analysis = setup_test_analysis();

        // Search for a signal inside Architecture (Level 1)
        let sym = analysis
            .find_symbol("sig_x")
            .expect("Should find Signal nested in Architecture");

        assert_eq!(sym.name, "sig_x");
        assert_eq!(sym.kind, OxideSymbolKind::Signal);
    }

    #[test]
    fn test_03_find_deep_nested_child() {
        let analysis = setup_test_analysis();

        // Search for a variable deep inside (Level 2)
        let sym = analysis
            .find_symbol("var_z")
            .expect("Should find Variable nested two levels deep");

        assert_eq!(sym.name, "var_z");
    }

    #[test]
    fn test_04_find_case_insensitive() {
        let analysis = setup_test_analysis();

        // Search using uppercase target for a nested symbol
        let sym = analysis
            .find_symbol("PORT_A")
            .expect("Should find Port regardless of case");

        // The found symbol should preserve its original case
        assert_eq!(sym.name, "port_a");
        assert_eq!(sym.kind, OxideSymbolKind::Port);
    }

    #[test]
    fn test_05_find_not_exists() {
        let analysis = setup_test_analysis();

        // Should return None for a missing symbol
        let sym = analysis.find_symbol("missing_signal");

        assert!(sym.is_none());
    }
}
