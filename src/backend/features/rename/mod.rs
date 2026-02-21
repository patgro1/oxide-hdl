//! Symbol rename functionality.
//!
//! Provides file-scoped rename operations for VHDL symbols. Finds all occurrences
//! of a symbol within the current file and generates text edits to rename them.
//!
//! # Limitations
//!
//! - **File-scoped only**: Does not rename across multiple files
//! - **No workspace tracking**: Entities and packages in other files are not updated
//!
//! # Future Enhancements
//!
//! - Workspace-wide rename when usage tracking is implemented
//! - Cross-file entity/component renames
//! - Validation of naming conflicts

use crate::{
    analysis::{Analysis, ScopeKind, ScopeTree, Usage},
    utils::node_to_range,
};
use lazy_static::lazy_static;
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};
use tree_sitter::{Parser, Point};

lazy_static! {
    /// VHDL reserved keywords (VHDL-2008).
    /// Identifiers cannot use these names (case-insensitive).
    static ref VHDL_KEYWORDS: HashSet<&'static str> = {
        let mut set = HashSet::new();
        // VHDL-93 and VHDL-2008 reserved words
        let keywords = [
            "abs", "access", "after", "alias", "all", "and", "architecture", "array",
            "assert", "assume", "assume_guarantee", "attribute", "begin", "block",
            "body", "buffer", "bus", "case", "component", "configuration", "constant",
            "context", "cover", "default", "disconnect", "downto", "else", "elsif",
            "end", "entity", "exit", "fairness", "file", "for", "force", "function",
            "generate", "generic", "group", "guarded", "if", "impure", "in", "inertial",
            "inout", "is", "label", "library", "linkage", "literal", "loop", "map",
            "mod", "nand", "new", "next", "nor", "not", "null", "of", "on", "open",
            "or", "others", "out", "package", "parameter", "port", "postponed",
            "procedure", "process", "property", "protected", "pure", "range", "record",
            "register", "reject", "release", "rem", "report", "restrict",
            "restrict_guarantee", "return", "rol", "ror", "select", "sequence",
            "severity", "signal", "shared", "sla", "sll", "sra", "srl", "strong",
            "subtype", "then", "to", "transport", "type", "unaffected", "units",
            "until", "use", "variable", "vmode", "vprop", "vunit", "wait", "when",
            "while", "with", "xnor", "xor",
        ];
        for keyword in &keywords {
            set.insert(*keyword);
        }
        set
    };
}

/// Checks if the symbol at the given position can be renamed.
///
/// A symbol can be renamed if:
/// - It's a valid identifier (not a keyword)
/// - It's declared in the current file
/// - It's in a renameable scope (signal, variable, port, generic, etc.)
///
/// # Arguments
///
/// * `text` - The source text
/// * `position` - The cursor position
/// * `analysis` - The full analysis containing all scope trees
/// * `parser` - The tree-sitter parser (must be locked by caller)
///
/// # Returns
///
/// `Some(Range)` if the symbol can be renamed (range of the symbol name)
/// `None` if rename is not possible
pub async fn prepare_rename(
    text: &str,
    position: &Position,
    analysis: &Analysis,
    parser: &mut Parser,
) -> Option<Range> {
    let language = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;

    let start_point = Point {
        row: position.line as usize,
        column: position.character as usize,
    };
    if let Some(node) = tree
        .root_node()
        .descendant_for_point_range(start_point, start_point)
    {
        let name = &text[node.byte_range()];
        if analysis.find_declaration_at(name, position).is_some() {
            return Some(node_to_range(node));
        }
    }
    None
}

/// Performs a rename operation on a symbol within the current file.
///
/// Finds all occurrences of the symbol (declaration + all usages) and creates
/// text edits to replace them with the new name.
///
/// # Arguments
///
/// * `text` - The source text
/// * `position` - The cursor position where rename was triggered
/// * `new_name` - The new name for the symbol
/// * `analysis` - The full analysis containing all scope trees
/// * `current_uri` - The URI of the current file
/// * `parser` - The tree-sitter parser (must be locked by caller)
///
/// # Returns
///
/// `Some(WorkspaceEdit)` containing all text edits for the rename
/// `None` if rename is not possible
pub async fn rename_symbol(
    text: &str,
    position: &Position,
    new_name: &str,
    analysis: &Analysis,
    current_uri: &Url,
    parser: &mut Parser,
) -> Option<WorkspaceEdit> {
    if !is_valid_vhdl_identifier(new_name) {
        return None;
    }

    // Parse to find the identifier at cursor position
    let language = unsafe { crate::tree_sitter_vhdl() };
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;

    let start_point = Point {
        row: position.line as usize,
        column: position.character as usize,
    };

    let mut node = tree
        .root_node()
        .descendant_for_point_range(start_point, start_point)?;

    // If the node isn't an identifier, recursively search for the innermost identifier at cursor
    if node.kind() != "identifier" && node.kind() != "library_type" {
        fn find_identifier_at_point(
            node: tree_sitter::Node,
            point: tree_sitter::Point,
        ) -> Option<tree_sitter::Node> {
            if (node.kind() == "identifier" || node.kind() == "library_type")
                && node.start_position() <= point
                && node.end_position() > point
            {
                return Some(node);
            }
            for child in node.children(&mut node.walk()) {
                if let Some(found) = find_identifier_at_point(child, point) {
                    return Some(found);
                }
            }
            None
        }

        node = find_identifier_at_point(node, start_point)?;
    }

    let symbol_name = &text[node.byte_range()];

    let declaration = analysis.find_declaration_at(symbol_name, position);

    if let Some(declaration) = declaration {
        let scope_tree = analysis.find_scope_tree_at(&declaration.range.start);

        if let Some(scope_tree) = scope_tree {
            let scope_tree = scope_tree.find_innermost_scope(&declaration.range.start);
            let mut edits: Vec<TextEdit> = Vec::new();

            // Determine which scope trees to search based on declaration location
            let base_scopes: Vec<&ScopeTree> = if scope_tree.kind == ScopeKind::Entity {
                // For entity declarations (ports/generics), include the entity itself
                // plus all architectures that reference it
                let mut scopes: Vec<&ScopeTree> = vec![scope_tree];
                scopes.extend(
                    analysis
                        .scope_trees
                        .iter()
                        .filter(|s| s.entity == scope_tree.name),
                );
                scopes
            } else {
                vec![scope_tree]
            };

            // Collect all usages from the relevant scope trees
            let usages: Vec<Usage> = base_scopes
                .iter()
                .flat_map(|st| collect_all_usage(symbol_name, st))
                .collect();

            // Create text edits for the declaration and all usages
            edits.push(TextEdit {
                range: declaration.selection_range,
                new_text: new_name.to_string(),
            });
            for usage in usages {
                edits.push(TextEdit {
                    range: usage.range,
                    new_text: new_name.to_string(),
                });
            }

            let mut changes = HashMap::new();
            changes.insert(current_uri.clone(), edits);

            return Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            });
        }
    }

    None
}

/// Recursively collects all usages of a symbol within a scope tree.
///
/// Walks the scope tree and its children, collecting all usage references
/// that match the given name (case-insensitive).
///
/// # Arguments
///
/// * `name` - The symbol name to search for
/// * `scope_tree` - The scope tree to search within
///
/// # Returns
///
/// Vector of all matching usages found in the scope tree and its descendants
fn collect_all_usage(name: &str, scope_tree: &ScopeTree) -> Vec<Usage> {
    let mut usages: Vec<Usage> = Vec::new();

    usages.extend(
        scope_tree
            .local_usage
            .iter()
            .filter(|u| u.name.eq_ignore_ascii_case(name))
            .cloned(),
    );

    for child in &scope_tree.children {
        usages.extend(collect_all_usage(name, child));
    }

    usages
}

/// Validates that a name is a valid VHDL identifier.
///
/// VHDL basic identifiers must:
/// - Start with a letter (a-z, A-Z)
/// - Contain only letters, digits (0-9), and underscores (_)
/// - Not end with an underscore
/// - Not have consecutive underscores
/// - Not be empty
/// - Not be a reserved keyword (case-insensitive)
///
/// Extended identifiers (enclosed in backslashes) are not supported.
fn is_valid_vhdl_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let mut chars = name.chars();

    // First character must be a letter
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }

    // Check remaining characters
    let mut prev_was_underscore = false;
    for c in chars {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' => {
                prev_was_underscore = false;
            }
            '_' => {
                // Consecutive underscores not allowed
                if prev_was_underscore {
                    return false;
                }
                prev_was_underscore = true;
            }
            _ => return false, // Invalid character
        }
    }

    // Cannot end with underscore
    if name.ends_with('_') {
        return false;
    }

    // Check if it's a reserved keyword (case-insensitive)
    let lowercase_name = name.to_lowercase();
    !VHDL_KEYWORDS.contains(lowercase_name.as_str())
}

#[cfg(test)]
mod tests;
