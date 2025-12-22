//! VHDL semantic analysis and scope management.
//!
//! This module provides the core analysis infrastructure for understanding
//! VHDL code structure, including scope trees, declaration tracking, and
//! symbol resolution.

mod builders;
mod scope_tree;
mod types;

pub use builders::*;
pub use scope_tree::*;
pub use types::*;

use crate::backend::utils::node_to_range;
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::Range;
use tree_sitter::{Language, Node};

#[cfg(test)]
mod tests;

#[allow(dead_code)]
unsafe extern "C" {
    /// External declaration for the tree-sitter-vhdl language function.
    /// This is required to initialize the Tree-sitter parser with the VHDL grammar.
    fn tree_sitter_vhdl() -> Language;
}

/// Represents the analysis result for a single VHDL file.
///
/// Contains a lookup map of all top-level symbols found in the file.
#[derive(Debug, Clone)]
pub struct Analysis {
    /// Map of top-level symbol names (normalized to lowercase) to the `Symbol` struct.
    ///
    /// Using lowercase keys ensures case-insensitive lookup, while the `Symbol` struct
    /// preserves the original display name.
    pub symbols: HashMap<String, Symbol>,

    /// How the file was parsed
    pub parse_level: ParseLevel,

    /// Scope tree with entity declaration
    pub entity_scope_trees: HashMap<String, ScopeTree>,

    /// Scope tree with signals, constants, types declaration and usage
    pub scope_trees: Vec<ScopeTree>,
}

impl Analysis {
    /// Creates a new, empty Analysis struct.
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            parse_level: ParseLevel::Shallow,
            entity_scope_trees: HashMap::new(),
            scope_trees: Vec::new(),
        }
    }

    /// Recursively searches for a symbol by name anywhere in the file's hierarchy.
    ///
    /// This method checks the top-level symbols first, and then recursively searches
    /// the children of all top-level symbols. This allows finding nested signals
    /// inside Architectures or variables inside Processes.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the symbol to find (case-insensitive).
    ///
    /// # Returns
    ///
    /// * `Some(&Symbol)` - A reference to the found symbol.
    /// * `None` - If the symbol was not found.
    pub fn find_symbol(&self, name: &str) -> Option<&Symbol> {
        let target = name.to_lowercase();
        if let Some(s) = self.symbols.get(&target) {
            return Some(s);
        }
        for s in self.symbols.values() {
            if let Some(found) = s.find_recursive(&target) {
                return Some(found);
            }
        }
        None
    }

    /// Gives the list of visible declaration from a node.
    ///
    /// # Arguments
    ///
    /// * `arch_scope` - ScopeTree of the architecture containing the target
    /// * `target` - Range of the targeted node we inquiriy visible declaration on
    ///
    /// # Returns
    /// A vector of Declaration containing all seen declaration if any, None otherwise
    pub fn collect_visible_declarations(
        &self,
        arch_scope: &ScopeTree,
        target: Range,
    ) -> Option<Vec<Declaration>> {
        let entity_scope = arch_scope
            .entity
            .as_ref()
            .and_then(|name| self.entity_scope_trees.get(name));
        arch_scope.collect_visible_declarations(&target, entity_scope)
    }
}

/// Recursively collects all identifier references in a subtree.
///
/// Walks the tree depth-first, collecting all identifier nodes (which
/// represent references to signals, variables, etc.).
///
/// # Arguments
///
/// * `node` - Root node to search from
/// * `text` - Full source text
/// * `references` - Mutable set to collect identifier names into
pub fn collect_identifiers_recursive(
    node: Node,
    text: &str,
    context: UsageContext,
    references: &mut HashSet<Usage>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            references.insert(Usage {
                name: text[child.byte_range()].to_string().to_lowercase(),
                context,
                range: node_to_range(child),
            });
        } else {
            collect_identifiers_recursive(child, text, context, references);
        }
    }
}
