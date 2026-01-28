//! Scope tree construction from Tree-sitter AST nodes.
//!
//! Contains functions to build scope trees for entities, architectures,
//! processes, and other VHDL constructs from parsed syntax trees.

use crate::{
    analysis::{
        DeclType, Declaration, PortDirection, ScopeKind, ScopeTree, TypeInfo, Usage, UsageContext,
        collect_identifiers_recursive,
    },
    utils::{
        ast::{collect_descendants, find_child, find_descendant, navigate_path},
        node_to_range,
    },
};
use std::collections::HashSet;
use tower_lsp::lsp_types::Range;
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
        if let Some(generic_clause) = navigate_path(ent_node, &["entity_head", "generic_clause"]) {
            tree.declarations
                .extend(extract_decl_from_generic_clause(generic_clause, text));
        }
        if let Some(port_clause) = navigate_path(ent_node, &["entity_head", "port_clause"]) {
            tree.declarations
                .extend(extract_decl_from_port_clause(port_clause, text));
        }
    }
    tree.rebuild_index();
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
            let names = extract_signal_names(interface_decl, text);
            let doc_comment = extract_doc_comment(interface_decl, text);
            let default_value = extract_default_value(interface_decl, text);
            for (name, range) in names {
                declarations.push(Declaration {
                    name,
                    decl_type: DeclType::Generic,
                    range: node_to_range(interface_decl),
                    selection_range: range,
                    type_info: extract_type_info(&interface_decl, text),
                    default_value: default_value.clone(),
                    doc_comment: doc_comment.clone(),
                });
            }
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
            let names = extract_signal_names(interface_decl, text);
            let doc_comment = extract_doc_comment(interface_decl, text);
            let default_value = extract_default_value(interface_decl, text);
            for (name, range) in names {
                declarations.push(Declaration {
                    name,
                    decl_type: DeclType::Port(direction),
                    range: node_to_range(interface_decl),
                    selection_range: range,
                    type_info: extract_type_info(&interface_decl, text),
                    default_value: default_value.clone(),
                    doc_comment: doc_comment.clone(),
                });
            }
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
    if let Some(arch_head) = find_child(arch_node, "architecture_head") {
        for signal_decl in collect_descendants(arch_head, "signal_declaration") {
            tree.declarations.extend(create_declarations_from_node(
                signal_decl,
                text,
                DeclType::Signal,
            ));
            collect_identifier_from_decl(&signal_decl, text, &mut tree.local_usage);
        }
        for const_decl in collect_descendants(arch_head, "constant_declaration") {
            tree.declarations.extend(create_declarations_from_node(
                const_decl,
                text,
                DeclType::Constant,
            ));
            collect_identifier_from_decl(&const_decl, text, &mut tree.local_usage);
        }
        for type_decl in collect_descendants(arch_head, "type_declaration") {
            collect_identifier_from_decl(&type_decl, text, &mut tree.local_usage);
        }
        for subtype_decl in collect_descendants(arch_head, "subtype_declaration") {
            collect_identifier_from_decl(&subtype_decl, text, &mut tree.local_usage);
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
    tree.rebuild_index();
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
    if let Some(proc_head) = find_child(process_node, "process_head") {
        for var_decl in collect_descendants(proc_head, "variable_declaration") {
            tree.declarations.extend(create_declarations_from_node(
                var_decl,
                text,
                DeclType::Variable,
            ));
            collect_identifier_from_decl(&var_decl, text, &mut tree.local_usage);
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
    tree.rebuild_index();
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
        if let Some(head_node) = find_child(body_node, "generate_head") {
            for signal_decl in collect_descendants(head_node, "signal_declaration") {
                tree.declarations.extend(create_declarations_from_node(
                    signal_decl,
                    text,
                    DeclType::Signal,
                ));
                collect_identifier_from_decl(&signal_decl, text, &mut tree.local_usage);
            }
            for const_decl in collect_descendants(head_node, "constant_declaration") {
                tree.declarations.extend(create_declarations_from_node(
                    const_decl,
                    text,
                    DeclType::Constant,
                ));
                collect_identifier_from_decl(&const_decl, text, &mut tree.local_usage);
            }
        }
        if let Some(block_node) = find_child(body_node, "generate_block") {
            for child in block_node.children(&mut block_node.walk()) {
                match child.kind() {
                    "process_statement" => {
                        tree.children.push(build_process_scope_tree(child, text))
                    }
                    "if_generate_statement" => tree
                        .children
                        .push(build_if_generate_scope_tree(child, text)),
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(child, text)),
                    "block_statement" => tree.children.push(build_block_scope_tree(child, text)),
                    _ => collect_identifiers_recursive(
                        child,
                        text,
                        UsageContext::Behavioral,
                        &mut tree.local_usage,
                    ),
                }
            }
        }
    }
    tree.rebuild_index();
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
        if let Some(head_node) = find_child(body_node, "generate_head") {
            for signal_decl in collect_descendants(head_node, "signal_declaration") {
                tree.declarations.extend(create_declarations_from_node(
                    signal_decl,
                    text,
                    DeclType::Signal,
                ));
                collect_identifier_from_decl(&signal_decl, text, &mut tree.local_usage);
            }
            for const_decl in collect_descendants(head_node, "constant_declaration") {
                tree.declarations.extend(create_declarations_from_node(
                    const_decl,
                    text,
                    DeclType::Constant,
                ));
                collect_identifier_from_decl(&const_decl, text, &mut tree.local_usage);
            }
        }
        if let Some(block_node) = find_child(body_node, "generate_block") {
            for child in block_node.children(&mut block_node.walk()) {
                match child.kind() {
                    "process_statement" => {
                        tree.children.push(build_process_scope_tree(child, text))
                    }
                    "if_generate_statement" => tree
                        .children
                        .push(build_if_generate_scope_tree(child, text)),
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(child, text)),
                    "block_statement" => tree.children.push(build_block_scope_tree(child, text)),
                    _ => collect_identifiers_recursive(
                        child,
                        text,
                        UsageContext::Behavioral,
                        &mut tree.local_usage,
                    ),
                }
            }
        }
    }
    tree.rebuild_index();
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
    if let Some(head_node) = find_child(block_node, "block_head") {
        for signal_decl in collect_descendants(head_node, "signal_declaration") {
            tree.declarations.extend(create_declarations_from_node(
                signal_decl,
                text,
                DeclType::Signal,
            ));
            collect_identifier_from_decl(&signal_decl, text, &mut tree.local_usage);
        }
        for const_decl in collect_descendants(head_node, "constant_declaration") {
            tree.declarations.extend(create_declarations_from_node(
                const_decl,
                text,
                DeclType::Constant,
            ));
            collect_identifier_from_decl(&const_decl, text, &mut tree.local_usage);
        }
    }

    // Process concurrent_block
    if let Some(concurrent_block) = find_child(block_node, "concurrent_block") {
        let mut inner_cursor = concurrent_block.walk();
        for inner_child in concurrent_block.children(&mut inner_cursor) {
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
    tree.rebuild_index();
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
///
/// # Returns
///
/// Vector of Declaration objects, one for each identifier in the declaration
fn extract_signal_names(signal_node: Node, text: &str) -> Vec<(String, Range)> {
    let mut signals: Vec<(String, Range)> = Vec::new();

    // Extract each identifier
    if let Some(identifier_list) = find_child(signal_node, "identifier_list") {
        for identifier in collect_descendants(identifier_list, "identifier") {
            let signal_name = &text[identifier.byte_range()];
            signals.push((signal_name.to_string(), node_to_range(identifier)));
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
///
/// # Returns
/// A vector of tuple of all identifiers with their corresponding range
fn collect_identifier_from_decl(node: &Node, text: &str, references: &mut HashSet<Usage>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier_list" => {}
            _ => collect_identifiers_recursive(child, text, UsageContext::TypeSpec, references),
        }
    }
}

/// Extract the type information from a generic or a port clause
///
/// Will extract every availble information on the signa/constant/port/generic type
///
/// Will start by looking for a simple_mode_indication before the subtype, will fallback
/// to a subtype directly if we cant find it.
///
/// # Arguments
/// `node` - The declaration node
/// `text` - Full source text
///
/// # Returns
/// A structure TypeInfo with as much information as what was extracted
fn extract_type_info(node: &Node, text: &str) -> TypeInfo {
    let mut type_info = TypeInfo::new();
    if let Some(indication_node) =
        navigate_path(*node, &["simple_mode_indication", "subtype_indication"])
            .or_else(|| find_child(*node, "subtype_indication"))
    {
        if let Some(name_node) = find_descendant(indication_node, "name") {
            if let Some(id) = find_child(name_node, "identifier")
                .or_else(|| find_child(name_node, "library_type"))
            {
                type_info.base_type = text[id.byte_range()].to_string();
            } else {
                type_info.base_type = text[name_node.byte_range()].to_string();
            }
            if let Some(paren_node) = find_descendant(name_node, "parenthesis_group") {
                type_info.constraints = Some(text[paren_node.byte_range()].to_string())
            }
        }
        if let Some(range_node) = find_descendant(indication_node, "range_constraints") {
            type_info.constraints = Some(text[range_node.byte_range()].to_string())
        }
    }
    // "initialiser" => {
    //     for init_node in inner.children(&mut inner.walk()) {
    //         if init_node.kind() == "conditional_expression" {
    //             let initial_value = &text[init_node.byte_range()];
    //             type_info.default_value = intial_valud.to_string();
    //         }
    //     }
    // }
    //_ => {}

    type_info
}

/// Extract the default value (or value for a constant) in a declaration
///
/// # Arguments
/// `decl_node`: Declaration node
/// `text`: Full source text
///
/// # Returns
/// Option containing the default value as a string if any
fn extract_default_value(decl_node: Node, text: &str) -> Option<String> {
    if let Some(initialiser) = find_descendant(decl_node, "initialiser") {
        // NOTE: For some reason, the RHS of the initializer is a conditional expression...
        // the AST of an initializer looks like this:
        // (initialiser
        //   (variable assignment)
        //   (conditional_expression
        //     (...))
        //  So we will get the conditional_expression text as the default value
        if let Some(default_value_node) = find_descendant(initialiser, "conditional_expression") {
            let default_value = text[default_value_node.byte_range()].to_string();
            return Some(default_value);
        }
    }
    None
}

/// Extract comments before a declaration. Will stop if it finds a non-comment node or an empty
/// line
///
/// # Arguments
/// `decl_node`: Declaration node
/// `text`: Full source text
///
/// # Returns
/// An option containing the doc comment if any
fn extract_doc_comment(decl_node: Node, text: &str) -> Option<String> {
    let decl_start_line = decl_node.start_position().row;
    let mut comments = Vec::new();
    let mut expected_line = decl_start_line - 1;

    if let Some(parent) = decl_node.parent() {
        let children: Vec<Node> = parent.children(&mut parent.walk()).collect();
        // Walk backward from the declaration node checking comments and line numbers
        if let Some(decl_idx) = children.iter().position(|n| *n == decl_node) {
            for i in (0..decl_idx).rev() {
                let child = children[i];
                if child.kind() == "line_comment" {
                    let comment_line = child.start_position().row;
                    if comment_line == expected_line {
                        if let Some(comment_content) = find_child(child, "comment_content") {
                            let comment_text =
                                text[comment_content.byte_range()].trim().to_string();
                            comments.insert(0, comment_text);
                        }
                        expected_line = child.start_position().row - 1;
                    } else {
                        // Hit an empty line
                        break;
                    }
                } else {
                    // Hit a non comment
                    break;
                }
            }
        }
    }

    if comments.is_empty() {
        None
    } else {
        Some(comments.join("\n"))
    }
}

/// Create a declaration from a specific node
///
/// # Arguments
/// `node`: Declaration node
/// `text`: Fill source text
/// `decl_type`: Type of the declartation
///
/// # Returns
/// Vector of declaration based on the node
fn create_declarations_from_node(node: Node, text: &str, decl_type: DeclType) -> Vec<Declaration> {
    let doc_comment = extract_doc_comment(node, text);
    let default_value = extract_default_value(node, text);
    extract_signal_names(node, text)
        .into_iter()
        .map(|(name, range)| Declaration {
            name,
            decl_type: decl_type.clone(),
            range: node_to_range(node),
            selection_range: range,
            type_info: extract_type_info(&node, text),
            default_value: default_value.clone(),
            doc_comment: doc_comment.clone(),
        })
        .collect()
}
