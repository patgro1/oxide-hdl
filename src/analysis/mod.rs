//! VHDL semantic analysis and scope management.
//!
//! This module provides the core analysis infrastructure for understanding
//! VHDL code structure, including scope trees, declaration tracking, and
//! symbol resolution.

mod builders;
mod builtins;
mod scope_tree;
mod types;

pub use builders::*;
pub use builtins::*;
pub use scope_tree::*;
pub use types::*;

use crate::utils::{node_to_range, position_in_range};
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{Position, Range};
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

    /// List of all the use clause found in the file
    pub use_clauses: Vec<UseClause>,

    /// How the file was parsed
    pub parse_level: ParseLevel,

    /// Scope tree with entity declaration
    pub entity_scope_trees: HashMap<String, ScopeTree>,

    /// Scope tree with signals, constants, types declaration and usage
    pub scope_trees: Vec<ScopeTree>,

    /// Scope tree for packages
    pub package_scope_trees: HashMap<String, ScopeTree>,
}

impl Analysis {
    /// Creates a new, empty Analysis struct.
    pub fn new() -> Self {
        let implicit_standard = UseClause {
            library: "std".to_string(),
            name: "standard".to_string(),
            all_import: true,
            range: Range::default(),
            imported_symbol: None,
        };
        Self {
            symbols: HashMap::new(),
            parse_level: ParseLevel::Shallow,
            entity_scope_trees: HashMap::new(),
            scope_trees: Vec::new(),
            package_scope_trees: HashMap::new(),
            use_clauses: vec![implicit_standard],
        }
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

    /// Find the scope tree for a given position
    ///
    /// Looks in the non-entity scope trees first, if we do not find anything move to entities
    ///
    /// # Arguments
    /// `pos` - Position we are trying to get a scope tree from
    ///
    /// # Returns
    /// Scope tree containing the position if any
    pub fn find_scope_tree_at(&self, pos: &Position) -> Option<&ScopeTree> {
        self.scope_trees
            .iter()
            .find(|scope_tree| position_in_range(*pos, scope_tree.range))
            .or_else(|| {
                self.entity_scope_trees
                    .values()
                    .find(|scope_tree| position_in_range(*pos, scope_tree.range))
            })
            .or_else(|| {
                self.package_scope_trees
                    .values()
                    .find(|scope_tree| position_in_range(*pos, scope_tree.range))
            })
    }

    /// Find the declaration of the given name that is visible at the given position
    ///
    /// Searches from the innermost scope outward so shadowing is properly taken into account
    ///
    /// # Arguments
    /// `name` - The name we are trying to find the declaration for
    /// `pos` - The position of the cursor
    ///
    /// # Returns
    /// An option with the declaration for the name is it is found
    pub fn find_declaration_at(&self, name: &str, pos: &Position) -> Option<&Declaration> {
        if let Some(scope_tree) = self.find_scope_tree_at(pos) {
            for inner_scope_tree in scope_tree.collect_scope_chain(pos).iter().rev() {
                if let Some(decl) = inner_scope_tree.get_declaration(name) {
                    return Some(decl);
                }
            }
            // At that point, we might need to check in the entity declaration. If the scope_tree links
            // to one, check if we can find the name in it.
            if let Some(entity_name) = &scope_tree.entity
                && let Some(entity_scope_tree) = self.entity_scope_trees.get(entity_name)
            {
                return entity_scope_tree.get_declaration(name);
            }
            // At that point, we might need to check in the package declaration. If the scope_tree links
            // to one, check if we can find the name in it.
            if let Some(package_name) = &scope_tree.package
                && let Some(package_scope_tree) = self.package_scope_trees.get(package_name)
            {
                return package_scope_tree.get_declaration(name);
            }
        }
        None
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
        if child.kind() == "identifier" || child.kind() == "attribute_identifier" {
            let id_text = text[child.byte_range()].to_string();
            if !id_text.is_empty() {
                references.insert(Usage {
                    name: id_text.clone(),
                    context,
                    range: node_to_range(child),
                });
            }
        } else {
            collect_identifiers_recursive(child, text, context, references);
        }
    }
}
