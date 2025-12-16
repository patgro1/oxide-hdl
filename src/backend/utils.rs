use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::Node;
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
