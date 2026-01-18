//! Ast utilities
//!
//! Contains functions to help navigate and extract specific data from the AST

use tree_sitter::Node;

/// Find the first direct child of specific kind
///
/// # Arguments
///
/// * `node` - Parent Node
/// * `kind` - Kind of the node we are looking for
///
/// # Returns
/// Some(node) if we found a direct child of specific kind
/// None otherwise
pub fn find_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|&child| child.kind() == kind)
}

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
/// # Returns
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
/// # Returns
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

/// Collect all direct descendants of a specific kind
///
/// # Arguments
///
/// * `node` - Parent node
/// * `kind` - Kind of the node we are looking for
///
/// # Returns
/// Vector containing all the nodes
pub fn collect_children<'a>(node: Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut results = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == kind {
            results.push(child);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::test_utils::parse_text;

    // ==================== navigate_path tests ====================

    #[test]
    fn test_navigate_path_simple_and_deep() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Navigate to port clause (shallow)
        let port_clause = navigate_path(
            root,
            &[
                "design_unit",
                "entity_declaration",
                "entity_head",
                "port_clause",
            ],
        );
        assert!(port_clause.is_some());
        assert_eq!(port_clause.unwrap().kind(), "port_clause");

        // Navigate deeper to interface_declaration
        let interface = navigate_path(
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
        assert!(interface.is_some());
        assert_eq!(interface.unwrap().kind(), "interface_declaration");
    }

    #[test]
    fn test_navigate_path_not_found() {
        let code = r#"
entity test is
end entity;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Try to navigate to non-existent port clause
        let result = navigate_path(
            root,
            &[
                "design_unit",
                "entity_declaration",
                "entity_head",
                "port_clause",
            ],
        );
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
        let tree = parse_text(code);
        let root = tree.root_node();

        // Wrong kind in path
        let result = navigate_path(root, &["design_unit", "architecture_body"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_navigate_path_empty() {
        let code = "entity test is end entity;";
        let tree = parse_text(code);
        let root = tree.root_node();

        // Empty path should return original node
        let result = navigate_path(root, &[]);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), root);
    }

    // ==================== find_descendant tests ====================

    #[test]
    fn test_find_descendant_immediate_and_deep() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Find design_unit (immediate child)
        let design_unit = find_descendant(root, "design_unit");
        assert!(design_unit.is_some());
        assert_eq!(design_unit.unwrap().kind(), "design_unit");

        // Find identifier (deep in the tree)
        let identifier = find_descendant(root, "identifier");
        assert!(identifier.is_some());
        assert_eq!(identifier.unwrap().kind(), "identifier");
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
        let tree = parse_text(code);
        let root = tree.root_node();

        // Should find first identifier (clk, not rst)
        let result = find_descendant(root, "identifier");
        assert!(result.is_some());
        // Note: Can't easily check which identifier without getting text
    }

    #[test]
    fn test_find_descendant_not_found() {
        let code = "entity test is end entity;";
        let tree = parse_text(code);
        let root = tree.root_node();

        // Try to find non-existent node type
        let result = find_descendant(root, "process_statement");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_descendant_finds_self() {
        let code = "entity test is end entity;";
        let tree = parse_text(code);
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
        let tree = parse_text(code);
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
        let tree = parse_text(code);
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
        let tree = parse_text(code);
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
        let tree = parse_text(code);
        let root = tree.root_node();

        // Collect all process_statement nodes
        let results = collect_descendants(root, "process_statement");
        assert_eq!(results.len(), 2, "Should find 2 process statements");
    }

    #[test]
    fn test_collect_descendants_includes_root() {
        let code = "entity test is end entity;";
        let tree = parse_text(code);
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
        let tree = parse_text(code);
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
        let tree = parse_text(code);
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
    #[test]
    fn test_collect_children_multiple() {
        let code = r#"
entity test is
    port (
        clk : in std_logic;
        rst : in std_logic;
        data : in std_logic
    );
end entity;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Navigate to interface_list
        let interface_list = navigate_path(
            root,
            &[
                "design_unit",
                "entity_declaration",
                "entity_head",
                "port_clause",
                "interface_list",
            ],
        )
        .expect("Should find interface_list");

        // Collect all interface_declaration children
        let declarations = collect_children(interface_list, "interface_declaration");
        assert_eq!(
            declarations.len(),
            3,
            "Should find 3 interface declarations"
        );
    }

    #[test]
    fn test_collect_children_none() {
        let code = "entity test is end entity;";
        let tree = parse_text(code);
        let root = tree.root_node();

        // Try to collect non-existent children
        let results = collect_children(root, "port_clause");
        assert!(
            results.is_empty(),
            "Should find no port clauses at root level"
        );
    }

    #[test]
    fn test_collect_children_single() {
        let code = r#"
entity test is
    port (
        clk : in std_logic
    );
end entity;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Collect design_unit (should be exactly one immediate child)
        let design_units = collect_children(root, "design_unit");
        assert_eq!(design_units.len(), 1);
    }

    #[test]
    fn test_collect_children_not_recursive() {
        let code = r#"
architecture rtl of test is
begin
    gen1: if true generate
    begin
        process
        begin
            wait;
        end process;
    end generate;
    
    gen2: if true generate
    begin
        process
        begin
            wait;
        end process;
    end generate;
end architecture;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Navigate to concurrent_block
        let concurrent_block =
            find_descendant(root, "concurrent_block").expect("Should find concurrent_block");

        // collect_children should only find immediate if_generate_statements (2)
        let immediate_generates = collect_children(concurrent_block, "if_generate_statement");

        // collect_descendants would find nested ones too (but there aren't any in this case)
        let all_generates = collect_descendants(concurrent_block, "if_generate_statement");

        assert_eq!(
            immediate_generates.len(),
            2,
            "Should find 2 immediate generates"
        );
        assert_eq!(all_generates.len(), 2, "Should find 2 total generates");
    }

    #[test]
    fn test_collect_children_vs_descendants_nested() {
        let code = r#"
architecture rtl of test is
begin
    outer_block: block
    begin
        inner_block: block
        begin
        end block;
    end block;
end architecture;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        let concurrent_block =
            find_descendant(root, "concurrent_block").expect("Should find concurrent_block");

        // collect_children finds only immediate block_statement (outer)
        let immediate_blocks = collect_children(concurrent_block, "block_statement");

        // collect_descendants finds all nested blocks (outer + inner)
        let all_blocks = collect_descendants(concurrent_block, "block_statement");

        assert_eq!(immediate_blocks.len(), 1, "Should find only outer block");
        assert_eq!(
            all_blocks.len(),
            2,
            "Should find both outer and inner blocks"
        );
    }

    #[test]
    fn test_collect_children_with_mixed_kinds() {
        let code = r#"
architecture rtl of test is
    signal a : std_logic;
    constant b : integer := 5;
    signal c : std_logic;
begin
end architecture;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        let arch_head =
            find_descendant(root, "architecture_head").expect("Should find architecture_head");

        // Collect only signals (not constants)
        let signals = collect_children(arch_head, "signal_declaration");
        assert_eq!(signals.len(), 2, "Should find 2 signal declarations");

        // Collect only constants
        let constants = collect_children(arch_head, "constant_declaration");
        assert_eq!(constants.len(), 1, "Should find 1 constant declaration");
    }

    #[test]
    fn test_collect_children_empty_parent() {
        let code = r#"
entity test is
    port ( );
end entity;
"#;
        let tree = parse_text(code);
        let root = tree.root_node();

        // Find the empty port_clause
        let port_clause = find_descendant(root, "port_clause").expect("Should find port_clause");

        // Should find no interface_declarations
        let declarations = collect_children(port_clause, "interface_declaration");
        assert!(
            declarations.is_empty(),
            "Empty port clause should have no declarations"
        );
    }
}
