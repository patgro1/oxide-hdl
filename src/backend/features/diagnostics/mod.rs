//! Diagnostic collection system for VHDL syntax and semantic errors.
//!
//! This module provides the infrastructure for detecting and reporting various types
//! of errors in VHDL code, including syntax errors, missing semicolons, unmatched
//! parentheses, and semantic issues like missing types or port directions.

// pub mod sensitivity;
pub mod syntax;
pub mod unused;

use crate::analysis::{Analysis, ScopeTree};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
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
pub fn collect_all_diagnostics(root: Node, analysis: &Analysis, text: &str) -> Vec<Diagnostic> {
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
                    walk_node(child, text, scope_tree, analysis, &mut collectors);
                    arch_index += 1;
                } else {
                    walk_node(child, text, None, analysis, &mut collectors);
                }
            }
        } else {
            walk_node(node, text, None, analysis, &mut collectors);
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
) {
    check_node(node, text, scope_tree, analysis, collectors);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, text, scope_tree, analysis, collectors);
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
            // sensitivity::check_process_sensitivity(node, text, collectors);
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

/// Creates a diagnostic at the node's location.
///
/// Generates an LSP diagnostic with the error positioned at the start of
/// the provided node. Used for errors where the entire node is problematic.
///
/// # Arguments
///
/// * `node` - The Tree-sitter node where the error occurred
/// * `message` - The type of diagnostic message to generate
///
/// # Returns
///
/// An LSP `Diagnostic` object with ERROR severity.
fn create_diagnostic(node: Node, message: DiagnosticMessage) -> Diagnostic {
    Diagnostic {
        range: crate::backend::utils::node_to_range(node),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("oxide-hdl".to_string()),
        message: message.to_string(),
        ..Default::default()
    }
}

/// Creates a diagnostic at the end of a node's actual content.
///
/// Useful for "missing semicolon" or similar errors where the diagnostic
/// should appear at the location where something is missing, not at the
/// start of the construct. Accounts for trailing whitespace by finding
/// the last non-whitespace character.
///
/// # Arguments
///
/// * `node` - The Tree-sitter node to analyze
/// * `text` - The full source text (needed to trim whitespace)
/// * `message` - The type of diagnostic message to generate
///
/// # Returns
///
/// An LSP `Diagnostic` positioned at the end of the node's content.
///
/// # Examples
///
/// ```ignore
/// // For missing semicolon after port clause:
/// // port (
/// //     clk : in std_logic
/// // )           ← diagnostic appears here
/// ```
fn create_diagnostic_at_end(node: Node, text: &str, message: DiagnosticMessage) -> Diagnostic {
    let node_text = &text[node.byte_range()];
    let trimmed = node_text.trim_end();

    // If node_text has trailing whitespace, the end_position might be past the actual content
    let lines: Vec<&str> = trimmed.lines().collect();

    let (line, col) = if let Some(last_line) = lines.last() {
        // Calculate which line the last content is on
        let num_lines = lines.len();
        let line = node.start_position().row + num_lines - 1;

        // Column is at the end of the last line
        let col = if num_lines == 1 {
            // Single line: add to start column
            node.start_position().column + last_line.len()
        } else {
            // Multi-line: column is just the last line length
            last_line.len()
        };

        (line, col)
    } else {
        // Empty node, use start position
        (node.start_position().row, node.start_position().column)
    };

    Diagnostic {
        range: Range {
            start: Position {
                line: line as u32,
                character: col as u32,
            },
            end: Position {
                line: line as u32,
                character: (col + 1) as u32,
            },
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("oxide-hdl".to_string()),
        message: message.to_string(),
        ..Default::default()
    }
}
