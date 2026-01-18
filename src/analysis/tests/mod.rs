mod builders_tests;
mod scope_tree_tests;
mod visible_tests;

use super::*;
use tower_lsp::lsp_types::Range;

pub use crate::backend::test_utils::parse_text;

// --- SETUP HELPERS ---

fn make_symbol(name: &str, kind: OxideSymbolKind, children: Vec<Symbol>) -> Symbol {
    Symbol {
        name: name.to_string(),
        kind,
        range: Range::default(),
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

// =============================================================================
// Analysis lookup helper tests (scope tree based)
// =============================================================================

mod analysis_lookup_tests {
    use crate::analysis::*;
    use crate::backend::test_utils::{make_decl, make_pos, make_range, make_scope};

    // =========================================================================
    // Analysis::find_scope_tree_at tests
    // =========================================================================

    #[test]
    fn test_find_scope_tree_at_in_architecture() {
        let mut analysis = Analysis::new();

        let arch = make_scope(
            ScopeKind::Architecture,
            make_range(10, 0, 50, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        analysis.scope_trees.push(arch);

        let pos = make_pos(30, 5);
        let result = analysis.find_scope_tree_at(&pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().kind, ScopeKind::Architecture));
    }

    #[test]
    fn test_find_scope_tree_at_outside_all() {
        let mut analysis = Analysis::new();

        let arch = make_scope(
            ScopeKind::Architecture,
            make_range(10, 0, 50, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        analysis.scope_trees.push(arch);

        // Position outside the architecture
        let pos = make_pos(100, 5);
        let result = analysis.find_scope_tree_at(&pos);

        assert!(result.is_none());
    }

    #[test]
    fn test_find_scope_tree_at_multiple_architectures() {
        let mut analysis = Analysis::new();

        let mut arch1 = make_scope(
            ScopeKind::Architecture,
            make_range(10, 0, 50, 0),
            vec![make_decl("sig1", DeclType::Signal)],
        );
        arch1.name = Some("arch1".to_string());

        let mut arch2 = make_scope(
            ScopeKind::Architecture,
            make_range(60, 0, 100, 0),
            vec![make_decl("sig2", DeclType::Signal)],
        );
        arch2.name = Some("arch2".to_string());

        analysis.scope_trees.push(arch1);
        analysis.scope_trees.push(arch2);

        // Position in second architecture
        let pos = make_pos(80, 5);
        let result = analysis.find_scope_tree_at(&pos);

        assert!(result.is_some());
        assert_eq!(result.unwrap().name, Some("arch2".to_string()));
    }

    #[test]
    fn test_find_scope_tree_at_in_entity() {
        let mut analysis = Analysis::new();

        // Entity at lines 0-20
        let mut entity = make_scope(
            ScopeKind::Entity,
            make_range(0, 0, 20, 0),
            vec![make_decl("clk", DeclType::Port(PortDirection::In))],
        );
        entity.name = Some("my_entity".to_string());
        analysis
            .entity_scope_trees
            .insert("my_entity".to_string(), entity);

        // Architecture at lines 25-100
        let arch = make_scope(
            ScopeKind::Architecture,
            make_range(25, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        analysis.scope_trees.push(arch);

        // Position inside entity (not architecture)
        let pos = make_pos(10, 5);
        let result = analysis.find_scope_tree_at(&pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().kind, ScopeKind::Entity));
    }

    #[test]
    fn test_find_scope_tree_at_prefers_architecture() {
        // If somehow a position is in both (shouldn't happen in valid VHDL),
        // architecture should be checked first
        let mut analysis = Analysis::new();

        let mut entity = make_scope(
            ScopeKind::Entity,
            make_range(0, 0, 50, 0),
            vec![make_decl("clk", DeclType::Port(PortDirection::In))],
        );
        entity.name = Some("my_entity".to_string());
        analysis
            .entity_scope_trees
            .insert("my_entity".to_string(), entity);

        let arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 50, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        analysis.scope_trees.push(arch);

        let pos = make_pos(25, 5);
        let result = analysis.find_scope_tree_at(&pos);

        assert!(result.is_some());
        // Architecture should win since it's checked first
        assert!(matches!(result.unwrap().kind, ScopeKind::Architecture));
    }

    // =========================================================================
    // Analysis::find_declaration_at tests
    // =========================================================================

    #[test]
    fn test_find_declaration_at_in_current_scope() {
        let mut analysis = Analysis::new();

        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("my_var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("my_sig", DeclType::Signal)],
        );
        arch.children.push(process);
        analysis.scope_trees.push(arch);

        // Find variable from inside process
        let pos = make_pos(30, 5);
        let result = analysis.find_declaration_at("my_var", &pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().decl_type, DeclType::Variable));
    }

    #[test]
    fn test_find_declaration_at_in_parent_scope() {
        let mut analysis = Analysis::new();

        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("my_var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("my_sig", DeclType::Signal)],
        );
        arch.children.push(process);
        analysis.scope_trees.push(arch);

        // Find signal from inside process (should walk up to arch)
        let pos = make_pos(30, 5);
        let result = analysis.find_declaration_at("my_sig", &pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().decl_type, DeclType::Signal));
    }

    #[test]
    fn test_find_declaration_at_in_entity() {
        let mut analysis = Analysis::new();

        // Create entity with port
        let mut entity = make_scope(
            ScopeKind::Entity,
            make_range(0, 0, 10, 0),
            vec![make_decl("clk", DeclType::Port(PortDirection::In))],
        );
        entity.name = Some("my_entity".to_string());
        analysis
            .entity_scope_trees
            .insert("my_entity".to_string(), entity);

        // Create architecture referencing the entity
        let process = make_scope(
            ScopeKind::Process,
            make_range(30, 0, 50, 0),
            vec![make_decl("my_var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(20, 0, 100, 0),
            vec![make_decl("my_sig", DeclType::Signal)],
        );
        arch.entity = Some("my_entity".to_string());
        arch.children.push(process);
        analysis.scope_trees.push(arch);

        // Find port from inside process (should walk up through arch to entity)
        let pos = make_pos(40, 5);
        let result = analysis.find_declaration_at("clk", &pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().decl_type, DeclType::Port(_)));
    }

    #[test]
    fn test_find_declaration_at_not_found() {
        let mut analysis = Analysis::new();

        let arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("my_sig", DeclType::Signal)],
        );
        analysis.scope_trees.push(arch);

        let pos = make_pos(50, 5);
        let result = analysis.find_declaration_at("nonexistent", &pos);

        assert!(result.is_none());
    }

    #[test]
    fn test_find_declaration_at_shadowing() {
        let mut analysis = Analysis::new();

        // Process has variable "data" that shadows architecture signal "data"
        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("data", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("data", DeclType::Signal)],
        );
        arch.children.push(process);
        analysis.scope_trees.push(arch);

        // From inside process, should find the variable (inner scope shadows outer)
        let pos = make_pos(30, 5);
        let result = analysis.find_declaration_at("data", &pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().decl_type, DeclType::Variable));
    }

    #[test]
    fn test_find_declaration_at_case_insensitive() {
        let mut analysis = Analysis::new();

        let arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("MySignal", DeclType::Signal)],
        );
        analysis.scope_trees.push(arch);

        let pos = make_pos(50, 5);

        // Should find regardless of case
        assert!(analysis.find_declaration_at("mysignal", &pos).is_some());
        assert!(analysis.find_declaration_at("MYSIGNAL", &pos).is_some());
        assert!(analysis.find_declaration_at("MySignal", &pos).is_some());
    }

    #[test]
    fn test_find_declaration_at_inside_entity() {
        // When cursor is positioned inside entity declaration itself
        let mut analysis = Analysis::new();

        // Entity with multiple ports
        let mut entity = make_scope(
            ScopeKind::Entity,
            make_range(0, 0, 20, 0),
            vec![
                make_decl("clk", DeclType::Port(PortDirection::In)),
                make_decl("data_in", DeclType::Port(PortDirection::In)),
                make_decl("data_out", DeclType::Port(PortDirection::Out)),
            ],
        );
        entity.name = Some("my_entity".to_string());
        analysis
            .entity_scope_trees
            .insert("my_entity".to_string(), entity);

        // Position inside entity (not in any architecture)
        let pos = make_pos(10, 5);
        let result = analysis.find_declaration_at("data_in", &pos);

        assert!(result.is_some());
        assert!(matches!(result.unwrap().decl_type, DeclType::Port(_)));
    }
}
