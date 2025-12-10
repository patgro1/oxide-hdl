use std::collections::HashMap;
use tower_lsp::lsp_types::{Range, SymbolKind};
use tree_sitter::Language;

#[allow(dead_code)]
unsafe extern "C" {
    fn tree_sitter_vhdl() -> Language;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OxideSymbolKind {
    Entity,
    Package,
    Component,
    ComponentInstantiation,
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
            OxideSymbolKind::Entity => SymbolKind::INTERFACE,
            OxideSymbolKind::Package => SymbolKind::MODULE,
            OxideSymbolKind::Component => SymbolKind::INTERFACE,
            OxideSymbolKind::ComponentInstantiation => SymbolKind::FIELD,
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

impl Symbol {
    pub fn find_recursive<'a>(&'a self, target: &str) -> Option<&'a Symbol> {
        for child in &self.children {
            if child.name.to_lowercase() == target {
                return Some(child);
            }
            if let Some(found) = child.find_recursive(target) {
                return Some(found);
            }
        }
        None
    }

    // Debugger helper function
    #[allow(dead_code)]
    pub fn dump_symbol_recursive(&self, depth: usize, output: &mut String) {
        let indent = "  ".repeat(depth);
        output.push_str(&format!("{}{:?} {}\n", indent, self.kind, self.name));
        for child in self.children.clone() {
            child.dump_symbol_recursive(depth + 1, output);
        }
    }
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
            if let Some(found) = s.find_recursive(&target) {
                return Some(found);
            }
        }
        None
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
