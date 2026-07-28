mod builders_tests;
mod scope_tree_tests;
mod visible_tests;

pub use crate::backend::test_utils::parse_text;

// --- TEST CASES ---

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

    // =========================================================================
    // has_no_scope_trees — detecting a transiently unparseable buffer
    //
    // Any unclosed construct makes tree-sitter fail to produce an
    // architecture_definition, so extract_document_symbols yields an Analysis
    // with zero scope trees. That signature is what tells the caller not to
    // clobber the last good analysis while the user is mid-keystroke.
    // =========================================================================

    /// Parses VHDL and reports whether the resulting Analysis has any scope trees.
    fn scope_trees_lost(code: &str) -> bool {
        let tree = crate::backend::test_utils::parse_text(code);
        let analysis =
            crate::backend::syntax::parser::extract_document_symbols(code, tree.root_node());
        analysis.has_no_scope_trees()
    }

    const ARCH_HEAD: &str =
        "architecture rtl of top is\n  signal a : bit;\n  signal b : bit;\nbegin\n";

    #[test]
    fn test_complete_architecture_has_scope_trees() {
        assert!(
            !scope_trees_lost(&format!("{ARCH_HEAD}  b <= a;\nend architecture;\n")),
            "a well-formed architecture must produce scope trees"
        );
    }

    #[test]
    fn test_unclosed_process_loses_scope_trees() {
        assert!(scope_trees_lost(&format!(
            "{ARCH_HEAD}  process(a)\n  begin\n    b <= a;\n\nend architecture;\n"
        )));
    }

    #[test]
    fn test_unclosed_if_inside_closed_process_loses_scope_trees() {
        // The nastiest case: the process itself is closed, but an `if` awaiting
        // its `end if;` still collapses the whole architecture.
        assert!(scope_trees_lost(&format!(
            "{ARCH_HEAD}  process(a)\n  begin\n    if a = '1' then\n      b <= a;\n  end process;\nend architecture;\n"
        )));
    }

    #[test]
    fn test_unclosed_generate_loses_scope_trees() {
        assert!(scope_trees_lost(&format!(
            "{ARCH_HEAD}  g: for i in 0 to 3 generate\n  begin\n    b <= a;\n\nend architecture;\n"
        )));
    }

    #[test]
    fn test_unclosed_block_loses_scope_trees() {
        assert!(scope_trees_lost(&format!(
            "{ARCH_HEAD}  blk: block\n  begin\n    b <= a;\n\nend architecture;\n"
        )));
    }

    #[test]
    fn test_missing_end_architecture_loses_scope_trees() {
        assert!(scope_trees_lost(&format!("{ARCH_HEAD}  b <= a;\n")));
    }

    #[test]
    fn test_entity_only_file_is_not_considered_empty() {
        // An entity declaration populates entity_scope_trees, not scope_trees.
        // It must NOT be mistaken for an unparseable buffer.
        assert!(
            !scope_trees_lost("entity top is\n  port (clk : in bit);\nend entity;\n"),
            "an entity-only file has real content and must not look empty"
        );
    }

    #[test]
    fn test_package_only_file_is_not_considered_empty() {
        assert!(
            !scope_trees_lost(
                "package my_pkg is\n  constant C : integer := 1;\nend package;\n"
            ),
            "a package-only file has real content and must not look empty"
        );
    }
}
