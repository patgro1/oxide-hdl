pub mod ast;
use lazy_static::lazy_static;
use std::sync::Mutex;
use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::Node;

lazy_static! {
    /// Global lock for tree-sitter parser usage.
    ///
    /// Tree-sitter parsers use unsafe C FFI code that isn't thread-safe.
    /// This global lock ensures only one parser operates at a time, preventing
    /// memory corruption when multiple operations (parsing, rename, completion, etc.)
    /// happen concurrently.
    ///
    /// **CRITICAL**: All code that creates or uses a tree-sitter Parser MUST
    /// acquire this lock first, both in production and tests.
    pub static ref PARSER_LOCK: Mutex<()> = Mutex::new(());
}

/// Converts a Tree-sitter `Node` into a standard LSP `Range`.
///
/// Tree-sitter uses row/column (0-indexed), which matches the LSP protocol.
///
/// # Arguments
/// * `node` - The Tree-sitter node to convert.
///
/// # Returns
/// An LSP `Range` struct covering the start and end positions of the node.
pub fn node_to_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start: Position {
            line: start.row as u32,
            character: start.column as u32,
        },
        end: Position {
            line: end.row as u32,
            character: end.column as u32,
        },
    }
}

/// Check if a position is within a range
///
/// # Arguments
/// `pos` - The position we are checking
/// `range` - The range we need to check if the position is contained
///
/// # Returns
/// True if position is within the given range
pub fn position_in_range(pos: Position, range: Range) -> bool {
    let is_after_start = pos.line > range.start.line
        || (pos.line == range.start.line && pos.character >= range.start.character);
    let is_before_end = pos.line < range.end.line
        || (pos.line == range.end.line && pos.character <= range.end.character);

    is_after_start && is_before_end
}
