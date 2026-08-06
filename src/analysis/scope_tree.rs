//! Hierarchical scope tree for VHDL declarations.
//!
//! Provides recursive scope resolution and visibility tracking for
//! signals, variables, ports, and other VHDL identifiers.

use crate::analysis::Instance;
use crate::analysis::types::{DeclType, Declaration, ScopeKind, Usage, UsageContext, UseClause};
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
    pub kind: ScopeKind,

    /// Range where the Scope is
    pub range: Range,

    /// Name of the current scope (label, entity name, arch name)
    pub name: Option<String>,

    /// Entity attached to the current scope tree
    pub entity: Option<String>,

    /// Package head attached to the current scope tree
    pub package: Option<String>,

    /// Declarations made in this scope
    pub declarations: Vec<Declaration>,

    /// Identifiers referenced directly in this scope (not including children)
    pub local_usage: HashSet<Usage>,

    /// Child scopes nested within this scope
    pub children: Vec<ScopeTree>,

    /// Using lower case because VHDL is case insensitive
    pub decl_index: HashMap<String, usize>,

    /// List of the instantiation in the current scope
    pub instantiations: Vec<Instance>,

    /// Use clauses declared directly in this scope (for generate/block declarative regions)
    pub use_clauses: Vec<UseClause>,

    /// Maps attribute name (lowercase) → set of entity names (lowercase) the attribute
    /// has been applied to in this scope. `"*"` means `all` or `others`.
    pub attr_specs: HashMap<String, HashSet<String>>,
}

/// Enum specifying the type of region we are in.
///
/// Two kinds of regions exist: SEQUENTIAL and CONCURRENT.
/// Architectures, generates, and blocks are concurrent regions because they implement concurrent logic.
/// Processes are sequential regions.
///
/// This distinction is used to determine which kinds of declarations can be found in a specific
/// region.
pub enum RegionType {
    /// Architecture, Generate, Blocks
    Concurrent,
    /// Process, Function, Procedure
    Sequential,
    /// Package body deserves a special case
    Implementation,
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
            package: None,
            range: node_to_range(*node),
            declarations: Vec::new(),
            local_usage: HashSet::new(),
            children: Vec::new(),
            decl_index: HashMap::new(),
            instantiations: Vec::new(),
            use_clauses: Vec::new(),
            attr_specs: HashMap::new(),
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
            DeclType::Constant | DeclType::Generic | DeclType::Alias | DeclType::Attribute => self
                .local_usage
                .iter()
                .any(|u| u.name.to_lowercase() == decl_name_lower),
            DeclType::Parameter(_, _)
            | DeclType::Port(_)
            | DeclType::Signal
            | DeclType::RecordField
            | DeclType::EnumLiteral
            | DeclType::Variable => self.local_usage.iter().any(|u| {
                u.name.to_lowercase() == decl_name_lower && u.context == UsageContext::Behavioral
            }),
            DeclType::Subtype
            | DeclType::Function
            | DeclType::FunctionDeclaration
            | DeclType::Type
            | DeclType::Procedure
            | DeclType::ProcedureDeclaration
            | DeclType::Component => true,
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

    /// Recursively collect all visible declarations from the current range.
    ///
    /// # Arguments
    ///
    /// * `target` - Range we want to get visible declarations from
    /// * `header` - Header scope tree (entity or package) attached to the current scope
    ///
    /// # Returns
    ///
    /// Option containing a vector of all visible declarations for the range
    pub fn collect_visible_declarations(
        &self,
        target: &Range,
        header: Option<&ScopeTree>,
    ) -> Option<Vec<Declaration>> {
        // Breaking recursion if we are the target
        let mut decls = self.collect_visible_internal(target)?;
        if let Some(header) = header {
            decls.extend(header.declarations.clone());
        }
        Some(decls)
    }

    /// Internal recursive helper for collecting visible declarations.
    ///
    /// Returns declarations from this scope if it matches the target range,
    /// or recurses into children to find the target scope and accumulates
    /// parent declarations on the way back up.
    fn collect_visible_internal(&self, target: &Range) -> Option<Vec<Declaration>> {
        if self.range == *target {
            return Some(self.declarations.clone());
        }
        for child in &self.children {
            if let Some(mut child_decl) = child.collect_visible_internal(target) {
                child_decl.extend(self.declarations.clone());
                return Some(child_decl);
            }
        }
        None
    }

    /// Simple declaration lookup in the scope tree.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the declaration we are trying to find
    ///
    /// # Returns
    ///
    /// Option containing a reference to the found declaration
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

    /// Helper to check if a given position is contained by the scope tree
    ///
    /// # Arguments
    /// `pos` - Position we are comparing
    ///
    /// # Returns
    /// True if the position is in the scope tree range
    fn scope_tree_contains_pos(&self, pos: &Position) -> bool {
        position_in_range(*pos, self.range)
    }

    /// Recursively collects all use_clauses from this scope and all descendants.
    ///
    /// Used for JIT-parsing packages referenced inside generate/block scopes without
    /// polluting the top-level `analysis.use_clauses`.
    pub fn collect_all_use_clauses(&self) -> Vec<UseClause> {
        let mut clauses = self.use_clauses.clone();
        for child in &self.children {
            clauses.extend(child.collect_all_use_clauses());
        }
        clauses
    }

    /// Collects every instantiation in this scope and all nested child scopes.
    ///
    /// Instantiations inside `generate` and `block` statements live in child scope
    /// trees, so a flat read of `self.instantiations` misses them. Used to decide
    /// which entity files to JIT-parse when a document is opened.
    pub fn collect_all_instantiations(&self) -> Vec<&Instance> {
        let mut out: Vec<&Instance> = self.instantiations.iter().collect();
        for child in &self.children {
            out.extend(child.collect_all_instantiations());
        }
        out
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
    ///
    /// A vector containing references of all the ScopeTree containing the position
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

    /// Returns true if `attr_name` has been applied to `entity_name` in this scope.
    /// Respects the `"*"` sentinel which means `all` or `others`.
    pub fn is_attr_applied(&self, attr_name: &str, entity_name: &str) -> bool {
        match self.attr_specs.get(&attr_name.to_lowercase()) {
            None => false,
            Some(names) => {
                names.contains("*") || names.contains(&entity_name.to_lowercase())
            }
        }
    }
}

/// Finds the instantiation whose unit-name token contains `pos`, searching
/// every scope tree in `scope_trees` and their nested generate/block children
/// (via `collect_all_instantiations`).
///
/// Used to detect when the cursor sits on the entity/component name inside
/// `label: entity lib.name`, so hover and goto-definition can resolve it as
/// an instantiation. The check must run before *both* of the generic
/// resolution paths, since neither understands instantiation syntax: the
/// dotted-name chain resolver (`get_qualified_chain_at_pos` /
/// `resolve_path_chain`) and the bare-word fallback
/// (`get_identifier_from_ast` → `lookup_symbol`). At this cursor position the
/// chain resolver simply comes up empty, so it is the bare-word fallback that
/// actually mishandles it — it finds the entity's shallow, childless `Symbol`
/// and degrades to `format_basic`'s `entity : void`.
pub fn find_instance_at(scope_trees: &[ScopeTree], pos: Position) -> Option<&Instance> {
    for tree in scope_trees {
        for inst in tree.collect_all_instantiations() {
            if let Some(range) = inst.unit_range
                && position_in_range(pos, range)
            {
                return Some(inst);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_scope(attr_name: &str, entity_names: &[&str]) -> ScopeTree {
        let mut attr_specs = HashMap::new();
        let names: HashSet<String> = entity_names.iter().map(|s| s.to_string()).collect();
        attr_specs.insert(attr_name.to_lowercase(), names);
        ScopeTree {
            kind: ScopeKind::Architecture,
            range: Range::default(),
            name: None,
            entity: None,
            package: None,
            declarations: vec![],
            local_usage: HashSet::new(),
            children: vec![],
            decl_index: HashMap::new(),
            instantiations: vec![],
            use_clauses: vec![],
            attr_specs,
        }
    }

    #[test]
    fn is_attr_applied_basic_match() {
        let scope = make_test_scope("mark_debug", &["my_sig"]);
        assert!(scope.is_attr_applied("mark_debug", "my_sig"));
    }

    #[test]
    fn is_attr_applied_no_match() {
        let scope = make_test_scope("mark_debug", &["my_sig"]);
        assert!(!scope.is_attr_applied("mark_debug", "other_sig"));
    }

    #[test]
    fn is_attr_applied_wildcard() {
        let scope = make_test_scope("mark_debug", &["*"]);
        assert!(scope.is_attr_applied("mark_debug", "anything_at_all"));
    }

    #[test]
    fn is_attr_applied_case_insensitive() {
        let scope = make_test_scope("mark_debug", &["my_sig"]);
        assert!(scope.is_attr_applied("MARK_DEBUG", "MY_SIG"));
    }

    #[test]
    fn is_attr_applied_no_attr() {
        let scope = make_test_scope("other_attr", &["my_sig"]);
        assert!(!scope.is_attr_applied("mark_debug", "my_sig"));
    }

    fn make_instance(component: &str, unit_range: Option<Range>) -> Instance {
        Instance {
            label: "u0".to_string(),
            component: component.to_string(),
            library: Some("work".to_string()),
            architecture: None,
            unit_kind: crate::analysis::InstantiatedUnitKind::Entity,
            range: Range::default(),
            selection_range: Range::default(),
            unit_range,
        }
    }

    fn range_at(line: u32, start: u32, end: u32) -> Range {
        Range {
            start: Position {
                line,
                character: start,
            },
            end: Position {
                line,
                character: end,
            },
        }
    }

    #[test]
    fn find_instance_at_hits_when_pos_inside_unit_range() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope
            .instantiations
            .push(make_instance("uart_rx", Some(range_at(3, 24, 31))));

        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 3,
                character: 27,
            },
        );
        assert_eq!(found.map(|i| i.component.as_str()), Some("uart_rx"));
    }

    #[test]
    fn find_instance_at_misses_just_outside_unit_range() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope
            .instantiations
            .push(make_instance("uart_rx", Some(range_at(3, 24, 31))));

        // position_in_range treats the range's end as inclusive, so the true
        // boundary is one character past 31 — use 32 to land unambiguously
        // outside on either interpretation.
        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 3,
                character: 32,
            },
        );
        assert!(found.is_none());
    }

    #[test]
    fn find_instance_at_ignores_instantiations_with_no_unit_range() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope.instantiations.push(make_instance("uart_rx", None));

        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(found.is_none());
    }

    #[test]
    fn find_instance_at_picks_the_right_one_among_several() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope
            .instantiations
            .push(make_instance("uart_rx", Some(range_at(3, 24, 31))));
        scope
            .instantiations
            .push(make_instance("cpu", Some(range_at(5, 10, 13))));

        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 5,
                character: 11,
            },
        );
        assert_eq!(found.map(|i| i.component.as_str()), Some("cpu"));
    }
}
