//! Ast utilities
//!
//! Contains functions to help navigate and extract specific data from the AST

use tree_sitter::Node;

/// Navigate an ast from node to node using the array of string to find the next kind to go through
///
///
/// # Arguments
///
/// * `node` - Starting node
/// * `path` - Array of str containing the path to follow
///
/// # Returns
/// Some(node) if we successfully reach the end of the array
/// None otherwise
pub fn navigate_path<'a>(node: Node<'a>, path: &[&str]) -> Option<Node<'a>> {
    let mut current = node;
    for kind in path {
        current = current
            .children(&mut current.walk())
            .find(|n| n.kind() == *kind)?;
    }
    Some(current)
}

/// Find the first descendant of specific kind (DFS)
///
/// # Arguments
///
/// * `node` - Starting node
/// * `kind` - Kind of node we want to find
///
/// Returns
/// Some(node) if we find a descendant of the specific kind
/// None otherwise
pub fn find_descendant<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }

    for child in node.children(&mut node.walk()) {
        if let Some(found) = find_descendant(child, kind) {
            return Some(found);
        }
    }
    None
}

/// Collect all descendant of the specific kind
///
/// # Arguments
///
/// * `node` - Starting node
/// * `kind` - Kind of the node we are looking for
///
/// Returns
/// Vector with all the nodes
pub fn collect_descendants<'a>(node: Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut results = Vec::new();
    if node.kind() == kind {
        results.push(node);
    }
    for child in node.children(&mut node.walk()) {
        results.extend(collect_descendants(child, kind));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::test_utils::SHARED_PARSER_LOCK;
    use tree_sitter::Parser;

    fn parse_text(code: &str) -> tree_sitter::Tree {
        let _guard = SHARED_PARSER_LOCK.lock().unwrap();
        let mut parser = Parser::new();
        let lang = unsafe { crate::tree_sitter_vhdl() };
        parser.set_language(&lang).unwrap();
        parser.parse(code, None).unwrap()
    }

    /// Helper to parse VHDL and get root node
    fn parse_vhdl(code: &str) -> tree_sitter::Tree {
        parse_text(code)
    }

    // ==================== navigate_path tests ====================

    #[test]
    fn test_navigate_path_simple() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Navigate to port clause
        let result = navigate_path(
            root,
            &[
                "design_unit",
                "entity_declaration",
                "entity_head",
                "port_clause",
            ],
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().kind(), "port_clause");
    }

    #[test]
    fn test_navigate_path_deep() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Navigate deep path
        let result = navigate_path(
            root,
            &[
                "design_unit",
                "entity_declaration",
                "entity_head",
                "port_clause",
                "interface_list",
                "interface_declaration",
            ],
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().kind(), "interface_declaration");
    }

    #[test]
    fn test_navigate_path_not_found() {
        let code = r#"
entity test is
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Try to navigate to non-existent port clause
        let result = navigate_path(root, &["design_unit", "entity_declaration", "port_clause"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_navigate_path_wrong_kind() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Wrong kind in path
        let result = navigate_path(root, &["design_unit", "architecture_body"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_navigate_path_empty() {
        let code = "entity test is end entity;";
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Empty path should return original node
        let result = navigate_path(root, &[]);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), root);
    }

    // ==================== find_descendant tests ====================

    #[test]
    fn test_find_descendant_immediate_child() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Find design_unit (immediate child)
        let result = find_descendant(root, "design_unit");
        assert!(result.is_some());
        assert_eq!(result.unwrap().kind(), "design_unit");
    }

    #[test]
    fn test_find_descendant_deep() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Find identifier (deep in the tree)
        let result = find_descendant(root, "identifier");
        assert!(result.is_some());
        assert_eq!(result.unwrap().kind(), "identifier");
    }

    #[test]
    fn test_find_descendant_returns_first() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        rst : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Should find first identifier (clk, not rst)
        let result = find_descendant(root, "identifier");
        assert!(result.is_some());
        // Note: Can't easily check which identifier without getting text
    }

    #[test]
    fn test_find_descendant_not_found() {
        let code = "entity test is end entity;";
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Try to find non-existent node type
        let result = find_descendant(root, "process_statement");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_descendant_finds_self() {
        let code = "entity test is end entity;";
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Should find itself if kind matches
        let result = find_descendant(root, "design_file");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), root);
    }

    // ==================== collect_descendants tests ====================

    #[test]
    fn test_collect_descendants_multiple() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        rst : in std_logic;
        data : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Collect all identifiers
        let results = collect_descendants(root, "identifier");
        assert!(
            results.len() >= 3,
            "Should find at least 3 identifiers (clk, rst, data)"
        );
    }

    #[test]
    fn test_collect_descendants_none() {
        let code = "entity test is end entity;";
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Collect non-existent node type
        let results = collect_descendants(root, "process_statement");
        assert!(results.is_empty());
    }

    #[test]
    fn test_collect_descendants_single() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Collect port_clause (should be exactly one)
        let results = collect_descendants(root, "port_clause");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind(), "port_clause");
    }

    #[test]
    fn test_collect_descendants_nested() {
        let code = r#"
architecture rtl of test is
begin
    process
        variable x : integer;
    begin
        x := 1;
    end process;
    
    process
        variable y : integer;
    begin
        y := 2;
    end process;
end architecture;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Collect all process_statement nodes
        let results = collect_descendants(root, "process_statement");
        assert_eq!(results.len(), 2, "Should find 2 process statements");
    }

    #[test]
    fn test_collect_descendants_includes_root() {
        let code = "entity test is end entity;";
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // If searching for root's kind, it should include itself
        let results = collect_descendants(root, "design_file");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], root);
    }

    // ==================== Integration tests ====================

    #[test]
    fn test_navigate_then_collect() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        rst : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // Navigate to port clause, then collect all identifiers
        let port_clause = navigate_path(
            root,
            &[
                "design_unit",
                "entity_declaration",
                "entity_head",
                "port_clause",
            ],
        )
        .expect("Should find port_clause");

        let identifiers = collect_descendants(port_clause, "identifier");
        assert!(identifiers.len() >= 2, "Should find at least clk and rst");
    }

    #[test]
    fn test_find_descendant_vs_collect() {
        let code = r#"
entity test is
    port (
        a, b, c : in std_logic
    );
end entity;
"#;
        let tree = parse_vhdl(code);
        let root = tree.root_node();

        // find_descendant returns first
        let first = find_descendant(root, "identifier");
        assert!(first.is_some());

        // collect_descendants returns all
        let all = collect_descendants(root, "identifier");
        assert!(all.len() >= 3);

        // First from collect should match find_descendant
        assert_eq!(first.unwrap(), all[0]);
    }
}
