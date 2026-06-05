#[cfg(test)]
mod tests {
    use crate::backend::AnalysisMap;
    use crate::backend::features::goto::{
        apply_definition_priority, lookup_declaration, lookup_definition, lookup_implementation,
    };
    use crate::backend::features::lookup::resolve_path_chain;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::{Position, Url};

    // Goto-definition on a qualified name (pkg_a.my_func) must consult only
    // pkg_a's scope tree, not every package that exports a symbol with that name.
    // The Backend handler uses resolve_path_chain + apply_definition_priority;
    // this test validates that integration end-to-end.
    #[test]
    fn test_goto_definition_qualified_pkg_name_no_ambiguity() {
        let files = vec![
            (
                "pkg_a.vhd",
                "package pkg_a is\n  function my_func return boolean;\nend package;",
            ),
            (
                "pkg_b.vhd",
                "package pkg_b is\n  function my_func return boolean;\nend package;",
            ),
            (
                "top.vhd",
                "use work.pkg_a.my_func;\nuse work.pkg_b.my_func;\narchitecture rtl of top is\nbegin\nend architecture;",
            ),
        ];
        let (map, uris) = setup_workspace(files);
        let top_uri = &uris[2];
        let pos = Position { line: 3, character: 0 };

        let results = resolve_path_chain(
            &["pkg_a".to_string(), "my_func".to_string()],
            top_uri,
            &map,
            &pos,
        );
        let locs = apply_definition_priority(results, top_uri);

        assert_eq!(
            locs.len(),
            1,
            "qualified pkg_a.my_func should resolve to exactly one location, got {}",
            locs.len()
        );
        assert!(
            locs[0].uri.path().contains("pkg_a"),
            "expected definition in pkg_a.vhd, got: {}",
            locs[0].uri.path()
        );
    }

    fn setup_workspace(files: Vec<(&str, &str)>) -> (AnalysisMap, Vec<Url>) {
        let mut map = AnalysisMap::new();
        let mut uris = Vec::new();

        // 1. Initial extraction for all files
        for (name, content) in &files {
            let uri = Url::parse(&format!("file:///{}", name)).unwrap();
            let tree = parse_text(content);
            let analysis =
                crate::backend::syntax::parser::extract_document_symbols(content, tree.root_node());
            println!(
                "DEBUG: Analysis for {}: symbols={}, decl_pkgs={}, body_pkgs={}",
                name,
                analysis.symbols.len(),
                analysis.package_declaration_scope_trees.len(),
                analysis.package_body_scope_trees.len()
            );
            for (pkg_name, tree) in &analysis.package_body_scope_trees {
                println!(
                    "  - Package Body '{}' has {} declarations and {} children",
                    pkg_name,
                    tree.declarations.len(),
                    tree.children.len()
                );
            }
            map.insert(uri.clone(), analysis);
            uris.push(uri);
        }

        (map, uris)
    }

    #[test]
    fn test_goto_definition_prioritizes_entity() {
        let files = vec![
            (
                "pkg.vhd",
                "package my_pkg is\n  component uart_tx is\n  end component;\nend package;",
            ),
            ("uart.vhd", "entity uart_tx is\nend entity;"),
            (
                "top.vhd",
                "library work;\nuse work.my_pkg.all;\narchitecture rtl of top is\nbegin\n  U_TX: entity work.uart_tx;\nend architecture;",
            ),
        ];
        let (map, uris) = setup_workspace(files);

        // Lookup 'uart_tx' from top.vhd
        let pos = Position {
            line: 4,
            character: 20,
        };
        let locs = lookup_definition("uart_tx", &uris[2], &map, pos);

        assert!(!locs.is_empty());
        // Should point to uart.vhd (the entity), not pkg.vhd (the component)
        assert_eq!(locs[0].uri.path(), "/uart.vhd");
    }

    #[test]
    fn test_goto_declaration_prioritizes_component() {
        let files = vec![
            (
                "pkg.vhd",
                "package my_pkg is\n  component uart_tx is\n  end component;\nend package;",
            ),
            ("uart.vhd", "entity uart_tx is\nend entity;"),
            (
                "top.vhd",
                "library work;\nuse work.my_pkg.all;\narchitecture rtl of top is\nbegin\n  U_TX: uart_tx;\nend architecture;",
            ),
        ];
        let (map, uris) = setup_workspace(files);

        let pos = Position {
            line: 4,
            character: 10,
        };
        let locs = lookup_declaration("uart_tx", &uris[2], &map, pos);

        assert!(!locs.is_empty());
        // Should point to pkg.vhd (the component)
        assert_eq!(locs[0].uri.path(), "/pkg.vhd");
    }

    #[test]
    fn test_goto_implementation_finds_architecture() {
        let files = vec![
            ("uart_ent.vhd", "entity uart_tx is\nend entity;"),
            (
                "uart_arch.vhd",
                "architecture rtl of uart_tx is\nbegin\nend architecture;",
            ),
            (
                "top.vhd",
                "architecture rtl of top is\nbegin\n  U_TX: entity work.uart_tx;\nend architecture;",
            ),
        ];
        let (map, uris) = setup_workspace(files);

        let pos = Position {
            line: 2,
            character: 20,
        };
        let locs = lookup_implementation("uart_tx", &uris[2], &map, pos);

        assert!(!locs.is_empty());
        // Should point to uart_arch.vhd
        assert_eq!(locs[0].uri.path(), "/uart_arch.vhd");
    }

    #[test]
    fn test_goto_subprogram_body_priority() {
        let files = vec![
            (
                "pkg.vhd",
                "package my_pkg is\n  function calc return boolean;\nend package;",
            ),
            (
                "pkg_body.vhd",
                "package body my_pkg is\n  function calc return boolean is\n  begin\n    return true;\n  end function;\nend package body;",
            ),
            (
                "top.vhd",
                "use work.my_pkg.all;\narchitecture rtl of top is\n  signal s : boolean := calc;\nbegin\nend architecture;",
            ),
        ];
        let (map, uris) = setup_workspace(files);

        let pos = Position {
            line: 2,
            character: 25,
        };
        let locs = lookup_definition("calc", &uris[2], &map, pos);

        assert!(!locs.is_empty());
        // Should point to pkg_body.vhd
        assert_eq!(locs[0].uri.path(), "/pkg_body.vhd");
    }

    #[test]
    fn test_goto_declaration_fallback_to_entity() {
        let files = vec![
            ("uart.vhd", "entity uart_tx is\nend entity;"),
            (
                "top.vhd",
                "architecture rtl of top is\nbegin\n  U_TX: entity work.uart_tx;\nend architecture;",
            ),
        ];
        let (map, uris) = setup_workspace(files);

        let pos = Position {
            line: 2,
            character: 20,
        };
        let locs = lookup_declaration("uart_tx", &uris[1], &map, pos);

        assert!(!locs.is_empty());
        // No component found, should fallback to entity in uart.vhd
        assert_eq!(locs[0].uri.path(), "/uart.vhd");
    }

    #[test]
    fn test_goto_implementation_finds_shallow_architecture() {
        let ent_code = "entity uart_tx is\nend entity;";
        let arch_code = "architecture rtl of uart_tx is\nbegin\nend architecture;";

        let mut map = AnalysisMap::new();

        // Entity: Deep parsed
        let ent_uri = Url::parse("file:///uart_ent.vhd").unwrap();
        let ent_tree = parse_text(ent_code);
        let ent_analysis = crate::backend::syntax::parser::extract_document_symbols(
            ent_code,
            ent_tree.root_node(),
        );
        map.insert(ent_uri.clone(), ent_analysis);

        // Architecture: Shallow parsed (regex only)
        let arch_uri = Url::parse("file:///uart_arch.vhd").unwrap();
        let arch_syms = crate::backend::syntax::scanner::scan_fast(arch_code);
        let mut arch_analysis = crate::analysis::Analysis::new();
        for s in arch_syms {
            arch_analysis
                .symbols
                .insert(s.name.clone().to_lowercase(), s);
        }
        map.insert(arch_uri.clone(), arch_analysis);

        // Target: The entity name
        let pos = Position {
            line: 0,
            character: 7,
        };
        let locs = lookup_implementation("uart_tx", &ent_uri, &map, pos);

        assert!(
            !locs.is_empty(),
            "Should have found the shallow architecture"
        );
        assert_eq!(locs[0].uri.path(), "/uart_arch.vhd");
    }
}
