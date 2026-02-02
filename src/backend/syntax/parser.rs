// src/backend/syntax/parser.rs

use crate::analysis::{
    Analysis, ParseLevel, build_arch_scope_tree, build_entity_scope_tree,
    build_package_body_scope_tree, build_package_scope_tree,
};
use tree_sitter::Node;

/// Recursively extracts all VHDL symbols from a parsed syntax tree.
///
/// This is the main entry point for the "Deep Parse" phase. It walks the entire
/// AST, identifying symbols (Entities, Architectures, Signals, etc.) and building
/// a hierarchical `Analysis` struct.
///
/// # Logic
/// 1. Initializes a `TreeCursor` to walk the tree efficiently.
/// 2. Calls the recursive `visit_node` function to traverse the AST.
/// 3. Flattens the top-level symbols into a HashMap for O(1) lookup.
///
/// # Arguments
/// * `text` - The full source code of the document (used to extract names/types).
/// * `root_node` - The root node of the Tree-sitter tree.
///
/// # Returns
/// An `Analysis` struct containing a map of all top-level symbols found.
pub fn extract_document_symbols(text: &str, root_node: Node) -> Analysis {
    let mut analysis = Analysis::new();
    analysis.parse_level = ParseLevel::Deep;

    let mut cursor = root_node.walk();
    for node in root_node.children(&mut cursor) {
        if node.kind() == "design_unit" {
            for child in node.children(&mut node.walk()) {
                if child.kind() == "entity_declaration" {
                    let scope_tree = build_entity_scope_tree(child, text);
                    if let Some(ref name) = scope_tree.name {
                        analysis.entity_scope_trees.insert(name.clone(), scope_tree);
                    }
                }
                if child.kind() == "architecture_definition" {
                    let scope_tree = build_arch_scope_tree(child, text);
                    analysis.scope_trees.push(scope_tree);
                }
                if child.kind() == "package_declaration" {
                    let scope_tree = build_package_scope_tree(child, text);
                    if let Some(ref name) = scope_tree.name {
                        analysis
                            .package_scope_trees
                            .insert(name.clone(), scope_tree);
                    }
                }
                if child.kind() == "package_definition" {
                    let scope_tree = build_package_body_scope_tree(child, text);
                    analysis.scope_trees.push(scope_tree)
                }
            }
        }
    }

    analysis
}
