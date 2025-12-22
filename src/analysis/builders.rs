//! Scope tree construction from Tree-sitter AST nodes.
//!
//! Contains functions to build scope trees for entities, architectures,
//! processes, and other VHDL constructs from parsed syntax trees.

use crate::{
    analysis::{
        DeclType, Declaration, PortDirection, ScopeKind, ScopeTree, Usage, UsageContext,
        collect_identifiers_recursive,
    },
    backend::utils::node_to_range,
};
use std::collections::HashSet;
use tree_sitter::Node;

/// Builds a complete scope tree for an entity.
///
///
/// # Arguments
///
/// * `ent_node` - Tree-sitter node of type `entity_declaration`
/// * `text` - Full source text of the file
///
/// # Returns
///
/// Root node of the scope tree representing the entire entity declaration
pub fn build_entity_scope_tree(ent_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Entity, &ent_node);
    if let Some(name_node) = ent_node.child_by_field_name("entity") {
        tree.name = Some(text[name_node.byte_range()].to_string());
        for child in ent_node.children(&mut ent_node.walk()) {
            if child.kind() == "entity_head" {
                for inner in child.children(&mut child.walk()) {
                    match inner.kind() {
                        "generic_clause" => {
                            tree.declarations
                                .extend(extract_decl_from_generic_clause(inner, text));
                        }
                        "port_clause" => {
                            tree.declarations
                                .extend(extract_decl_from_port_clause(inner, text));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    tree
}

/// Extract generics from the generic clause
///
///
/// # Arguments
///
/// * `generic_clause` - The generic clause node
/// * `text` - Full source text of the file
///
/// # Returns
///
/// Vector of Declaration of all the generics
fn extract_decl_from_generic_clause(generic_clause: Node, text: &str) -> Vec<Declaration> {
    let mut declarations = Vec::new();
    if let Some(interface_list) = generic_clause
        .children(&mut generic_clause.walk())
        .find(|c| c.kind() == "interface_list")
    {
        // Clear and debuggable
        for interface_decl in interface_list.children(&mut interface_list.walk()) {
            if interface_decl.kind() != "interface_declaration" {
                continue;
            }
            declarations.extend(extract_signal_names(
                interface_decl,
                text,
                DeclType::Generic,
            ));
        }
    }
    declarations
}

/// Extract ports from the port clause
///
///
/// # Arguments
///
/// * `port_clause` - The port clause node
/// * `text` - Full source text of the file
///
/// # Returns
///
/// Vector of Declaration of all the ports
fn extract_decl_from_port_clause(port_clause: Node, text: &str) -> Vec<Declaration> {
    let mut declarations = Vec::new();
    if let Some(interface_list) = port_clause
        .children(&mut port_clause.walk())
        .find(|c| c.kind() == "interface_list")
    {
        for interface_decl in interface_list.children(&mut interface_list.walk()) {
            if interface_decl.kind() != "interface_declaration" {
                continue;
            }
            let direction = extract_direction_from_interface(interface_decl, text);
            declarations.extend(extract_signal_names(
                interface_decl,
                text,
                DeclType::Port(direction),
            ));
        }
    }

    declarations
}

/// Extract the direction from the port declaration
///
///
/// # Arguments
///
/// * `interface_clause` - The interface clause node
/// * `text` - Full source text of the file
///
/// # Returns
///
/// PortDirection (defaults to IN)
fn extract_direction_from_interface(interface_clause: Node, text: &str) -> PortDirection {
    for child in interface_clause.children(&mut interface_clause.walk()) {
        if child.kind() == "simple_mode_indication" {
            for inner in child.children(&mut child.walk()) {
                if inner.kind() == "mode" {
                    let mode_text = text[inner.byte_range()].to_lowercase();
                    return match mode_text.as_str() {
                        "in" => PortDirection::In,
                        "out" => PortDirection::Out,
                        "inout" => PortDirection::InOut,
                        "buffer" => PortDirection::Buffer,
                        "linkage" => PortDirection::Linkage,
                        _ => PortDirection::In,
                    };
                }
            }
        }
    }
    PortDirection::In
}

/// Builds a complete scope tree for an architecture.
///
/// Constructs the root scope node representing the architecture, then
/// recursively builds child scopes for all processes, generates, and blocks
/// found within.
///
/// # Arguments
///
/// * `arch_node` - Tree-sitter node of type `architecture_definition`
/// * `text` - Full source text of the file
///
/// # Returns
///
/// Root node of the scope tree representing the entire architecture
pub fn build_arch_scope_tree(arch_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Architecture, &arch_node);

    // TODO: extract that entity name associated with the architecuture
    if let Some(entity_name_node) = arch_node.child_by_field_name("entity") {
        let entity_name = text[entity_name_node.byte_range()].to_lowercase();
        tree.entity = Some(entity_name);
    }

    // Collect architecture-level declarations from architecture_head
    let mut cursor = arch_node.walk();
    for child in arch_node.children(&mut cursor) {
        if child.kind() == "architecture_head" {
            let mut head_cursor = child.walk();
            for decl_child in child.children(&mut head_cursor) {
                if decl_child.kind() == "signal_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Signal,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                } else if decl_child.kind() == "constant_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Constant,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                } else if decl_child.kind() == "type_declaration"
                    || decl_child.kind() == "subtype_declaration"
                {
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                }
            }
            break;
        }
    }

    // Process concurrent_block to find usage and child scopes
    let mut cursor = arch_node.walk();
    for child in arch_node.children(&mut cursor) {
        if child.kind() == "concurrent_block" {
            let mut inner_cursor = child.walk();
            for inner_child in child.children(&mut inner_cursor) {
                match inner_child.kind() {
                    "process_statement" => tree
                        .children
                        .push(build_process_scope_tree(inner_child, text)),
                    "if_generate_statement" => tree
                        .children
                        .push(build_if_generate_scope_tree(inner_child, text)),
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(inner_child, text)),
                    "block_statement" => tree
                        .children
                        .push(build_block_scope_tree(inner_child, text)),
                    _ => collect_identifiers_recursive(
                        inner_child,
                        text,
                        UsageContext::Behavioral,
                        &mut tree.local_usage,
                    ),
                }
            }
            break;
        }
    }
    tree
}

/// Builds a scope tree for a process.
///
/// Processes can declare variables (not signals) and have a sensitivity list
/// that marks signals as used.
///
/// # Arguments
///
/// * `process_node` - Tree-sitter node of type `process_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the process
pub fn build_process_scope_tree(process_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Process, &process_node);

    // Collect variable declarations from process_head
    let mut cursor = process_node.walk();
    for child in process_node.children(&mut cursor) {
        if child.kind() == "process_head" {
            let mut head_cursor = child.walk();
            for decl_child in child.children(&mut head_cursor) {
                if decl_child.kind() == "variable_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Variable,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                }
            }
            break;
        }
    }

    // Collect usage from sensitivity list and sequential block
    let mut cursor = process_node.walk();
    for child in process_node.children(&mut cursor) {
        if child.kind() == "sensitivity_specification" {
            // Signals in sensitivity list are considered used
            collect_identifiers_recursive(
                child,
                text,
                UsageContext::Behavioral,
                &mut tree.local_usage,
            );
        } else if child.kind() == "sequential_block" {
            collect_identifiers_recursive(
                child,
                text,
                UsageContext::Behavioral,
                &mut tree.local_usage,
            );
            break;
        }
    }
    tree
}

/// Builds a scope tree for an if-generate statement.
///
/// If-generates can declare signals and constants, and can contain
/// nested processes, generates, and blocks.
///
/// # Arguments
///
/// * `generate_node` - Tree-sitter node of type `if_generate_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the generate block
pub fn build_if_generate_scope_tree(generate_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Generate, &generate_node);

    // Find the generate_body nested inside if_generate
    let mut body_node: Option<Node> = None;
    let if_generate_node = generate_node
        .children(&mut generate_node.walk())
        .find(|c| c.kind() == "if_generate");
    if let Some(if_generate_node) = if_generate_node {
        // Find everything that was used in the if statement
        for inner in if_generate_node.children(&mut if_generate_node.walk()) {
            if inner.kind() == "generate_body" {
                body_node = Some(inner);
            } else {
                collect_identifiers_recursive(
                    inner,
                    text,
                    UsageContext::Behavioral,
                    &mut tree.local_usage,
                );
            }
        }
    }

    if let Some(body_node) = body_node {
        let mut cursor = body_node.walk();
        for child in body_node.children(&mut cursor) {
            if child.kind() == "generate_head" {
                // Collect signal/constant declarations
                let mut decl_cursor = child.walk();
                for decl_child in child.children(&mut decl_cursor) {
                    if decl_child.kind() == "signal_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Signal,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    } else if decl_child.kind() == "constant_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Constant,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    }
                }
            } else if child.kind() == "generate_block" {
                // Process generate body - may contain nested scopes
                let mut inner_cursor = child.walk();
                for inner_child in child.children(&mut inner_cursor) {
                    match inner_child.kind() {
                        "process_statement" => tree
                            .children
                            .push(build_process_scope_tree(inner_child, text)),
                        "if_generate_statement" => tree
                            .children
                            .push(build_if_generate_scope_tree(inner_child, text)),
                        "for_generate_statement" => tree
                            .children
                            .push(build_for_generate_scope_tree(inner_child, text)),
                        "block_statement" => tree
                            .children
                            .push(build_block_scope_tree(inner_child, text)),
                        _ => collect_identifiers_recursive(
                            inner_child,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        ),
                    }
                }
            }
        }
    }
    tree
}

/// Builds a scope tree for a for-generate statement.
///
/// For-generates have a simpler structure than if-generates (no nested
/// `if_generate` wrapper), but otherwise function identically.
///
/// # Arguments
///
/// * `generate_node` - Tree-sitter node of type `for_generate_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the generate block
pub fn build_for_generate_scope_tree(generate_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Generate, &generate_node);

    // Find generate_body directly (no if_generate wrapper)
    let mut body_node: Option<Node> = None;
    for child in generate_node.children(&mut generate_node.walk()) {
        if child.kind() == "generate_body" {
            body_node = Some(child);
        } else if child.kind() == "for_loop" {
            collect_identifiers_recursive(
                child,
                text,
                UsageContext::Behavioral,
                &mut tree.local_usage,
            );
        }
    }

    if let Some(body_node) = body_node {
        let mut cursor = body_node.walk();
        for child in body_node.children(&mut cursor) {
            if child.kind() == "generate_head" {
                let mut decl_cursor = child.walk();
                for decl_child in child.children(&mut decl_cursor) {
                    if decl_child.kind() == "signal_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Signal,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    } else if decl_child.kind() == "constant_declaration" {
                        tree.declarations.extend(extract_signal_names(
                            decl_child,
                            text,
                            DeclType::Constant,
                        ));
                        collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                    }
                }
            } else if child.kind() == "generate_block" {
                let mut inner_cursor = child.walk();
                for inner_child in child.children(&mut inner_cursor) {
                    match inner_child.kind() {
                        "process_statement" => tree
                            .children
                            .push(build_process_scope_tree(inner_child, text)),
                        "if_generate_statement" => tree
                            .children
                            .push(build_if_generate_scope_tree(inner_child, text)),
                        "for_generate_statement" => tree
                            .children
                            .push(build_for_generate_scope_tree(inner_child, text)),
                        "block_statement" => tree
                            .children
                            .push(build_block_scope_tree(inner_child, text)),
                        _ => collect_identifiers_recursive(
                            inner_child,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        ),
                    }
                }
            }
        }
    }
    tree
}

/// Builds a scope tree for a block statement.
///
/// Blocks have a structure similar to architectures with a declarative
/// region (block_head) and a concurrent region (concurrent_block).
///
/// # Arguments
///
/// * `block_node` - Tree-sitter node of type `block_statement`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the block
pub fn build_block_scope_tree(block_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Block, &block_node);

    // Collect declarations from block_head
    let mut cursor = block_node.walk();
    for child in block_node.children(&mut cursor) {
        if child.kind() == "block_head" {
            let mut head_cursor = child.walk();
            for decl_child in child.children(&mut head_cursor) {
                if decl_child.kind() == "signal_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Signal,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                } else if decl_child.kind() == "constant_declaration" {
                    tree.declarations.extend(extract_signal_names(
                        decl_child,
                        text,
                        DeclType::Constant,
                    ));
                    collect_identifier_from_decl(decl_child, text, &mut tree.local_usage);
                }
            }
            break;
        }
    }

    // Process concurrent_block
    let mut cursor = block_node.walk();
    for child in block_node.children(&mut cursor) {
        if child.kind() == "concurrent_block" {
            let mut inner_cursor = child.walk();
            for inner_child in child.children(&mut inner_cursor) {
                match inner_child.kind() {
                    "process_statement" => tree
                        .children
                        .push(build_process_scope_tree(inner_child, text)),
                    "if_generate_statement" => tree
                        .children
                        .push(build_if_generate_scope_tree(inner_child, text)),
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(inner_child, text)),
                    "block_statement" => tree
                        .children
                        .push(build_block_scope_tree(inner_child, text)),
                    _ => collect_identifiers_recursive(
                        inner_child,
                        text,
                        UsageContext::Behavioral,
                        &mut tree.local_usage,
                    ),
                }
            }
            break;
        }
    }

    tree
}

/// Extracts all declared names from a signal/variable/constant declaration.
///
/// Handles declarations with multiple names (e.g., `signal a, b, c : std_logic`).
///
/// # Arguments
///
/// * `signal_node` - Declaration node (signal_declaration, variable_declaration, etc.)
/// * `text` - Full source text
/// * `decl_type` - Type of declaration being processed
///
/// # Returns
///
/// Vector of Declaration objects, one for each identifier in the declaration
fn extract_signal_names(signal_node: Node, text: &str, decl_type: DeclType) -> Vec<Declaration> {
    let mut signals: Vec<Declaration> = Vec::new();

    // Find identifier_list child
    let mut cursor = signal_node.walk();
    let mut identifier_list: Option<Node> = None;
    for child in signal_node.children(&mut cursor) {
        if child.kind() == "identifier_list" {
            identifier_list = Some(child);
            break;
        }
    }

    // Extract each identifier
    if let Some(identifier_list) = identifier_list {
        let mut cursor = identifier_list.walk();
        for child in identifier_list.children(&mut cursor) {
            if child.kind() == "identifier" {
                let signal_name = &text[child.byte_range()];
                signals.push(Declaration {
                    name: signal_name.to_string(),
                    decl_type: decl_type.clone(),
                    range: node_to_range(signal_node),
                    selection_range: node_to_range(child),
                });
            }
        }
    }
    signals
}

/// Extract identifiers from declartion
///
/// Will extract every identifier on the right side of a declaration
///
/// # Arguments
///
/// `node` - Root node to search from
/// `text` - Full source text
/// `references` - Mutable set to collect identifer names into
fn collect_identifier_from_decl(node: Node, text: &str, references: &mut HashSet<Usage>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier_list" => {}
            _ => collect_identifiers_recursive(child, text, UsageContext::TypeSpec, references),
        }
    }
}
