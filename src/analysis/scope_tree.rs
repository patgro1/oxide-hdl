//! Hierarchical scope tree for VHDL declarations.
//!
//! Provides recursive scope resolution and visibility tracking for
//! signals, variables, ports, and other VHDL identifiers.

use crate::analysis::types::{DeclType, Declaration, ScopeKind, Usage, UsageContext};
use crate::utils::{node_to_range, position_in_range};
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{Position, Range};
use tree_sitter::Node;

/// Hierarchical scope tree representing VHDL code structure.
///
/// Each node in the tree represents a scope (architecture, process, generate, or block)
/// and contains:
/// - Declarations made in that scope
/// - Identifiers used in that scope (not in child scopes)
/// - Child scopes (nested processes, generates, blocks)
///
/// The tree structure allows accurate tracking of:
/// - What's declared at each level
/// - What's used at each level
/// - Which parent declarations are accessible to child scopes
#[derive(Debug, Clone)]
pub struct ScopeTree {
    /// Kind of scope this node represents
    #[allow(dead_code)]
    pub kind: ScopeKind,

    /// Range where the Scope is
    pub range: Range,

    // Name of the current scope (label, entity name, arch name)
    pub name: Option<String>,

    // Entity attached to the current scope tree
    pub entity: Option<String>,

    /// Declarations made in this scope
    pub declarations: Vec<Declaration>,

    /// Identifiers referenced directly in this scope (not including children)
    pub local_usage: HashSet<Usage>,

    /// Child scopes nested within this scope
    pub children: Vec<ScopeTree>,

    /// Flat map of declarations with their index in the declaration vector.
    /// Using lower case because VHDL is case insensitive
    pub decl_index: HashMap<String, usize>,
}

impl ScopeTree {
    /// Creates a new empty scope tree node.
    ///
    /// # Arguments
    ///
    /// * `kind` - The type of scope this node represents
    pub fn new(kind: ScopeKind, node: &Node) -> Self {
        Self {
            kind,
            name: None,
            entity: None,
            range: node_to_range(*node),
            declarations: Vec::new(),
            local_usage: HashSet::new(),
            children: Vec::new(),
            decl_index: HashMap::new(),
        }
    }

    /// Recursively checks for unused declarations in this scope and all children.
    ///
    /// Declarations from parent scopes are passed down so children can check
    /// against them. A declaration is considered used if it appears in:
    /// - The local_usage set of this scope
    /// - The local_usage set of any descendant scope
    ///
    /// # Arguments
    ///
    /// * `parent_declarations` - Set of declaration names accessible from parent scopes
    ///
    /// # Returns
    ///
    /// Vector of unused declarations found in this scope tree
    pub fn check_unused(&self, parent_declarations: &HashSet<String>) -> Vec<Declaration> {
        let mut unused = Vec::new();

        // Build combined set of all accessible declarations
        let mut all_available: HashSet<String> = parent_declarations.clone();
        for decl in &self.declarations {
            all_available.insert(decl.name.clone().to_lowercase());
        }

        // Check local declarations for usage
        for decl in &self.declarations {
            if !self.is_used_anywhere(decl) {
                unused.push(decl.clone());
            }
        }

        // Recursively check child scopes
        for child in &self.children {
            unused.extend(child.check_unused(&all_available));
        }

        unused
    }

    /// Checks if an identifier is used anywhere in this scope tree.
    ///
    /// Recursively searches this scope and all descendant scopes for
    /// usage of the given identifier.
    ///
    /// # Arguments
    ///
    /// * `decl` - Declaration to search for (case-insensitive)
    ///
    /// # Returns
    ///
    /// `true` if the identifier is referenced anywhere in this scope tree
    pub fn is_used_anywhere(&self, decl: &Declaration) -> bool {
        let decl_name_lower = decl.name.to_lowercase();
        let used_locally = match decl.decl_type {
            DeclType::Constant | DeclType::Generic => {
                self.local_usage.iter().any(|u| u.name == decl_name_lower)
            }
            DeclType::Port(_) | DeclType::Signal | DeclType::Variable => self
                .local_usage
                .iter()
                .any(|u| u.name == decl_name_lower && u.context == UsageContext::Behavioral),
        };
        if used_locally {
            return true;
        }
        for child in &self.children {
            if child.is_used_anywhere(decl) {
                return true;
            }
        }
        false
    }

    /// Build the declaration index mapping a name with an index in the declaration vector
    pub fn rebuild_index(&mut self) {
        self.decl_index = self
            .declarations
            .iter()
            .enumerate()
            .map(|(idx, decl)| (decl.name.clone().to_lowercase(), idx))
            .collect();
    }

    /// Recursively collect all visible declarations from the current range
    ///
    /// # Arguments
    /// `target` - Range we want to get visible declarations from
    /// `entity` - Entity attached to the current scope tree
    ///
    /// # Returns
    /// Vector of all visible declarations for the range
    pub fn collect_visible_declarations(
        &self,
        target: &Range,
        entity: Option<&ScopeTree>,
    ) -> Option<Vec<Declaration>> {
        // Breaking recursion if we are the target
        if self.range == *target {
            return Some(self.declarations.clone());
        }
        for child in &self.children {
            if let Some(mut child_decl) = child.collect_visible_declarations(target, None) {
                child_decl.extend(self.declarations.clone());
                if let Some(entity) = entity {
                    child_decl.extend(entity.declarations.clone());
                }
                return Some(child_decl);
            }
        }
        None
    }

    /// Simple declaration lookup in the scope tree
    ///
    /// # Arguments
    /// `name`: Name of the declaration we are trying to find
    ///
    /// # Return
    /// Option on the found declaration index
    pub fn get_declaration(&self, name: &str) -> Option<&Declaration> {
        let decl_idx = self.decl_index.get(&name.to_lowercase())?;
        self.declarations.get(*decl_idx)
    }

    /// Recursively find the innermost scope for a given position
    ///
    /// Assumes that the position given was already checked to be in range of the scopetree
    ///
    /// # Arguments
    /// `pos` - Position we want to look for
    ///
    /// # Returns
    /// The smallest scope-tree including the position
    pub fn find_innermost_scope(&self, pos: &Position) -> &ScopeTree {
        debug_assert!(
            self.scope_tree_contains_pos(pos),
            "When calling this method, we assume that the current scope tree is in range"
        );

        for child in &self.children {
            if child.scope_tree_contains_pos(pos) {
                return child.find_innermost_scope(pos);
            }
        }

        self
    }

    /// Helper the check if a given position is contained by the scope tree
    ///
    /// # Arguments
    /// `pos` - Position we are comparing
    ///
    /// # Returns
    /// True if the position is in the scope tree range
    fn scope_tree_contains_pos(&self, pos: &Position) -> bool {
        position_in_range(*pos, self.range)
    }

    /// Recursively scan to find the innermost scope and returns the list of scope containing the
    /// position
    ///
    /// Assumes that the position given was already checked to be in range of the scopetree
    ///
    /// # Arguments
    /// `pos` - Position we are trying to find scopes for
    ///
    /// # Returns
    /// A vector containing references of all the ScopeTree containing the positon
    pub fn collect_scope_chain(&self, pos: &Position) -> Vec<&ScopeTree> {
        debug_assert!(
            self.scope_tree_contains_pos(pos),
            "When calling this method, we assume that the current scope tree is in range"
        );
        let mut scope_chain = Vec::new();
        scope_chain.push(self);
        for child in &self.children {
            if child.scope_tree_contains_pos(pos) {
                scope_chain.extend(child.collect_scope_chain(pos));
                break;
            }
        }

        scope_chain
    }
}
