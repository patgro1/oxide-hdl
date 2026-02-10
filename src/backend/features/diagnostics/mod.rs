//! Diagnostic collection system for VHDL syntax and semantic errors.
//!
//! This module provides the infrastructure for detecting and reporting various types
//! of errors in VHDL code, including syntax errors, missing semicolons, unmatched
//! parentheses, undeclared identifiers, and semantic issues like missing types or
//! port directions.
//!
//! * [`messages`]: Diagnostic message factory
//! * [`sensitivity`]: Sensitivity list validation
//! * [`syntax`]: Syntax error detection
//! * [`undeclared`]: Undeclared identifier detection
//! * [`unused`]: Unused signal/variable detection

pub mod messages;
pub mod sensitivity;
pub mod syntax;
pub mod undeclared;
pub mod unused;

use crate::{
    analysis::{Analysis, ScopeTree},
    backend::AnalysisMap,
};
use std::{cmp::Ordering, collections::HashMap};
use tower_lsp::lsp_types::{Diagnostic, Range, Url};
use tree_sitter::Node;

/// The kind identifier for ERROR nodes in the Tree-sitter AST.
const ERROR_NODE_KIND: &str = "ERROR";

/// Types of diagnostic messages that can be generated.
///
/// Each variant represents a specific type of error that can be detected
/// in VHDL code. The messages are designed to be user-friendly and actionable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiagnosticMessage {
    /// Generic syntax error detected by Tree-sitter parser.
    SyntaxError,

    // Semantic - declarations
    /// Signal declaration is missing its type specification.
    SignalMissingType,

    /// Port declaration is missing direction (in/out/inout/buffer).
    PortMissingDirection,

    /// Generic missing semicolon error.
    MissingSemicolon,

    /// Missing semicolon after port clause.
    MissingSemiColonAfterPort,

    /// Missing semicolon after component instantiation.
    MissingSemicolonAfterInstance,

    /// Label statement is invalid or attached to wrong construct.
    InvalidLabelStatement,

    /// Unmatched parentheses in port map or similar construct.
    UnmatchedParentheses,
    // Futures??
    // UndefinedSignal(String)
    // UnusedSignal(String)
}

/// Collection of diagnostics organized by category.
///
/// Separates diagnostics into different vectors based on their type,
/// allowing for easier filtering and configuration in the future.
pub struct DiagnosticCollectors {
    /// Syntax and structural errors.
    pub syntax: Vec<Diagnostic>,

    /// Unused signal/variable warnings (future).
    pub unused: Vec<Diagnostic>,

    /// Undefined symbol errors (future).
    pub undefined: Vec<Diagnostic>,

    /// Process sensitivity list errors (future).
    pub sensitivity: Vec<Diagnostic>,
}

impl std::fmt::Display for DiagnosticMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::SyntaxError => write!(f, "Syntax error"),
            Self::SignalMissingType => write!(f, "Signal declaration missing type"),
            Self::PortMissingDirection => write!(f, "Port missing direction"),
            Self::MissingSemicolon
            | Self::MissingSemiColonAfterPort
            | Self::MissingSemicolonAfterInstance => {
                write!(f, "Missing ;")
            }
            Self::InvalidLabelStatement => write!(f, "Invalid label statement"),
            Self::UnmatchedParentheses => write!(f, "Missing )"),
        }
    }
}

impl DiagnosticCollectors {
    /// Creates a new empty diagnostic collector.
    ///
    /// # Returns
    ///
    /// A `DiagnosticCollectors` instance with empty vectors for all categories.
    pub fn new() -> Self {
        Self {
            syntax: Vec::new(),
            unused: Vec::new(),
            undefined: Vec::new(),
            sensitivity: Vec::new(),
        }
    }

    /// Combines all diagnostic categories into a single vector.
    ///
    /// Consumes the collector and returns all diagnostics in a flat list,
    /// suitable for publishing to the LSP client.
    ///
    /// # Returns
    ///
    /// A vector containing all diagnostics from all categories.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        let mut all = Vec::new();
        all.extend(self.syntax);
        all.extend(self.unused);
        all.extend(self.undefined);
        all.extend(self.sensitivity);

        all.sort_by(|a, b| match compare_range(&a.range, &b.range) {
            Ordering::Equal => a.message.cmp(&b.message),
            other => other,
        });

        all.dedup_by(|a, b| {
            a.range == b.range && a.message == b.message && a.severity == b.severity
        });

        all
    }
}

/// Collects all diagnostics for a parsed VHDL file.
///
/// This is the main entry point for diagnostic collection. It walks the
/// Tree-sitter AST and applies all enabled checks, collecting errors and
/// warnings into a single list.
///
/// # Arguments
///
/// * `root` - The root node the collect diagnostic on
/// * `analysis` - The analysis of the current document
/// * `text` - The full source text of the file
///
/// # Returns
///
/// A vector of LSP `Diagnostic` objects representing all detected issues.
///
/// # Examples
///
/// ```ignore
/// let tree = parser.parse(source_code, None)?;
/// let diagnostics = collect_all_diagnostics(tree.root_node(), analysis, source_code);
/// client.publish_diagnostics(uri, diagnostics, None).await;
/// ```
pub fn collect_all_diagnostics(
    root: Node,
    analysis: &Analysis,
    text: &str,
    global_map: &AnalysisMap,
    current_uri: &Url,
) -> Vec<Diagnostic> {
    let mut collectors = DiagnosticCollectors::new();

    if root.kind() == ERROR_NODE_KIND {
        syntax::check_syntax_error(root, &mut collectors);
    }
    let mut cursor = root.walk();
    let mut arch_index = 0;

    for node in root.children(&mut cursor) {
        if node.kind() == "design_unit" {
            for child in node.children(&mut node.walk()) {
                if child.kind() == "architecture_definition" {
                    let scope_tree = analysis.scope_trees.get(arch_index);
                    walk_node(
                        child,
                        text,
                        scope_tree,
                        analysis,
                        &mut collectors,
                        global_map,
                        current_uri,
                    );
                    arch_index += 1;
                } else if child.kind() == "entity_declaration" {
                    let entity_scope = child
                        .child_by_field_name("entity")
                        .map(|n| &text[n.byte_range()])
                        .and_then(|name| analysis.entity_scope_trees.get(name));
                    walk_node(
                        child,
                        text,
                        entity_scope,
                        analysis,
                        &mut collectors,
                        global_map,
                        current_uri,
                    );
                } else {
                    walk_node(
                        child,
                        text,
                        None,
                        analysis,
                        &mut collectors,
                        global_map,
                        current_uri,
                    );
                }
            }
        } else {
            walk_node(
                node,
                text,
                None,
                analysis,
                &mut collectors,
                global_map,
                current_uri,
            );
        }
    }

    collectors.into_diagnostics()
}

/// Recursively walks the AST tree and checks each node for errors.
///
/// Performs a depth-first traversal of the syntax tree, calling `check_node`
/// on each node and then recursing into its children.
///
/// # Arguments
///
/// * `node` - The current Tree-sitter node to check
/// * `scope_trees` - Vector containing all the scope trees of the document
/// * `text` - The full source text of the file
/// * `collectors` - Mutable reference to diagnostic collectors
fn walk_node(
    node: Node,
    text: &str,
    scope_tree: Option<&ScopeTree>,
    analysis: &Analysis,
    collectors: &mut DiagnosticCollectors,
    global_map: &AnalysisMap,
    current_uri: &Url,
) {
    check_node(
        node,
        text,
        scope_tree,
        analysis,
        collectors,
        global_map,
        current_uri,
    );

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(
            child,
            text,
            scope_tree,
            analysis,
            collectors,
            global_map,
            current_uri,
        );
    }
}

/// Dispatches appropriate checks based on node type.
///
/// This function acts as a router, examining the node's kind and calling
/// the appropriate validation functions. ERROR nodes are always checked,
/// while semantic checks are only applied to nodes without structural errors.
///
/// # Arguments
///
/// * `node` - The Tree-sitter node to check
/// * `scope_trees` - Vector containing all the scope trees of the document
/// * `text` - The full source text of the file
/// * `collectors` - Mutable reference to diagnostic collectors
///
/// # Node Types Checked
///
/// - `ERROR` - Unparseable syntax
/// - `signal_declaration` - Signal type and semicolon
/// - `interface_declaration` - Port direction
/// - `port_clause` - Semicolon after ports
/// - `component_instantiation_statement` - Semicolon after instance
/// - `label_declaration` - Valid label target
/// - `sensitivity_specification` - Matching parentheses
/// - `association_list` - Matching parentheses in port maps
fn check_node(
    node: Node,
    text: &str,
    scope_tree: Option<&ScopeTree>,
    analysis: &Analysis,
    collectors: &mut DiagnosticCollectors,
    global_map: &AnalysisMap,
    current_uri: &Url,
) {
    if node.kind() == ERROR_NODE_KIND {
        syntax::check_syntax_error(node, collectors);
    }
    if node.is_error() {
        return;
    }

    match node.kind() {
        "architecture_definition" => {
            if let Some(scope_tree) = scope_tree {
                unused::check_unused_signals(scope_tree, collectors);
                let mut lookup_cache: HashMap<String, bool> = HashMap::new();
                // Create the context bundle
                let ctx = undeclared::ValidationContext {
                    root: node, // The architecture node acts as the "root" for this scope's search
                    global_map,
                    current_uri,
                };

                // Call with just 4 arguments
                undeclared::check_undeclared_identifiers(
                    &ctx,
                    scope_tree,
                    collectors,
                    &mut lookup_cache,
                );
            }
        }
        "entity_declaration" => {
            if let Some(scope_tree) = scope_tree {
                let mut lookup_cache: HashMap<String, bool> = HashMap::new();
                let ctx = undeclared::ValidationContext {
                    root: node,
                    global_map,
                    current_uri,
                };
                undeclared::check_undeclared_identifiers(
                    &ctx,
                    scope_tree,
                    collectors,
                    &mut lookup_cache,
                );
            }
        }
        "signal_declaration" => {
            syntax::check_signal_declaration(node, collectors);
            syntax::check_end_with_semicolon(
                node,
                text,
                collectors,
                DiagnosticMessage::MissingSemicolon,
            )
        }
        "process_statement" => {
            sensitivity::check_process_sensitivity(
                node,
                text,
                scope_tree,
                analysis,
                collectors,
                global_map,
                current_uri,
            );
        }
        "interface_declaration" => {
            syntax::check_port_declaration(node, collectors);
        }
        "port_clause" => {
            syntax::check_end_with_semicolon(
                node,
                text,
                collectors,
                DiagnosticMessage::MissingSemiColonAfterPort,
            );
        }
        "component_instantiation_statement" => {
            syntax::check_end_with_semicolon(
                node,
                text,
                collectors,
                DiagnosticMessage::MissingSemicolonAfterInstance,
            );
        }
        "label_declaration" => {
            syntax::check_label_has_valid_parent(node, collectors);
        }
        "sensitivity_specification" => {
            syntax::check_sensitivity_parens(node, text, collectors);
        }
        "association_list" => {
            syntax::check_association_list_parens(node, text, collectors);
        }
        _ => {}
    }
}

/// Helper function to compare two ranges for sorting.
fn compare_range(a: &Range, b: &Range) -> Ordering {
    match a.start.line.cmp(&b.start.line) {
        Ordering::Equal => match a.start.character.cmp(&b.start.character) {
            Ordering::Equal => match a.end.line.cmp(&b.end.line) {
                Ordering::Equal => a.end.character.cmp(&b.end.character),
                other => other,
            },
            other => other,
        },
        other => other,
    }
}
