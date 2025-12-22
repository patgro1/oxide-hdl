//! Hierarchical scope tree for VHDL declarations.
//!
//! Provides recursive scope resolution and visibility tracking for
//! signals, variables, ports, and other VHDL identifiers.

use crate::analysis::types::{DeclType, Declaration, ScopeKind, Usage, UsageContext};
use crate::backend::utils::node_to_range;
use std::collections::HashSet;
use tower_lsp::lsp_types::Range;
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
}
