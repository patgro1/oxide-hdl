//! Tests for ScopeTree lookup helpers

mod lookup_tests {
    use crate::analysis::*;
    use crate::backend::test_utils::{make_decl, make_pos, make_range, make_scope};

    // =========================================================================
    // ScopeTree::get_declaration tests
    // =========================================================================

    #[test]
    fn test_get_declaration_found() {
        let scope = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![
                make_decl("clk", DeclType::Signal),
                make_decl("data", DeclType::Signal),
                make_decl("count", DeclType::Constant),
            ],
        );

        let decl = scope.get_declaration("data");
        assert!(decl.is_some());
        assert_eq!(decl.unwrap().name, "data");
    }

    #[test]
    fn test_get_declaration_not_found() {
        let scope = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("clk", DeclType::Signal)],
        );

        let decl = scope.get_declaration("missing");
        assert!(decl.is_none());
    }

    #[test]
    fn test_get_declaration_case_insensitive() {
        let scope = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("MySignal", DeclType::Signal)],
        );

        // Should find regardless of case
        assert!(scope.get_declaration("mysignal").is_some());
        assert!(scope.get_declaration("MYSIGNAL").is_some());
        assert!(scope.get_declaration("MySignal").is_some());
    }

    #[test]
    fn test_get_declaration_empty_scope() {
        let scope = make_scope(ScopeKind::Process, make_range(0, 0, 10, 0), vec![]);

        assert!(scope.get_declaration("anything").is_none());
    }

    // =========================================================================
    // ScopeTree::find_innermost_scope tests
    // =========================================================================

    #[test]
    fn test_find_innermost_scope_no_children() {
        let scope = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );

        let pos = make_pos(50, 10);
        let innermost = scope.find_innermost_scope(&pos);

        // Should return self when no children
        assert!(matches!(innermost.kind, ScopeKind::Architecture));
    }

    #[test]
    fn test_find_innermost_scope_in_child() {
        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        arch.children.push(process);

        // Position inside the process
        let pos = make_pos(30, 10);
        let innermost = arch.find_innermost_scope(&pos);

        assert!(matches!(innermost.kind, ScopeKind::Process));
    }

    #[test]
    fn test_find_innermost_scope_outside_children() {
        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        arch.children.push(process);

        // Position outside the process but inside arch
        let pos = make_pos(50, 10);
        let innermost = arch.find_innermost_scope(&pos);

        assert!(matches!(innermost.kind, ScopeKind::Architecture));
    }

    #[test]
    fn test_find_innermost_scope_deeply_nested() {
        let inner_process = make_scope(
            ScopeKind::Process,
            make_range(30, 0, 35, 0),
            vec![make_decl("inner_var", DeclType::Variable)],
        );

        let mut generate = make_scope(
            ScopeKind::Generate,
            make_range(20, 0, 40, 0),
            vec![make_decl("gen_sig", DeclType::Signal)],
        );
        generate.children.push(inner_process);

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("arch_sig", DeclType::Signal)],
        );
        arch.children.push(generate);

        // Position inside the innermost process
        let pos = make_pos(32, 5);
        let innermost = arch.find_innermost_scope(&pos);

        assert!(matches!(innermost.kind, ScopeKind::Process));
    }

    #[test]
    fn test_find_innermost_scope_sibling_scopes() {
        let process1 = make_scope(
            ScopeKind::Process,
            make_range(10, 0, 20, 0),
            vec![make_decl("var1", DeclType::Variable)],
        );

        let process2 = make_scope(
            ScopeKind::Process,
            make_range(30, 0, 40, 0),
            vec![make_decl("var2", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        arch.children.push(process1);
        arch.children.push(process2);

        // Position in second process
        let pos = make_pos(35, 5);
        let innermost = arch.find_innermost_scope(&pos);

        // Should find process2, not process1
        assert!(matches!(innermost.kind, ScopeKind::Process));
        assert!(innermost.get_declaration("var2").is_some());
        assert!(innermost.get_declaration("var1").is_none());
    }

    // =========================================================================
    // ScopeTree::collect_scope_chain tests
    // =========================================================================

    #[test]
    fn test_collect_scope_chain_no_children() {
        let scope = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );

        let pos = make_pos(50, 10);
        let chain = scope.collect_scope_chain(&pos);

        assert_eq!(chain.len(), 1);
        assert!(matches!(chain[0].kind, ScopeKind::Architecture));
    }

    #[test]
    fn test_collect_scope_chain_one_level() {
        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        arch.children.push(process);

        let pos = make_pos(30, 10);
        let chain = arch.collect_scope_chain(&pos);

        // Outer to inner: [arch, process]
        assert_eq!(chain.len(), 2);
        assert!(matches!(chain[0].kind, ScopeKind::Architecture));
        assert!(matches!(chain[1].kind, ScopeKind::Process));
    }

    #[test]
    fn test_collect_scope_chain_deeply_nested() {
        let inner_process = make_scope(
            ScopeKind::Process,
            make_range(30, 0, 35, 0),
            vec![make_decl("inner_var", DeclType::Variable)],
        );

        let mut generate = make_scope(
            ScopeKind::Generate,
            make_range(20, 0, 40, 0),
            vec![make_decl("gen_sig", DeclType::Signal)],
        );
        generate.children.push(inner_process);

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("arch_sig", DeclType::Signal)],
        );
        arch.children.push(generate);

        let pos = make_pos(32, 5);
        let chain = arch.collect_scope_chain(&pos);

        // Outer to inner: [arch, generate, process]
        assert_eq!(chain.len(), 3);
        assert!(matches!(chain[0].kind, ScopeKind::Architecture));
        assert!(matches!(chain[1].kind, ScopeKind::Generate));
        assert!(matches!(chain[2].kind, ScopeKind::Process));
    }

    #[test]
    fn test_collect_scope_chain_outside_children() {
        let process = make_scope(
            ScopeKind::Process,
            make_range(20, 0, 40, 0),
            vec![make_decl("var", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        arch.children.push(process);

        // Position outside process but inside arch
        let pos = make_pos(50, 10);
        let chain = arch.collect_scope_chain(&pos);

        // Only arch, since position isn't in the process
        assert_eq!(chain.len(), 1);
        assert!(matches!(chain[0].kind, ScopeKind::Architecture));
    }

    #[test]
    fn test_collect_scope_chain_selects_correct_sibling() {
        let process1 = make_scope(
            ScopeKind::Process,
            make_range(10, 0, 20, 0),
            vec![make_decl("var1", DeclType::Variable)],
        );

        let process2 = make_scope(
            ScopeKind::Process,
            make_range(30, 0, 40, 0),
            vec![make_decl("var2", DeclType::Variable)],
        );

        let mut arch = make_scope(
            ScopeKind::Architecture,
            make_range(0, 0, 100, 0),
            vec![make_decl("sig", DeclType::Signal)],
        );
        arch.children.push(process1);
        arch.children.push(process2);

        // Position in second process
        let pos = make_pos(35, 5);
        let chain = arch.collect_scope_chain(&pos);

        assert_eq!(chain.len(), 2);
        assert!(matches!(chain[0].kind, ScopeKind::Architecture));
        assert!(matches!(chain[1].kind, ScopeKind::Process));
        // Verify it's process2 by checking its declaration
        assert!(chain[1].get_declaration("var2").is_some());
    }
}
