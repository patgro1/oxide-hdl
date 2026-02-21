// src/backend/syntax/parser.rs

use crate::{
    analysis::{
        Analysis, ParseLevel, UseClause, build_arch_scope_tree, build_entity_scope_tree,
        build_package_body_scope_tree, build_package_scope_tree,
    },
    utils::{
        ast::{collect_children, find_child},
        node_to_range,
    },
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
                if child.kind() == "use_clause" {
                    analysis
                        .use_clauses
                        .extend(extract_imported_packages_from_node(child, text));
                }
                if child.kind() == "entity_declaration" {
                    let scope_tree = build_entity_scope_tree(child, text);
                    if let Some(ref name) = scope_tree.name {
                        analysis
                            .entity_scope_trees
                            .insert(name.to_lowercase(), scope_tree);
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
                            .package_declaration_scope_trees
                            .insert(name.to_lowercase(), scope_tree);
                    }
                }
                if child.kind() == "package_definition" {
                    let scope_tree = build_package_body_scope_tree(child, text);
                    if let Some(ref name) = scope_tree.name {
                        analysis
                            .package_body_scope_trees
                            .insert(name.to_lowercase(), scope_tree);
                    }
                }
            }
        }
    }
    analysis
}

/// This function is used to extract all imported packages from a single use clause
///
/// # Arguments
/// * `node` - The use clause node
/// * `text` - Full source text
///
/// # Returns
/// A vector of UseClause containing all the imports
fn extract_imported_packages_from_node(node: Node, text: &str) -> Vec<UseClause> {
    let mut packages = Vec::new();
    if let Some(use_list) = find_child(node, "selected_name_list") {
        for import in collect_children(use_list, "selected_name") {
            if let Some(library_node) = import.child_by_field_name("library")
                && let Some(package_node) = import.child_by_field_name("package")
            {
                if find_child(import, "ALL").is_some() {
                    packages.push(UseClause {
                        range: node_to_range(import),
                        library: text[library_node.byte_range()].to_string(),
                        name: text[package_node.byte_range()].to_string(),
                        all_import: true,
                        imported_symbol: None,
                    });
                } else if let Some(name_node) = import.named_child(import.named_child_count() - 1) {
                    packages.push(UseClause {
                        range: node_to_range(import),
                        library: text[library_node.byte_range()].to_string(),
                        name: text[package_node.byte_range()].to_string(),
                        all_import: false,
                        imported_symbol: Some(text[name_node.byte_range()].to_string()),
                    })
                }
            }
        }
    }

    packages
}
