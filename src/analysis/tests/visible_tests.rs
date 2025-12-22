mod collect_visible_tests {
    use crate::analysis::*;
    use crate::backend::test_utils::SHARED_PARSER_LOCK;
    use tower_lsp::lsp_types::{Position, Range};
    use tree_sitter::Parser;

    fn make_range(start_line: u32, end_line: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: end_line,
                character: 0,
            },
        }
    }

    fn make_node_info(line: u32) -> NodeInfo {
        NodeInfo { line, column: 0 }
    }

    fn make_decl(name: &str, decl_type: DeclType) -> Declaration {
        Declaration {
            name: name.to_string(),
            decl_type,
            node_info: make_node_info(0),
        }
    }

    fn parse_text(code: &str) -> tree_sitter::Tree {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        parser.parse(code, None).unwrap()
    }

    #[test]
    fn test_target_in_root_scope() {
        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 10),
            name: None,
            entity: None,
            declarations: vec![
                make_decl("arch_sig", DeclType::Signal),
                make_decl("arch_const", DeclType::Constant),
            ],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let target = make_range(0, 10);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 2);
        assert!(decls.iter().any(|d| d.name == "arch_sig"));
        assert!(decls.iter().any(|d| d.name == "arch_const"));
    }

    #[test]
    fn test_target_in_nested_scope() {
        let process = ScopeTree {
            kind: ScopeKind::Process,
            range: make_range(5, 15),
            name: None,
            entity: None,
            declarations: vec![make_decl("proc_var", DeclType::Variable)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 20),
            name: None,
            entity: None,
            declarations: vec![make_decl("arch_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![process],
        };

        let target = make_range(5, 15);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 2);
        // Process var should be first (innermost), arch sig second
        assert_eq!(decls[0].name, "proc_var");
        assert_eq!(decls[1].name, "arch_sig");
    }

    #[test]
    fn test_deeply_nested_three_levels() {
        let process = ScopeTree {
            kind: ScopeKind::Process,
            range: make_range(10, 20),
            name: None,
            entity: None,
            declarations: vec![make_decl("level2", DeclType::Variable)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let generate = ScopeTree {
            kind: ScopeKind::Generate,
            range: make_range(5, 25),
            name: None,
            entity: None,
            declarations: vec![make_decl("level1", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![process],
        };

        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 30),
            name: None,
            entity: None,
            declarations: vec![make_decl("level0", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![generate],
        };

        let target = make_range(10, 20);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 3);
        assert_eq!(decls[0].name, "level2"); // Innermost
        assert_eq!(decls[1].name, "level1"); // Middle
        assert_eq!(decls[2].name, "level0"); // Outermost
    }

    #[test]
    fn test_target_not_in_scope() {
        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 10),
            name: None,
            entity: None,
            declarations: vec![make_decl("sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let target = make_range(50, 60);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_none());
    }

    #[test]
    fn test_sibling_scopes_not_visible() {
        let process = ScopeTree {
            kind: ScopeKind::Process,
            range: make_range(8, 12),
            name: None,
            entity: None,
            declarations: vec![],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let gen1 = ScopeTree {
            kind: ScopeKind::Generate,
            range: make_range(5, 15),
            name: None,
            entity: None,
            declarations: vec![make_decl("gen1_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![process],
        };

        let gen2 = ScopeTree {
            kind: ScopeKind::Generate,
            range: make_range(16, 25),
            name: None,
            entity: None,
            declarations: vec![make_decl("gen2_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 30),
            name: None,
            entity: None,
            declarations: vec![make_decl("arch_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![gen1, gen2],
        };

        let target = make_range(8, 12);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 2);
        assert!(decls.iter().any(|d| d.name == "arch_sig"));
        assert!(decls.iter().any(|d| d.name == "gen1_sig"));
        assert!(!decls.iter().any(|d| d.name == "gen2_sig"));
    }

    #[test]
    fn test_empty_scope_tree() {
        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 10),
            name: None,
            entity: None,
            declarations: vec![],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let target = make_range(0, 10);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 0);
    }

    #[test]
    fn test_multiple_children_only_one_contains_target() {
        let process = ScopeTree {
            kind: ScopeKind::Process,
            range: make_range(18, 22),
            name: None,
            entity: None,
            declarations: vec![make_decl("proc_var", DeclType::Variable)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let gen1 = ScopeTree {
            kind: ScopeKind::Generate,
            range: make_range(5, 15),
            name: None,
            entity: None,
            declarations: vec![make_decl("gen1_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let gen2 = ScopeTree {
            kind: ScopeKind::Generate,
            range: make_range(16, 25),
            name: None,
            entity: None,
            declarations: vec![make_decl("gen2_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![process],
        };

        let gen3 = ScopeTree {
            kind: ScopeKind::Generate,
            range: make_range(26, 35),
            name: None,
            entity: None,
            declarations: vec![make_decl("gen3_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 40),
            name: None,
            entity: None,
            declarations: vec![make_decl("arch_sig", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![gen1, gen2, gen3],
        };

        let target = make_range(18, 22);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 3);
        assert_eq!(decls[0].name, "proc_var");
        assert_eq!(decls[1].name, "gen2_sig");
        assert_eq!(decls[2].name, "arch_sig");
        assert!(!decls.iter().any(|d| d.name == "gen1_sig"));
        assert!(!decls.iter().any(|d| d.name == "gen3_sig"));
    }
    #[test]
    fn test_duplicate_names_both_returned() {
        // Shadowing: process variable "data" shadows arch signal "data"
        // Both should be in the list (caller handles shadowing)

        let process = ScopeTree {
            kind: ScopeKind::Process,
            range: make_range(5, 15),
            name: None,
            entity: None,
            declarations: vec![make_decl("data", DeclType::Variable)],
            local_usage: HashSet::new(),
            children: vec![],
        };

        let arch = ScopeTree {
            kind: ScopeKind::Architecture,
            range: make_range(0, 20),
            name: None,
            entity: None,
            declarations: vec![make_decl("data", DeclType::Signal)],
            local_usage: HashSet::new(),
            children: vec![process],
        };

        let target = make_range(5, 15);
        let result = arch.collect_visible_declarations(&target, None);

        assert!(result.is_some());
        let decls = result.unwrap();
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].name, "data");
        assert!(matches!(decls[0].decl_type, DeclType::Variable));
        assert_eq!(decls[1].name, "data");
        assert!(matches!(decls[1].decl_type, DeclType::Signal));
    }
    #[test]
    fn test_collect_visible_from_process_includes_entity() {
        let code = r#"
entity uart_tx is
generic (
            BAUD_RATE : integer := 9600;
            DATA_BITS : integer := 8
        );
port (
         clk : in std_logic;
         rst : in std_logic;
         tx_data : in std_logic_vector(7 downto 0);
         tx_valid : in std_logic;
         tx_out : out std_logic;
         tx_ready : out std_logic
     );
end entity;
architecture rtl of uart_tx is
constant CONST_A: integer := 0;
constant CONST_V: integer := 0;
constant CONST_Z: integer := 0;
signal toto: std_logic;
begin
p_proc: process() is
    variable xyz: std_logic_vector(31 downto 0);
begin
end process;
end architecture;
"#;

        let tree = parse_text(code);
        let root = tree.root_node();

        // Find entity and architecture nodes
        let mut entity_node = None;
        let mut arch_node = None;

        for node in root.children(&mut root.walk()) {
            if node.kind() == "design_unit" {
                for child in node.children(&mut node.walk()) {
                    match child.kind() {
                        "entity_declaration" => entity_node = Some(child),
                        "architecture_definition" => arch_node = Some(child),
                        _ => {}
                    }
                }
            }
        }

        assert!(entity_node.is_some());
        assert!(arch_node.is_some());

        // Build scopes
        let entity_scope = build_entity_scope_tree(entity_node.unwrap(), code);
        let arch_scope = build_arch_scope_tree(arch_node.unwrap(), code);

        // Find process range
        let process_range = arch_scope
            .children
            .iter()
            .find(|c| matches!(c.kind, ScopeKind::Process))
            .map(|c| c.range)
            .expect("Should find process");

        // Collect visible declarations from architecture
        let all_visible = arch_scope
            .collect_visible_declarations(&process_range, Some(&entity_scope))
            .expect("Should find process in arch scope");

        // Should have: 1 var + 4 arch decls + 2 generics + 6 ports = 13 total
        assert_eq!(all_visible.len(), 13);

        // Check process variable
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "xyz" && matches!(d.decl_type, DeclType::Variable))
        );

        // Check architecture declarations
        assert!(all_visible.iter().any(|d| d.name == "CONST_A"));
        assert!(all_visible.iter().any(|d| d.name == "CONST_V"));
        assert!(all_visible.iter().any(|d| d.name == "CONST_Z"));
        assert!(all_visible.iter().any(|d| d.name == "toto"));

        // Check entity generics
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "BAUD_RATE" && matches!(d.decl_type, DeclType::Generic))
        );
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "DATA_BITS" && matches!(d.decl_type, DeclType::Generic))
        );

        // Check entity ports
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "clk" && matches!(d.decl_type, DeclType::Port(_)))
        );
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "rst" && matches!(d.decl_type, DeclType::Port(_)))
        );
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "tx_data" && matches!(d.decl_type, DeclType::Port(_)))
        );
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "tx_valid" && matches!(d.decl_type, DeclType::Port(_)))
        );
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "tx_out" && matches!(d.decl_type, DeclType::Port(_)))
        );
        assert!(
            all_visible
                .iter()
                .any(|d| d.name == "tx_ready" && matches!(d.decl_type, DeclType::Port(_)))
        );
    }
}
