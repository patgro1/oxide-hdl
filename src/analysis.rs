//! Analysis module for Oxide HDL.
//!
//! This module defines the core data structures used to represent the VHDL syntax tree
//! and symbol table. It acts as the "Model" in the MVC architecture of the LSP.
//!
//! The central struct is [`Analysis`], which holds the symbol table for a single file.
//! The hierarchical structure of VHDL (Entities containing Ports, Architectures containing Signals)
//! is represented by the recursive [`Symbol`] struct.
//!
use core::fmt;
use std::collections::HashMap;
use tower_lsp::lsp_types::{Range, SymbolKind};
use tree_sitter::Language;

#[allow(dead_code)]
unsafe extern "C" {
    /// External declaration for the tree-sitter-vhdl language function.
    /// This is required to initialize the Tree-sitter parser with the VHDL grammar.
    fn tree_sitter_vhdl() -> Language;
}

/// Represents the semantic kind of a VHDL symbol.
///
/// This enum maps VHDL constructs (like Entity, Signal, Process) to an internal representation
/// that can be easily converted to LSP `SymbolKind` for editor display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OxideSymbolKind {
    /// A VHDL Entity declaration (Interface).
    Entity,
    /// A VHDL Package declaration (Collection of types/constants).
    Package,
    /// A Component declaration.
    Component,
    /// A Component Instantiation statement (Usage of a component).
    ComponentInstantiation, // Note: You might want to rename this to 'Instantiation' to match your other files if needed.
    /// An Interface Port (Input/Output).
    Port,
    /// A Generic parameter.
    Generic,
    /// A VHDL Architecture body (Implementation).
    Architecture,
    /// A Process block.
    Process,
    /// A Block statement.
    Block,
    /// A Generate statement (If/For).
    Generate,
    /// A Record or Type definition.
    Struct,
    /// A Constant value.
    Constant,
    /// A Function or Procedure definition.
    Function,
    /// An internal Signal or Variable.
    Signal,
    /// Fallback for generic classes.
    Class,
}

impl fmt::Display for OxideSymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            OxideSymbolKind::Entity => "entity",
            OxideSymbolKind::Package => "package",
            OxideSymbolKind::Component => "component",
            OxideSymbolKind::ComponentInstantiation => "instantiation",
            OxideSymbolKind::Port => "port",
            OxideSymbolKind::Generic => "generic",
            OxideSymbolKind::Constant => "constant",
            OxideSymbolKind::Architecture => "architecture",
            OxideSymbolKind::Block => "block",
            OxideSymbolKind::Generate => "generate",
            OxideSymbolKind::Process => "process",
            OxideSymbolKind::Function => "function",
            OxideSymbolKind::Struct => "record",
            OxideSymbolKind::Signal => "signal",
            OxideSymbolKind::Class => "class",
        };
        write!(f, "{}", s)
    }
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
            OxideSymbolKind::Class => SymbolKind::CLASS,
        }
    }
}

// Represents a single symbol in the VHDL source code.
///
/// Symbols are hierarchical. For example, an `Architecture` symbol will contain
/// `Signal` and `Process` symbols in its `children` vector.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The name of the symbol as it appears in the source code (original casing).
    pub name: String,
    /// The semantic kind of the symbol.
    pub kind: OxideSymbolKind,
    /// Additional details, such as the type signature (e.g., `std_logic_vector(7 downto 0)`).
    pub detail: Option<String>,
    /// The range in the source document where this symbol is defined.
    pub range: Range,
    /// Nested symbols defined within this symbol's scope.
    pub children: Vec<Symbol>,
}

/// Represents the analysis result for a single VHDL file.
///
/// Contains a lookup map of all top-level symbols found in the file.
#[derive(Debug, Clone)]
pub struct Analysis {
    /// Map of top-level symbol names (normalized to lowercase) to the `Symbol` struct.
    ///
    /// Using lowercase keys ensures case-insensitive lookup, while the `Symbol` struct
    /// preserves the original display name.
    pub symbols: HashMap<String, Symbol>,
}

impl Symbol {
    /// Recursively searches this symbol's children for a specific name.
    ///
    /// This method performs a case-insensitive search.
    ///
    /// # Arguments
    ///
    /// * `target` - The name of the symbol to find (must be lowercase).
    ///
    /// # Returns
    ///
    /// * `Some(&Symbol)` - A reference to the found symbol.
    /// * `None` - If the symbol was not found in the children.
    fn find_recursive<'a>(&'a self, target: &str) -> Option<&'a Symbol> {
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

    /// Alias for `find_recursive` to match the API used in features.
    /// Recursively searches this symbol's children for a specific name.
    pub fn find_child<'a>(&'a self, target: &str) -> Option<&'a Symbol> {
        self.find_recursive(target)
    }

    /// Recursively dumps the symbol hierarchy to a string for debugging purposes.
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
    /// Creates a new, empty Analysis struct.
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
        }
    }

    /// Recursively searches for a symbol by name anywhere in the file's hierarchy.
    ///
    /// This method checks the top-level symbols first, and then recursively searches
    /// the children of all top-level symbols. This allows finding nested signals
    /// inside Architectures or variables inside Processes.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the symbol to find (case-insensitive).
    ///
    /// # Returns
    ///
    /// * `Some(&Symbol)` - A reference to the found symbol.
    /// * `None` - If the symbol was not found.
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
