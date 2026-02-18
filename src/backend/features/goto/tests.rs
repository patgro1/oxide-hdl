#[cfg(test)]
mod tests {
    use crate::backend::features::goto::{lookup_definition, lookup_declaration, lookup_implementation};
    use crate::backend::AnalysisMap;
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::{Position, Url};

    fn setup_workspace(files: Vec<(&str, &str)>) -> (AnalysisMap, Vec<Url>) {
        let mut map = AnalysisMap::new();
        let mut uris = Vec::new();
        
        // 1. Initial extraction for all files
        for (name, content) in &files {
            let uri = Url::parse(&format!("file:///{}", name)).unwrap();
            let tree = parse_text(content);
            let analysis = crate::backend::syntax::parser::extract_document_symbols(content, tree.root_node());
            println!("DEBUG: Analysis for {}: symbols={}, decl_pkgs={}, body_pkgs={}", 
                name, analysis.symbols.len(), analysis.package_declaration_scope_trees.len(), analysis.package_body_scope_trees.len());
            for (pkg_name, tree) in &analysis.package_body_scope_trees {
                println!("  - Package Body '{}' has {} declarations and {} children", 
                    pkg_name, tree.declarations.len(), tree.children.len());
            }
            map.insert(uri.clone(), analysis);
            uris.push(uri);
        }
        
        (map, uris)
    }

    #[test]
    fn test_goto_definition_prioritizes_entity() {
        let files = vec![
            ("pkg.vhd", "package my_pkg is\n  component uart_tx is\n  end component;\nend package;"),
            ("uart.vhd", "entity uart_tx is\nend entity;"),
            ("top.vhd", "library work;\nuse work.my_pkg.all;\narchitecture rtl of top is\nbegin\n  U_TX: entity work.uart_tx;\nend architecture;")
        ];
        let (map, uris) = setup_workspace(files);
        
        // Lookup 'uart_tx' from top.vhd
        let pos = Position { line: 4, character: 20 };
        let locs = lookup_definition("uart_tx", &uris[2], &map, pos);
        
        assert!(!locs.is_empty());
        // Should point to uart.vhd (the entity), not pkg.vhd (the component)
        assert_eq!(locs[0].uri.path(), "/uart.vhd");
    }

    #[test]
    fn test_goto_declaration_prioritizes_component() {
        let files = vec![
            ("pkg.vhd", "package my_pkg is\n  component uart_tx is\n  end component;\nend package;"),
            ("uart.vhd", "entity uart_tx is\nend entity;"),
            ("top.vhd", "library work;\nuse work.my_pkg.all;\narchitecture rtl of top is\nbegin\n  U_TX: uart_tx;\nend architecture;")
        ];
        let (map, uris) = setup_workspace(files);
        
        let pos = Position { line: 4, character: 10 };
        let locs = lookup_declaration("uart_tx", &uris[2], &map, pos);
        
        assert!(!locs.is_empty());
        // Should point to pkg.vhd (the component)
        assert_eq!(locs[0].uri.path(), "/pkg.vhd");
    }

    #[test]
    fn test_goto_implementation_finds_architecture() {
        let files = vec![
            ("uart_ent.vhd", "entity uart_tx is\nend entity;"),
            ("uart_arch.vhd", "architecture rtl of uart_tx is\nbegin\nend architecture;"),
            ("top.vhd", "architecture rtl of top is\nbegin\n  U_TX: entity work.uart_tx;\nend architecture;")
        ];
        let (map, uris) = setup_workspace(files);
        
        let pos = Position { line: 2, character: 20 };
        let locs = lookup_implementation("uart_tx", &uris[2], &map, pos);
        
        assert!(!locs.is_empty());
        // Should point to uart_arch.vhd
        assert_eq!(locs[0].uri.path(), "/uart_arch.vhd");
    }

    #[test]
    fn test_goto_subprogram_body_priority() {
        let files = vec![
            ("pkg.vhd", "package my_pkg is\n  function calc return boolean;\nend package;"),
            ("pkg_body.vhd", "package body my_pkg is\n  function calc return boolean is\n  begin\n    return true;\n  end function;\nend package body;"),
            ("top.vhd", "use work.my_pkg.all;\narchitecture rtl of top is\n  signal s : boolean := calc;\nbegin\nend architecture;")
        ];
        let (map, uris) = setup_workspace(files);
        
        let pos = Position { line: 2, character: 25 };
        let locs = lookup_definition("calc", &uris[2], &map, pos);
        
        assert!(!locs.is_empty());
        // Should point to pkg_body.vhd
        assert_eq!(locs[0].uri.path(), "/pkg_body.vhd");
    }

    #[test]
    fn test_goto_declaration_fallback_to_entity() {
        let files = vec![
            ("uart.vhd", "entity uart_tx is\nend entity;"),
            ("top.vhd", "architecture rtl of top is\nbegin\n  U_TX: entity work.uart_tx;\nend architecture;")
        ];
        let (map, uris) = setup_workspace(files);
        
        let pos = Position { line: 2, character: 20 };
        let locs = lookup_declaration("uart_tx", &uris[1], &map, pos);
        
        assert!(!locs.is_empty());
        // No component found, should fallback to entity in uart.vhd
        assert_eq!(locs[0].uri.path(), "/uart.vhd");
    }
}
