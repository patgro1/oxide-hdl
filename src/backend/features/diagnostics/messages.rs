//! Diagnostic message factory.
//!
//! Centralized creation of all diagnostic messages with consistent
//! formatting, severity levels, and tags.

use crate::analysis::{DeclType, Declaration};
use crate::backend::features::diagnostics::DiagnosticMessage;
use crate::utils::node_to_range;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, DiagnosticTag, Position, Range};
use tree_sitter::Node;

/// Creates a diagnostic for an unused identifier (signal, variable, constant, etc.).
///
/// Severity: HINT (code quality issue, not functional problem)
/// Tagged: UNNECESSARY (enables editor strikethrough/graying)
///
/// # Arguments
///
/// * `decl` - The unused declaration with name, type, and location
///
/// # Returns
///
/// Diagnostic pointing to the declaration with an appropriate message based on its type
pub fn unused_identifier(decl: Declaration) -> Diagnostic {
    Diagnostic {
        range: decl.range,
        severity: Some(DiagnosticSeverity::HINT),
        source: Some("oxide-hdl".to_string()),
        tags: vec![DiagnosticTag::UNNECESSARY].into(),
        message: match decl.decl_type {
            DeclType::Generic => format!("Unused generic '{}'", decl.name),
            DeclType::Port(_) => format!("Unused port '{}'", decl.name),
            DeclType::Variable => format!("Unused variable '{}'", decl.name),
            DeclType::Constant => format!("Unused constant '{}'", decl.name),
            DeclType::Signal => format!("Unused signal '{}'", decl.name),
            DeclType::Function => format!("Unused function '{}'", decl.name),
            DeclType::Procedure => format!("Unused procedure '{}'", decl.name),
            DeclType::Type => format!("Unused type '{}'", decl.name),
            DeclType::Subtype => format!("Unused type '{}'", decl.name),
        },
        ..Default::default()
    }
}

/// Creates a diagnostic for a signal that is read but missing from the sensitivity list.
///
/// Severity: WARNING (functional issue - can cause simulation/synthesis mismatch)
///
/// # Arguments
///
/// * `signal` - Name of the signal that is read but not in sensitivity list
/// * `process_node` - Tree-sitter node of the process statement (for location)
///
/// # Returns
///
/// Diagnostic pointing to the process header
pub fn missing_sensitivity(signal: &str, process_node: &Node) -> Diagnostic {
    Diagnostic {
        range: node_to_range(*process_node),
        severity: Some(DiagnosticSeverity::WARNING),
        message: format!("Signal '{}' is read but not in sensitivity list", signal),
        source: Some("oxide-hdl-sensitivity".to_string()),
        ..Default::default()
    }
}

/// Creates a diagnostic for a signal in the sensitivity list that is not read.
///
/// Severity: HINT (code quality - unnecessary but not harmful)
/// Tagged: UNNECESSARY (enables editor strikethrough/graying)
///
/// # Arguments
///
/// * `name` - Name of the unnecessary signal
/// * `range` - Location of the signal in the sensitivity list
///
/// # Returns
///
/// Diagnostic pointing to the specific signal in the sensitivity list
pub fn unnecessary_sensitivity(name: &str, range: &Range) -> Diagnostic {
    Diagnostic {
        range: *range,
        severity: Some(DiagnosticSeverity::HINT),
        message: format!("Signal '{}' is not needed in sensitivity list", name),
        source: Some("oxide-hdl-sensitivity".to_string()),
        tags: vec![DiagnosticTag::UNNECESSARY].into(),
        ..Default::default()
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
/// * `insertAtTheEnd` - If true, the message diagnostic is inserted at the end of the node
/// * `text` - The full source text (needed to trim whitespace)
///
/// # Returns
///
/// An LSP `Diagnostic` object with ERROR severity.
pub fn syntax_error(
    node: Node,
    message: DiagnosticMessage,
    insert_at_the_end: bool,
    text: &str,
) -> Diagnostic {
    let range = if insert_at_the_end {
        find_end_of_range(node, text)
    } else {
        node_to_range(node)
    };

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("oxide-hdl".to_string()),
        message: message.to_string(),
        ..Default::default()
    }
}

/// Finds the position where a missing delimiter should be (end of actual content).
///
/// Used for syntax errors like missing semicolons or unmatched parentheses,
/// where the diagnostic should point to where the delimiter is expected
/// (after the last token), not where the node ends (which may include whitespace).
///
/// # Arguments
///
/// * `node` - Tree-sitter ERROR node or incomplete statement
/// * `text` - Full source text
///
/// # Returns
///
/// Range pointing to the position just after the last non-whitespace character,
/// where the missing delimiter should have been
///
/// # Example
///
/// ```text
/// signal foo   \n     ← ERROR node includes trailing whitespace
///           ^         ← Diagnostic points here: "Expected ';'"
/// ```
fn find_end_of_range(node: Node, text: &str) -> Range {
    let node_text = &text[node.byte_range()];
    let trimmed = node_text.trim_end();

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

    Range {
        start: Position {
            line: line as u32,
            character: col as u32,
        },
        end: Position {
            line: line as u32,
            character: (col + 1) as u32,
        },
    }
}
