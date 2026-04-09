//! Scope tree construction from Tree-sitter AST nodes.
//!
//! Contains functions to build scope trees for entities, architectures,
//! processes, and other VHDL constructs from parsed syntax trees.

use crate::{
    analysis::{
        DeclType, Declaration, Instance, ParameterClass, PortDirection, RegionType, ScopeKind,
        ScopeTree, TypeInfo, Usage, UsageContext, UseClause, collect_identifiers_recursive,
    },
    utils::{
        ast::{collect_children, collect_descendants, find_child, find_descendant, navigate_path},
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
            // Collect identifiers only from type constraints and default values, not from declared names
            collect_identifier_from_interface_clause(&generic_clause, text, &mut tree.local_usage);
        }
        if let Some(port_clause) = navigate_path(ent_node, &["entity_head", "port_clause"]) {
            tree.declarations
                .extend(extract_decl_from_port_clause(port_clause, text));
            // Collect identifiers only from type constraints and default values, not from declared names
            collect_identifier_from_interface_clause(&port_clause, text, &mut tree.local_usage);
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
                    parameters: None,
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
                    parameters: None,
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

    if let Some(arch_name) = arch_node.child_by_field_name("architecture") {
        let name = text[arch_name.byte_range()].to_string();
        tree.name = Some(name);
    }
    if let Some(entity_name_node) = arch_node.child_by_field_name("entity") {
        let entity_name = text[entity_name_node.byte_range()].to_string();
        tree.entity = Some(entity_name);
    }

    // Collect architecture-level declarations from architecture_head
    if let Some(architecture_head) = find_child(arch_node, "architecture_head") {
        tree.declarations = extract_declaration_from_node(
            architecture_head,
            text,
            RegionType::Concurrent,
            &mut tree.local_usage,
        );
        for (attr, names) in extract_attr_specs_from_node(architecture_head, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
        }
        // Special case here for subprogram_declaration (implementation)
        for subprogram_node in collect_children(architecture_head, "subprogram_definition") {
            tree.children
                .push(build_subprogram_scope_tree(subprogram_node, text));
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
                    "if_generate_statement" => {
                        build_if_generate_scope_tree(inner_child, text, &mut tree)
                    }
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(inner_child, text)),
                    "block_statement" => tree
                        .children
                        .push(build_block_scope_tree(inner_child, text)),
                    "component_instantiation_statement" => {
                        tree.instantiations
                            .push(create_instance_from_node(inner_child, text));
                        if let Some(generic_aspect) = find_child(inner_child, "generic_map_aspect")
                        {
                            crate::analysis::collect_identifiers_from_map_aspect(
                                generic_aspect,
                                text,
                                UsageContext::Behavioral,
                                &mut tree.local_usage,
                            );
                        }
                        if let Some(port_aspect) = find_child(inner_child, "port_map_aspect") {
                            crate::analysis::collect_identifiers_from_map_aspect(
                                port_aspect,
                                text,
                                UsageContext::Behavioral,
                                &mut tree.local_usage,
                            );
                        }
                    }
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
/// Processes sequential statements within a process, creating child scopes for for-loops.
///
/// Sequential for-loops introduce their own scope with the loop variable visible only
/// within the loop body. This function recursively walks statements and creates child
/// scope trees for loops.
///
/// # Arguments
///
/// * `sequential_node` - Tree-sitter node of type `sequential_block` or statement
/// * `text` - Full source text
/// * `parent_tree` - Parent scope tree to add children and usages to
fn process_sequential_statements(sequential_node: Node, text: &str, parent_tree: &mut ScopeTree) {
    let mut cursor = sequential_node.walk();
    for child in sequential_node.children(&mut cursor) {
        match child.kind() {
            "loop_statement" => {
                // Create a child scope for the loop
                let mut loop_tree = ScopeTree::new(ScopeKind::Process, &child);

                // Extract loop variable from for_loop if present.
                // _iteration_scheme is a hidden rule; the actual child is for_loop or while_loop.
                if let Some(for_loop) = find_child(child, "for_loop")
                    && let Some(parameter_spec) = find_child(for_loop, "parameter_specification")
                    && let Some(identifier) = find_child(parameter_spec, "identifier")
                {
                    loop_tree.declarations.push(Declaration {
                        name: text[identifier.byte_range()].to_string(),
                        decl_type: DeclType::Constant,
                        range: node_to_range(identifier),
                        selection_range: node_to_range(identifier),
                        type_info: TypeInfo::new(),
                        default_value: None,
                        doc_comment: None,
                        parameters: None,
                    });

                    // Collect identifiers from the range expression (e.g. constants used in bounds)
                    collect_identifiers_recursive(
                        for_loop,
                        text,
                        UsageContext::Behavioral,
                        &mut loop_tree.local_usage,
                    );
                }

                // Collect identifiers from loop body as usages.
                // The loop body node is loop_body, not sequential_block.
                if let Some(loop_body) = find_child(child, "loop_body") {
                    process_sequential_statements(loop_body, text, &mut loop_tree);
                }

                loop_tree.rebuild_index();
                parent_tree.children.push(loop_tree);
            }
            "if_statement_block"
            | "if_statement"
            | "if_statement_body"
            | "elsif_statement"
            | "else_statement"
            | "case_statement"
            | "case_body"
            | "case_statement_alternative"
            | "sequential_block_statement" => {
                // For if/case and their body/branch nodes, recursively process their bodies
                process_sequential_statements(child, text, parent_tree);
            }
            _ => {
                // For all other statements, collect identifiers as usages
                if !child.kind().starts_with("ERROR") && !child.is_extra() {
                    collect_identifiers_recursive(
                        child,
                        text,
                        UsageContext::Behavioral,
                        &mut parent_tree.local_usage,
                    );
                }
            }
        }
    }
}

pub fn build_process_scope_tree(process_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Process, &process_node);

    if let Some(label_decl) = find_child(process_node, "label_declaration")
        && let Some(label) = find_child(label_decl, "label")
    {
        let name = text[label.byte_range()].to_string();
        tree.name = Some(name);
    }

    // Collect variable declarations from process_head
    if let Some(proc_head) = find_child(process_node, "process_head") {
        tree.declarations = extract_declaration_from_node(
            proc_head,
            text,
            RegionType::Sequential,
            &mut tree.local_usage,
        );
        for (attr, names) in extract_attr_specs_from_node(proc_head, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
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
            // Process sequential statements, creating child scopes for for-loops
            process_sequential_statements(child, text, &mut tree);
            break;
        }
    }
    tree.rebuild_index();
    tree
}

/// Populates a `Generate` scope tree from a `generate_body` node.
///
/// Handles both grammar forms:
/// - `generate [generate_block]`                       (no declarative region)
/// - `generate [generate_head] begin [generate_block]` (with declarative region)
///
/// Declarations, use_clauses, instantiations, child scopes, and identifier
/// usages are all added directly to `tree`. Since each branch scope is freshly
/// created before this is called, assignment (`=`) is used for declarations.
fn process_generate_body(body_node: Node, text: &str, tree: &mut ScopeTree) {
    if let Some(head_node) = find_child(body_node, "generate_head") {
        tree.declarations = extract_declaration_from_node(
            head_node,
            text,
            RegionType::Concurrent,
            &mut tree.local_usage,
        );
        for (attr, names) in extract_attr_specs_from_node(head_node, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
        }
        for child in head_node.children(&mut head_node.walk()) {
            if child.kind() == "use_clause" {
                tree.use_clauses
                    .extend(extract_use_clauses_from_node(child, text));
            }
        }
        for subprogram_node in collect_children(head_node, "subprogram_definition") {
            tree.children
                .push(build_subprogram_scope_tree(subprogram_node, text));
        }
    }
    if let Some(block_node) = find_child(body_node, "generate_block") {
        for child in block_node.children(&mut block_node.walk()) {
            match child.kind() {
                "process_statement" => {
                    tree.children.push(build_process_scope_tree(child, text))
                }
                "if_generate_statement" => {
                    build_if_generate_scope_tree(child, text, tree)
                }
                "for_generate_statement" => tree
                    .children
                    .push(build_for_generate_scope_tree(child, text)),
                "block_statement" => tree.children.push(build_block_scope_tree(child, text)),
                "component_instantiation_statement" => {
                    tree.instantiations
                        .push(create_instance_from_node(child, text));
                    if let Some(generic_aspect) = find_child(child, "generic_map_aspect") {
                        crate::analysis::collect_identifiers_from_map_aspect(
                            generic_aspect,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        );
                    }
                    if let Some(port_aspect) = find_child(child, "port_map_aspect") {
                        crate::analysis::collect_identifiers_from_map_aspect(
                            port_aspect,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        );
                    }
                }
                "subprogram_definition" => {
                    tree.children.push(build_subprogram_scope_tree(child, text))
                }
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

/// Recursively walks an `if_generate`, `elsif_generate`, or `else_generate`
/// node, creating one `Generate` scope per `generate_body` and pushing each
/// as a sibling child of `parent`.
///
/// Called by [`build_if_generate_scope_tree`]. See that function's doc for the
/// full explanation of why this is recursive (grammar "Russian doll" nesting)
/// and what to change if the grammar is ever fixed to use flat siblings.
///
/// Condition expressions (the `cond1` in `if cond1 generate`, etc.) are not
/// inside any `generate_body` — they fall through to `collect_identifiers_recursive`
/// and are recorded as usages in `parent` (the enclosing architecture or generate
/// scope), which is correct because they reference generics/constants declared there.
///
/// # Arguments
/// * `node` — an `if_generate`, `elsif_generate`, or `else_generate` tree-sitter node
/// * `parent` — the enclosing scope; branch scopes are pushed as its children
/// * `label` — the `if_generate_statement` label; applied to the first branch only
/// * `is_first` — mutable flag tracking whether the label has been assigned yet;
///   set to `false` after the first `generate_body` is processed
fn build_if_generate_branches<'a>(
    node: Node<'a>,
    text: &str,
    parent: &mut ScopeTree,
    label: Option<&str>,
    is_first: &mut bool,
) {
    for child in node.children(&mut node.walk()) {
        match child.kind() {
            "generate_body" => {
                let mut scope = ScopeTree::new(ScopeKind::Generate, &child);
                if *is_first {
                    scope.name = label.map(|s| s.to_string());
                    *is_first = false;
                }
                process_generate_body(child, text, &mut scope);
                scope.rebuild_index();
                parent.children.push(scope);
            }
            "elsif_generate" | "else_generate" => {
                build_if_generate_branches(child, text, parent, label, is_first);
            }
            _ => {
                // Condition expressions and keywords — usages belong to the enclosing scope
                collect_identifiers_recursive(
                    child,
                    text,
                    UsageContext::Behavioral,
                    &mut parent.local_usage,
                );
            }
        }
    }
}

/// Builds `Generate` scope trees for an if-generate statement, pushing one
/// sibling scope per branch directly into `parent`.
///
/// # Why `parent: &mut ScopeTree` instead of returning a single `ScopeTree`
///
/// A VHDL if-generate statement can have multiple branches (`if`, `elsif`,
/// `else`), each with its own independent declarative region. A signal declared
/// in the if-branch is **not** visible in the else-branch and vice-versa.
/// Merging all branches into a single scope would make every signal visible
/// everywhere inside the generate statement, producing false positives in
/// undeclared-identifier diagnostics and incorrect completions.
///
/// Each branch therefore gets its own `Generate` scope, scoped precisely to
/// its `generate_body` byte range. Because a single `if_generate_statement`
/// maps to N scopes (one per branch), returning a single `ScopeTree` is not
/// possible — instead we push directly into `parent`.
///
/// # Grammar note — "Russian doll" nesting (tree-sitter-vhdl)
///
/// As of the current tree-sitter-vhdl grammar, alternate branches are **not**
/// flat siblings of the if-branch inside `if_generate_statement`. They are
/// nested recursively:
///
/// ```text
/// if_generate_statement
///   if_generate
///     generate_body          ← if-branch
///     elsif_generate         ← child of if_generate
///       generate_body        ← elsif-branch
///       else_generate        ← child of elsif_generate (the inner doll)
///         generate_body      ← else-branch
/// ```
///
/// `_elsif_else_generate` (the hidden rule that produces `elsif_generate` or
/// `else_generate`) is an optional tail of both `if_generate` and
/// `elsif_generate`, so a plain `if/else` (no elsif) looks like:
///
/// ```text
/// if_generate
///   generate_body
///   else_generate            ← direct child, no elsif wrapper
///     generate_body
/// ```
///
/// This nesting is arguably a grammar bug — conceptually all branches are
/// siblings. If it is ever fixed and the grammar produces flat siblings instead,
/// [`build_if_generate_branches`] is the only place that needs to change: the
/// `"elsif_generate" | "else_generate"` recurse arm would become unnecessary
/// and the `"generate_body"` arm alone would suffice.
///
/// # Label assignment
///
/// The `if_generate_statement` label (e.g. `gen_cond :`) belongs conceptually
/// to the whole statement, but is attached only to the **first** (if) branch
/// scope so that symbol-table lookups return exactly one result.
pub fn build_if_generate_scope_tree(generate_node: Node, text: &str, parent: &mut ScopeTree) {
    let label = find_child(generate_node, "label_declaration")
        .and_then(|ld| find_child(ld, "label"))
        .map(|l| text[l.byte_range()].to_string());

    let if_generate_node = generate_node
        .children(&mut generate_node.walk())
        .find(|c| c.kind() == "if_generate");

    if let Some(if_generate_node) = if_generate_node {
        let mut is_first = true;
        build_if_generate_branches(
            if_generate_node,
            text,
            parent,
            label.as_deref(),
            &mut is_first,
        );
    }
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

    if let Some(label_decl) = find_child(generate_node, "label_declaration")
        && let Some(label) = find_child(label_decl, "label")
    {
        let name = text[label.byte_range()].to_string();
        tree.name = Some(name);
    }

    // Find generate_body directly (no if_generate wrapper)
    let mut body_node: Option<Node> = None;
    for child in generate_node.children(&mut generate_node.walk()) {
        if child.kind() == "generate_body" {
            body_node = Some(child);
        } else if child.kind() == "for_loop" {
            if let Some(parameter_spec) = find_child(child, "parameter_specification")
                && let Some(identifier) = find_child(parameter_spec, "identifier")
            {
                tree.declarations.push(Declaration {
                    name: text[identifier.byte_range()].to_string(),
                    decl_type: DeclType::Constant,
                    range: node_to_range(identifier),
                    selection_range: node_to_range(identifier),
                    type_info: TypeInfo::new(),
                    default_value: None,
                    doc_comment: None,
                    parameters: None,
                });
            }
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
            tree.declarations.extend(extract_declaration_from_node(
                head_node,
                text,
                RegionType::Concurrent,
                &mut tree.local_usage,
            ));
            for (attr, names) in extract_attr_specs_from_node(head_node, text) {
                tree.attr_specs.entry(attr).or_default().extend(names);
            }
            // Collect use_clauses declared in the generate declarative region
            for child in head_node.children(&mut head_node.walk()) {
                if child.kind() == "use_clause" {
                    tree.use_clauses
                        .extend(extract_use_clauses_from_node(child, text));
                }
            }
            // Special case here for subprogram_declaration (implementation)
            for subprogram_node in collect_children(head_node, "subprogram_definition") {
                tree.children
                    .push(build_subprogram_scope_tree(subprogram_node, text));
            }
        }
        if let Some(block_node) = find_child(body_node, "generate_block") {
            for child in block_node.children(&mut block_node.walk()) {
                match child.kind() {
                    "process_statement" => {
                        tree.children.push(build_process_scope_tree(child, text))
                    }
                    "if_generate_statement" => {
                        build_if_generate_scope_tree(child, text, &mut tree)
                    }
                    "for_generate_statement" => tree
                        .children
                        .push(build_for_generate_scope_tree(child, text)),
                    "block_statement" => tree.children.push(build_block_scope_tree(child, text)),
                    "component_instantiation_statement" => {
                        tree.instantiations
                            .push(create_instance_from_node(child, text));
                        if let Some(generic_aspect) = find_child(child, "generic_map_aspect") {
                            crate::analysis::collect_identifiers_from_map_aspect(
                                generic_aspect,
                                text,
                                UsageContext::Behavioral,
                                &mut tree.local_usage,
                            );
                        }
                        if let Some(port_aspect) = find_child(child, "port_map_aspect") {
                            crate::analysis::collect_identifiers_from_map_aspect(
                                port_aspect,
                                text,
                                UsageContext::Behavioral,
                                &mut tree.local_usage,
                            );
                        }
                    }
                    "subprogram_definition" => {
                        tree.children.push(build_subprogram_scope_tree(child, text))
                    }
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

    if let Some(label_decl) = find_child(block_node, "label_declaration")
        && let Some(label) = find_child(label_decl, "label")
    {
        let name = text[label.byte_range()].to_string();
        tree.name = Some(name);
    }
    // Collect declarations from block_head
    if let Some(head_node) = find_child(block_node, "block_head") {
        tree.declarations = extract_declaration_from_node(
            head_node,
            text,
            RegionType::Concurrent,
            &mut tree.local_usage,
        );
        for (attr, names) in extract_attr_specs_from_node(head_node, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
        }
        // Collect use_clauses declared in the block declarative region
        for child in head_node.children(&mut head_node.walk()) {
            if child.kind() == "use_clause" {
                tree.use_clauses
                    .extend(extract_use_clauses_from_node(child, text));
            }
        }
        // Special case here for subprogram_declaration (implementation)
        for subprogram_node in collect_children(head_node, "subprogram_definition") {
            tree.children
                .push(build_subprogram_scope_tree(subprogram_node, text));
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
                "if_generate_statement" => {
                    build_if_generate_scope_tree(inner_child, text, &mut tree)
                }
                "for_generate_statement" => tree
                    .children
                    .push(build_for_generate_scope_tree(inner_child, text)),
                "block_statement" => tree
                    .children
                    .push(build_block_scope_tree(inner_child, text)),
                "component_instantiation_statement" => {
                    tree.instantiations
                        .push(create_instance_from_node(inner_child, text));
                    if let Some(generic_aspect) = find_child(inner_child, "generic_map_aspect") {
                        crate::analysis::collect_identifiers_from_map_aspect(
                            generic_aspect,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        );
                    }
                    if let Some(port_aspect) = find_child(inner_child, "port_map_aspect") {
                        crate::analysis::collect_identifiers_from_map_aspect(
                            port_aspect,
                            text,
                            UsageContext::Behavioral,
                            &mut tree.local_usage,
                        );
                    }
                }
                "subprogram_definition" => tree
                    .children
                    .push(build_subprogram_scope_tree(inner_child, text)),
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

/// Extracts UseClause items from a single `use_clause` AST node.
///
/// Mirrors the logic in `parser.rs::extract_imported_packages_from_node` but
/// is exposed as a public function so generate/block builders can call it when
/// populating `ScopeTree::use_clauses`.
///
/// # Arguments
/// * `use_clause_node` - Tree-sitter node of kind `use_clause`
/// * `text` - Full source text
///
/// # Returns
/// Vector of `UseClause` items parsed from this node
pub fn extract_use_clauses_from_node(use_clause_node: Node, text: &str) -> Vec<UseClause> {
    let mut packages = Vec::new();
    if let Some(use_list) = find_child(use_clause_node, "selected_name_list") {
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
                } else if let Some(name_node) =
                    import.named_child(import.named_child_count() - 1)
                {
                    packages.push(UseClause {
                        range: node_to_range(import),
                        library: text[library_node.byte_range()].to_string(),
                        name: text[package_node.byte_range()].to_string(),
                        all_import: false,
                        imported_symbol: Some(text[name_node.byte_range()].to_string()),
                    });
                }
            }
        }
    }
    packages
}

/// Build scope tree for a package definition
///
/// # Arguments
///
/// * `package_node` - Tree-sitter node of type `package_declaration`
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree node representing the package
pub fn build_package_scope_tree(package_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::Package, &package_node);
    if let Some(name_node) = package_node.child_by_field_name("package") {
        tree.name = Some(text[name_node.byte_range()].to_string());
    }
    if let Some(decl_body) = find_child(package_node, "package_declaration_body") {
        tree.declarations = extract_declaration_from_node(
            decl_body,
            text,
            RegionType::Concurrent,
            &mut tree.local_usage,
        );
        for (attr, names) in extract_attr_specs_from_node(decl_body, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
        }
    }
    tree
}

/// Build scope tree for package implementation
/// # Arguments
///
/// * `package_node` - Tree-sitter node of the package_defintion
/// * `text` - Full source text
///
/// # Returns
/// Scope tree for the package implementation
pub fn build_package_body_scope_tree(package_node: Node, text: &str) -> ScopeTree {
    let mut tree = ScopeTree::new(ScopeKind::PackageBody, &package_node);
    if let Some(name) = package_node.child_by_field_name("package") {
        tree.name = Some(text[name.byte_range()].to_string());
        tree.package = tree.name.clone();
    }
    if let Some(decl_body) = find_child(package_node, "package_definition_body") {
        tree.declarations = extract_declaration_from_node(
            decl_body,
            text,
            RegionType::Implementation,
            &mut tree.local_usage,
        );
        for (attr, names) in extract_attr_specs_from_node(decl_body, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
        }

        for child in decl_body.children(&mut decl_body.walk()) {
            match child.kind() {
                "subprogram_definition" => {
                    tree.children.push(build_subprogram_scope_tree(child, text))
                }
                _ => continue,
            }
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
///
/// # Returns
///
/// Vector of tuples containing (name, range) for each identifier in the declaration
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

/// Collect identifier usages from the right side of a declaration.
///
/// Extracts every identifier on the right side of a declaration (type references,
/// default values, etc.) and adds them to the provided references set.
///
/// # Arguments
///
/// * `node` - Root node to search from
/// * `text` - Full source text
/// * `references` - Mutable set to collect identifier usages into
///
/// Collects identifiers from interface clauses (generic/port clauses).
/// Only collects from type specifications and default values, NOT from the
/// declared identifier names themselves.
///
/// # Arguments
///
/// * `clause_node` - The generic_clause or port_clause node
/// * `text` - Full source text
/// * `references` - Mutable set to collect identifier usages into
fn collect_identifier_from_interface_clause(
    clause_node: &Node,
    text: &str,
    references: &mut HashSet<Usage>,
) {
    // Find interface_list containing the interface_declarations
    if let Some(interface_list) = clause_node
        .children(&mut clause_node.walk())
        .find(|c| c.kind() == "interface_list")
    {
        for interface_decl in interface_list.children(&mut interface_list.walk()) {
            if interface_decl.kind() != "interface_declaration" {
                continue;
            }
            // For each interface_declaration, collect from type and default value, but NOT from identifier_list
            for part in interface_decl.children(&mut interface_decl.walk()) {
                match part.kind() {
                    "identifier_list" => {} // Skip - these are the names being declared
                    _ => collect_identifiers_recursive(
                        part,
                        text,
                        UsageContext::TypeSpec,
                        references,
                    ),
                }
            }
        }
    }
}

fn collect_identifier_from_decl(node: &Node, text: &str, references: &mut HashSet<Usage>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            // Skip identifier_list - these are the names being declared, not usages
            "identifier_list" => {}
            _ => collect_identifiers_recursive(child, text, UsageContext::TypeSpec, references),
        }
    }
}

/// Extract the type information from a generic or a port clause
///
/// Will extract every available information on the signal/constant/port/generic type
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

    // 1. Attempt to find subtype_indication (used by Signals, Ports, Constants, and Subtypes)
    let indication_node = navigate_path(*node, &["simple_mode_indication", "subtype_indication"])
        .or_else(|| find_child(*node, "subtype_indication"));

    if let Some(ind_node) = indication_node {
        if let Some(name_node) = find_descendant(ind_node, "name") {
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

        // Handle explicit constraints on the subtype indication (e.g., range constraints)
        if let Some(range_node) = find_child(ind_node, "range_constraint") {
            type_info.constraints = Some(text[range_node.byte_range()].to_string())
        }
    } else {
        // 2. Fallback for Type Declarations where the definition is a direct child
        // This handles: type T is range ...; type E is (...); type R is record ...;
        if let Some(range_node) = find_child(*node, "range_constraint") {
            type_info.constraints = Some(text[range_node.byte_range()].to_string());
        } else if let Some(enum_node) = find_child(*node, "enumeration_type_definition") {
            type_info.constraints = Some(text[enum_node.byte_range()].to_string());
        } else if let Some(record_node) = find_child(*node, "record_type_definition") {
            type_info.constraints = Some(text[record_node.byte_range()].to_string());
        } else if let Some(phys_node) = find_child(*node, "physical_type_definition") {
            type_info.constraints = Some(text[phys_node.byte_range()].to_string());
        } else if let Some(phys_node) = find_child(*node, "array_type_definition") {
            type_info.constraints = Some(text[phys_node.byte_range()].to_string());
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
                    // Making sure that a comment following a statement is not seen as doc comment
                    // for the next statement.
                    if i > 0 {
                        let prev_sibling = children[i - 1];
                        if prev_sibling.end_position().row == comment_line
                            && prev_sibling.kind() != "line_comment"
                        {
                            break;
                        }
                    }
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
///
/// * `node` - Declaration node
/// * `text` - Full source text
/// * `decl_type` - Type of the declaration
///
/// # Returns
///
/// Vector of declarations based on the node
fn create_declarations_from_node(node: Node, text: &str, decl_type: DeclType) -> Vec<Declaration> {
    let doc_comment = extract_doc_comment(node, text);
    let default_value = extract_default_value(node, text);
    extract_signal_names(node, text)
        .into_iter()
        .map(|(name, range)| Declaration {
            name,
            decl_type,
            range: node_to_range(node),
            selection_range: range,
            type_info: extract_type_info(&node, text),
            default_value: default_value.clone(),
            doc_comment: doc_comment.clone(),
            parameters: None,
        })
        .collect()
}

/// Create an instance struct from the component instantiation node
///
/// # Arguments
/// `node` - Component Instantiation Node
/// `text` - Fill source text
///
/// # Returns
/// A struct representing the instance
fn create_instance_from_node(node: Node, text: &str) -> Instance {
    let mut label = "".to_string();
    let mut selection_range = node_to_range(node);
    if let Some(label_decl) = find_child(node, "label_declaration")
        && let Some(label_node) = find_child(label_decl, "label")
    {
        label = text[label_node.byte_range()].to_string();
        selection_range = node_to_range(label_node);
    }
    let instantiated_unit = find_child(node, "instantiated_unit").unwrap_or(node);

    let mut component = "".to_string();
    if let Some(name) = find_child(instantiated_unit, "name") {
        // Check for the library.component name
        if let Some(selection) = find_child(name, "selection") {
            if let Some(iden) = find_child(selection, "identifier") {
                component = text[iden.byte_range()].to_string();
            }
        }
        // Check for the normal component name
        else if let Some(iden) = find_child(name, "identifier") {
            component = text[iden.byte_range()].to_string()
        }
    }

    Instance {
        label,
        component,
        range: node_to_range(node),
        selection_range,
    }
}

/// Create a declaration from a subprogram node (procedure or function).
///
/// # Arguments
///
/// * `node` - Subprogram specification node
/// * `text` - Full source text
/// * `decl_type` - Type of subprogram (Function or Procedure)
///
/// # Returns
///
/// A Declaration for the subprogram
fn create_subprogram_declaration_from_node(
    node: Node,
    text: &str,
    decl_type: DeclType,
) -> Declaration {
    let doc_comment = extract_doc_comment(node, text);
    let mut selection_range = node_to_range(node);
    let mut name = "".to_string();
    let name_node_id = {
        match decl_type {
            DeclType::Function | DeclType::FunctionDeclaration => "function",
            DeclType::Procedure | DeclType::ProcedureDeclaration => "procedure",
            _ => panic!(
                "We should not get here with other decl_type than Function or Procedure variants"
            ),
        }
    };
    if let Some(name_node) = node.child_by_field_name(name_node_id) {
        name = text[name_node.byte_range()].to_string();
        selection_range = node_to_range(name_node);
    }
    let parameters = extract_parameters_from_subprogram(node, text);

    let mut type_info = TypeInfo::new();
    if let DeclType::Function = decl_type
        && let Some(return_type_node) = node.child_by_field_name("type")
    {
        if let Some(id) = find_child(return_type_node, "identifier")
            .or_else(|| find_child(return_type_node, "library_type"))
        {
            type_info.base_type = text[id.byte_range()].to_string();
        } else {
            type_info.base_type = text[return_type_node.byte_range()].to_string();
        }

        if let Some(paren_node) = find_child(return_type_node, "parenthesis_group") {
            type_info.constraints = Some(text[paren_node.byte_range()].to_string());
        }
    }
    // For now we just extract the name of the function.
    Declaration {
        name,
        decl_type,
        range: node_to_range(node),
        selection_range,
        parameters: Some(parameters),
        type_info,
        default_value: None,
        doc_comment,
    }
}

/// Create a scope tree for a subprogram (function or procedure).
///
/// # Arguments
///
/// * `subprogram_node` - Subprogram definition node
/// * `text` - Full source text
///
/// # Returns
///
/// Scope tree for the subprogram
pub fn build_subprogram_scope_tree(subprogram_node: Node, text: &str) -> ScopeTree {
    // We need to check if we have a procedure or a function here.
    let mut scope_kind = ScopeKind::Function;
    let mut spec_node: Option<Node> = None;
    let mut name = "".to_string();
    if let Some(specification_node) = find_child(subprogram_node, "function_specification") {
        spec_node = Some(specification_node);
        if let Some(name_node) = specification_node.child_by_field_name("function") {
            name = text[name_node.byte_range()].to_string();
        }
    } else if let Some(specification_node) = find_child(subprogram_node, "procedure_specification")
    {
        if let Some(name_node) = specification_node.child_by_field_name("procedure") {
            name = text[name_node.byte_range()].to_string();
        }
        spec_node = Some(specification_node);
        scope_kind = ScopeKind::Procedure;
    }
    let mut tree = ScopeTree::new(scope_kind, &subprogram_node);
    tree.name = Some(name);
    if let Some(spec_node) = spec_node {
        tree.declarations
            .extend(extract_parameters_from_subprogram(spec_node, text));
    }

    if let Some(header_node) = find_child(subprogram_node, "subprogram_head") {
        tree.declarations.extend(extract_declaration_from_node(
            header_node,
            text,
            RegionType::Sequential,
            &mut tree.local_usage,
        ));
        for (attr, names) in extract_attr_specs_from_node(header_node, text) {
            tree.attr_specs.entry(attr).or_default().extend(names);
        }
    }
    for child in subprogram_node.children(&mut subprogram_node.walk()) {
        if child.kind() == "sequential_block" {
            process_sequential_statements(child, text, &mut tree);
            break;
        }
    }
    tree.rebuild_index();
    tree
}

/// Create a declaration from a type declaration node.
///
/// # Arguments
///
/// * `node` - Type declaration node
/// * `text` - Full source text
///
/// # Returns
///
/// A Declaration for the type
fn create_type_declaration_from_node(node: Node, text: &str) -> Declaration {
    let doc_comment = extract_doc_comment(node, text);
    let mut selection_range = node_to_range(node);
    let mut name = "".to_string();
    if let Some(id_node) = node.child_by_field_name("type") {
        name = text[id_node.byte_range()].to_string();
        selection_range = node_to_range(id_node);
    }
    let mut parameters = None;
    if let Some(enum_def) = find_child(node, "enumeration_type_definition") {
        parameters = Some(extract_enum_fields_declaration(&enum_def, text));
    }
    if let Some(record_def) = find_child(node, "record_type_definition") {
        parameters = Some(extract_record_fields_declarations(&record_def, text));
    }
    Declaration {
        name,
        decl_type: DeclType::Type,
        range: node_to_range(node),
        selection_range,
        type_info: extract_type_info(&node, text),
        default_value: None,
        doc_comment,
        parameters,
    }
}

/// Create a declaration from a subtype declaration node.
///
/// # Arguments
///
/// * `node` - Subtype declaration node
/// * `text` - Full source text
///
/// # Returns
///
/// A Declaration for the subtype
fn create_subtype_declaration_from_node(node: Node, text: &str) -> Declaration {
    let doc_comment = extract_doc_comment(node, text);
    let mut selection_range = node_to_range(node);
    let mut name = "".to_string();
    if let Some(id_node) = node.child_by_field_name("type") {
        name = text[id_node.byte_range()].to_string();
        selection_range = node_to_range(id_node);
    }
    Declaration {
        name,
        decl_type: DeclType::Subtype,
        range: node_to_range(node),
        parameters: None,
        selection_range,
        type_info: extract_type_info(&node, text),
        default_value: None,
        doc_comment,
    }
}

/// Create a declaration from an alias declaration node.
///
/// # Arguments
///
/// * `node` - Alias declaration node
/// * `text` - Full source text
///
/// # Returns
///
/// A Declaration for the alias
fn create_alias_declaration_from_node(node: Node, text: &str) -> Declaration {
    let doc_comment = extract_doc_comment(node, text);
    let mut selection_range = node_to_range(node);
    let mut name = "".to_string();
    let mut type_info = TypeInfo::new();
    if let Some(id_node) = find_child(node, "identifier") {
        name = text[id_node.byte_range()].to_string();
        selection_range = node_to_range(id_node);
    }
    if let Some(type_node) = find_child(node, "name") {
        type_info.base_type = text[type_node.byte_range()].to_string();
    }
    Declaration {
        name,
        decl_type: DeclType::Alias,
        range: node_to_range(node),
        parameters: None,
        selection_range,
        type_info,
        default_value: None,
        doc_comment,
    }
}

/// Extract parameters from a record definition node.
///
/// Each `element_declaration` inside the record may declare one or more identifiers
/// sharing the same type (e.g., `field_a, field_b : std_logic`). Each identifier is
/// expanded into its own `Declaration` with `DeclType::RecordField`.
///
/// # Arguments
/// * `node` - Record type definition node (`record_type_definition`)
/// * `text` - Full source text
///
/// # Returns
///
/// A vector of all the Declarations for the record fields
fn extract_record_fields_declarations(node: &Node, text: &str) -> Vec<Declaration> {
    let mut fields = Vec::new();
    for element in collect_children(*node, "element_declaration") {
        let doc_comment = extract_doc_comment(element, text);
        let identifiers = extract_signal_names(element, text);
        let type_info = extract_type_info(&element, text);
        for (identifier, range) in identifiers {
            fields.push(Declaration {
                name: identifier,
                decl_type: DeclType::RecordField,
                range: node_to_range(element),
                selection_range: range,
                type_info: type_info.clone(),
                parameters: None,
                default_value: None,
                doc_comment: doc_comment.clone(),
            })
        }
    }

    fields
}

/// Extract enum literals from an enumeration type definition node.
///
/// Each `enumeration_literal` child is extracted into its own `Declaration`
/// with `DeclType::EnumLiteral`. Some literals may be parsed by the grammar
/// as `library_function` or `library_constant` instead of `identifier`
/// (e.g., `STOP`), so all three node kinds are checked.
///
/// # Arguments
/// * `node` - Enumeration type definition node (`enumeration_type_definition`)
/// * `text` - Full source text
///
/// # Returns
///
/// A vector of Declarations for each enum literal
fn extract_enum_fields_declaration(node: &Node, text: &str) -> Vec<Declaration> {
    let mut fields = Vec::new();
    for enum_literal in collect_children(*node, "enumeration_literal") {
        // The grammar may parse some enum literals as identifier, library_function,
        // or library_constant depending on whether they collide with built-in names.
        let identifier_node = find_child(enum_literal, "identifier")
            .or_else(|| find_child(enum_literal, "library_function"))
            .or_else(|| find_child(enum_literal, "library_constant"));
        let identifier = match identifier_node {
            Some(id) => text[id.byte_range()].to_string(),
            None => continue,
        };
        fields.push(Declaration {
            name: identifier,
            decl_type: DeclType::EnumLiteral,
            range: node_to_range(enum_literal),
            selection_range: node_to_range(enum_literal),
            type_info: TypeInfo::new(),
            parameters: None,
            default_value: None,
            doc_comment: None,
        })
    }

    fields
}

/// Create a declaration from an attribute declaration node.
///
/// # Arguments
///
/// * `node` - Alias declaration node
/// * `text` - Full source text
///
/// # Returns
///
/// A Declaration for the alias
fn create_attribute_declaration_from_node(node: Node, text: &str) -> Declaration {
    let doc_comment = extract_doc_comment(node, text);
    let mut selection_range = node_to_range(node);
    let mut name = "".to_string();
    let mut type_info = TypeInfo::new();
    if let Some(id_node) = find_child(node, "identifier") {
        name = text[id_node.byte_range()].to_string();
        selection_range = node_to_range(id_node);
    }
    if let Some(type_node) = find_child(node, "name") {
        type_info.base_type = text[type_node.byte_range()].to_string();
    }
    Declaration {
        name,
        decl_type: DeclType::Attribute,
        range: node_to_range(node),
        parameters: None,
        selection_range,
        type_info,
        default_value: None,
        doc_comment,
    }
}

/// Create a component declaration node from a component_declaration
///
/// # Arguments
/// `node` - component_declaration node
/// `text` - Full source text
///
/// # Returns
/// Declaration for the component
fn create_component_declaration_from_node(node: Node, text: &str) -> Declaration {
    let doc_comment = extract_doc_comment(node, text);
    let mut selection_range = node_to_range(node);
    let mut name = "".to_string();

    if let Some(id_node) = find_child(node, "identifier") {
        name = text[id_node.byte_range()].to_string();
        selection_range = node_to_range(id_node);
    }

    let mut parameters = Vec::new();

    if let Some(body_node) = find_child(node, "component_body") {
        if let Some(generic_clause) = find_child(body_node, "generic_clause") {
            parameters.extend(extract_decl_from_generic_clause(generic_clause, text));
        }
        if let Some(port_clause) = find_child(body_node, "port_clause") {
            parameters.extend(extract_decl_from_port_clause(port_clause, text));
        }
    }

    Declaration {
        name,
        decl_type: DeclType::Component,
        range: node_to_range(node),
        selection_range,
        type_info: TypeInfo::new(),
        default_value: None,
        doc_comment,
        parameters: Some(parameters),
    }
}

/// Extract all the declarations from a node according to its region type.
///
/// Concurrent regions can contain signals, components, types, subtypes, functions, procedures,
/// and constants.
/// Sequential regions can contain variables, constants, functions, procedures, types, and subtypes.
/// The references set collects all identifiers used on the right hand side of declarations.
///
/// # Arguments
///
/// * `node` - Node to extract declarations from
/// * `text` - Full source text
/// * `region_type` - Region type for the given node
/// * `references` - Mutable HashSet to collect used references from declarations
///
/// # Returns
///
/// A vector containing all the declarations extracted
fn extract_declaration_from_node(
    node: Node,
    text: &str,
    region_type: RegionType,
    references: &mut HashSet<Usage>,
) -> Vec<Declaration> {
    let mut declarations = Vec::new();
    for const_decl in collect_descendants(node, "constant_declaration") {
        declarations.extend(create_declarations_from_node(
            const_decl,
            text,
            DeclType::Constant,
        ));
        collect_identifier_from_decl(&const_decl, text, references);
    }
    for type_decl in collect_descendants(node, "type_declaration") {
        let decl = create_type_declaration_from_node(type_decl, text);
        if let Some(ref params) = decl.parameters {
            for param in params {
                if matches!(param.decl_type, DeclType::EnumLiteral) {
                    declarations.push(param.clone());
                }
            }
        }
        declarations.push(decl);
        collect_identifier_from_decl(&type_decl, text, references);
    }
    for subtype_decl in collect_descendants(node, "subtype_declaration") {
        declarations.push(create_subtype_declaration_from_node(subtype_decl, text));
        collect_identifier_from_decl(&subtype_decl, text, references);
    }
    for alias_decl in collect_descendants(node, "alias_declaration") {
        declarations.push(create_alias_declaration_from_node(alias_decl, text));
        collect_identifier_from_decl(&alias_decl, text, references);
    }
    for attr_decl in collect_descendants(node, "attribute_declaration") {
        declarations.push(create_attribute_declaration_from_node(attr_decl, text));
        collect_identifier_from_decl(&attr_decl, text, references);
    }
    // For attribute definition, in theory we do not create any new declaration,
    // instead we are just looking at the identifier and mark them as used
    for attr_def in collect_descendants(node, "attribute_specification") {
        collect_identifiers_recursive(attr_def, text, UsageContext::TypeSpec, references);
    }
    for sub_prog in collect_descendants(node, "subprogram_declaration") {
        if let Some(function_node) = find_child(sub_prog, "function_specification") {
            declarations.push(create_subprogram_declaration_from_node(
                function_node,
                text,
                DeclType::FunctionDeclaration,
            ));
        } else if let Some(proc_node) = find_child(sub_prog, "procedure_specification") {
            declarations.push(create_subprogram_declaration_from_node(
                proc_node,
                text,
                DeclType::ProcedureDeclaration,
            ));
        }
    }
    for sub_prog in collect_descendants(node, "subprogram_definition") {
        if let Some(function_node) = find_child(sub_prog, "function_specification") {
            declarations.push(create_subprogram_declaration_from_node(
                function_node,
                text,
                DeclType::Function,
            ));
        } else if let Some(proc_node) = find_child(sub_prog, "procedure_specification") {
            declarations.push(create_subprogram_declaration_from_node(
                proc_node,
                text,
                DeclType::Procedure,
            ));
        }
    }
    for comp_decl in collect_descendants(node, "component_declaration") {
        declarations.push(create_component_declaration_from_node(comp_decl, text));
    }
    match region_type {
        RegionType::Concurrent => {
            for signal_decl in collect_descendants(node, "signal_declaration") {
                declarations.extend(create_declarations_from_node(
                    signal_decl,
                    text,
                    DeclType::Signal,
                ));
                collect_identifier_from_decl(&signal_decl, text, references);
            }
            // Shared variables can appear in concurrent regions (architecture, block, generate)
            for var_decl in collect_descendants(node, "variable_declaration") {
                declarations.extend(create_declarations_from_node(
                    var_decl,
                    text,
                    DeclType::Variable,
                ));
                collect_identifier_from_decl(&var_decl, text, references);
            }
        }
        RegionType::Sequential => {
            for var_decl in collect_descendants(node, "variable_declaration") {
                declarations.extend(create_declarations_from_node(
                    var_decl,
                    text,
                    DeclType::Variable,
                ));
                collect_identifier_from_decl(&var_decl, text, references);
            }
        }
        // There are no specific addition in the packages for now. Shared
        // variables are a thing but are voluntarily left out for now.
        RegionType::Implementation => {}
    }
    declarations
}

/// Extract parameters from a subprogram specification.
///
/// Expects the node to be a function_specification or procedure_specification.
///
/// # Arguments
///
/// * `node` - The specification node
/// * `text` - The full source text
///
/// # Returns
///
/// A vector of Declaration objects representing all parameters
fn extract_parameters_from_subprogram(node: Node, text: &str) -> Vec<Declaration> {
    // Declaration {
    //     name: "data",
    //     decl_type: DeclType::Parameter(PortDirection::In),
    //     type_info: /* std_logic_vector */,
    //     parameters: None,
    //     ...
    // }
    let mut params = Vec::new();
    if let Some(interface_list_node) =
        navigate_path(node, &["parameter_list_specification", "interface_list"])
    {
        // There are 4 possible cases for subprogram parameters
        // interface_constant_declaration
        // interface_signal_declaration
        // interface_variable_declaration
        // interface_declaration
        // The first 3 are only valid for procedures. The 4th one is valid for both.
        // When getting a simple interface_declaration, we need to parse the internal
        // to define what kind a parameter we have.
        for child in interface_list_node.children(&mut interface_list_node.walk()) {
            let direction = extract_direction_from_interface(child, text);
            let names = extract_signal_names(child, text);
            let doc_comment = extract_doc_comment(child, text);
            let default_value = extract_default_value(child, text);
            let param_type: (PortDirection, Option<ParameterClass>) = match child.kind() {
                "interface_constant_declaration" => {
                    (PortDirection::In, Some(ParameterClass::Constant))
                }
                "interface_variable_declaration" => (direction, Some(ParameterClass::Variable)),
                "interface_signal_declaration" => (direction, Some(ParameterClass::Signal)),
                "interface_file_declaration" => (direction, Some(ParameterClass::File)),
                "interface_declaration" => {
                    let class = if direction == PortDirection::In {
                        ParameterClass::Constant
                    } else {
                        ParameterClass::Variable
                    };
                    (direction, Some(class))
                }
                _ => continue,
            };
            for (name, range) in names {
                params.push(Declaration {
                    name,
                    decl_type: DeclType::Parameter(param_type.0, param_type.1),
                    range: node_to_range(child),
                    selection_range: range,
                    type_info: extract_type_info(&child, text),
                    default_value: default_value.clone(),
                    doc_comment: doc_comment.clone(),
                    parameters: None,
                })
            }
        }
    }

    params
}

/// Parses all `attribute_specification` nodes under `node` and returns a map of
/// attribute name (lowercase) → set of entity names (lowercase) the attribute applies to.
///
/// Uses `"*"` as a sentinel for `all` and `others`.
pub(crate) fn extract_attr_specs_from_node(
    node: Node,
    text: &str,
) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    let mut result: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();

    for attr_spec in collect_descendants(node, "attribute_specification") {
        let Some(attr_node) = attr_spec.child_by_field_name("attribute") else {
            continue;
        };
        let attr_name = text[attr_node.byte_range()].to_lowercase();

        let Some(entity_spec) = find_child(attr_spec, "entity_specification") else {
            continue;
        };
        let Some(name_list) = find_child(entity_spec, "entity_name_list") else {
            continue;
        };

        let entry = result.entry(attr_name).or_default();
        let mut cursor = name_list.walk();
        for child in name_list.children(&mut cursor) {
            match child.kind() {
                "ALL" | "OTHERS" => {
                    entry.insert("*".to_string());
                }
                "entity_designator" => {
                    if let Some(id) = find_child(child, "identifier") {
                        entry.insert(text[id.byte_range()].to_lowercase());
                    }
                }
                _ => {}
            }
        }
    }

    result
}
