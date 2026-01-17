mod builders_tests;
mod visible_tests;

use super::*;
use tower_lsp::lsp_types::{Position, Range}; // Used only for Symbol struct creation

use crate::backend::test_utils::SHARED_PARSER_LOCK;
use tree_sitter::Parser;
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
pub fn parse_text(code: &str) -> tree_sitter::Tree {
    let _guard = SHARED_PARSER_LOCK.lock().unwrap();
    let mut parser = Parser::new();
    let lang = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&lang).unwrap();
    parser.parse(code, None).unwrap()
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
