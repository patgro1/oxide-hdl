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

    /// All design units in file order, each paired with its own context clause.
    /// This is the authoritative per-unit view; `use_clauses` below is kept as
    /// a flat union for JIT package loading only.
    pub design_units: Vec<DesignUnit>,

    /// Flat union of all context clauses in the file — used only for JIT package
    /// loading in workspace.rs. Do NOT use this for symbol resolution; use
    /// `design_units[i].context_clauses` for the correct per-unit scope.
    pub use_clauses: Vec<UseClause>,

    /// How the file was parsed
    pub parse_level: ParseLevel,

    /// VHDL library this file's design units belong to, lowercase.
    ///
    /// Resolved from the `[libraries]` globs in `oxide.toml` at index time.
    /// Defaults to `work`, which is also what every file gets when no libraries
    /// are configured — making library-aware resolution a no-op for such workspaces.
    pub library: String,

    /// Scope tree with entity declaration (Headers)
    pub entity_scope_trees: HashMap<String, ScopeTree>,

    /// Scope tree with signals, constants, types declaration and usage (Architectures)
    pub scope_trees: Vec<ScopeTree>,

    /// Scope tree for package declarations (Headers)
    pub package_declaration_scope_trees: HashMap<String, ScopeTree>,

    /// Scope tree for package bodies (Implementations)
    pub package_body_scope_trees: HashMap<String, ScopeTree>,
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
            library: "work".to_string(),
            design_units: Vec::new(),
            entity_scope_trees: HashMap::new(),
            scope_trees: Vec::new(),
            package_declaration_scope_trees: HashMap::new(),
            package_body_scope_trees: HashMap::new(),
            use_clauses: vec![implicit_standard],
        }
    }

    /// Returns `true` when no design-unit scope trees were extracted at all.
    ///
    /// This is the signature of a **transiently unparseable buffer**, not of an
    /// empty file. Tree-sitter cannot produce an `architecture_definition` while
    /// any construct inside it is still unclosed, so a single `if` awaiting its
    /// `end if;` collapses every scope tree in the file. The same happens for an
    /// unclosed `process`, `generate` or `block`, or a file not yet terminated
    /// with `end architecture;`.
    ///
    /// Callers use this to avoid replacing a good analysis with a useless one
    /// while the user is mid-keystroke. Note that a file holding only an entity
    /// or only a package still has content — those populate `entity_scope_trees`
    /// and `package_*_scope_trees` — so all four collections are checked.
    pub fn has_no_scope_trees(&self) -> bool {
        self.scope_trees.is_empty()
            && self.entity_scope_trees.is_empty()
            && self.package_declaration_scope_trees.is_empty()
            && self.package_body_scope_trees.is_empty()
    }

    /// Returns the context clauses that apply to the design unit containing `scope_tree`.
    ///
    /// Searches `design_units` for an entry whose scope tree covers the same range.
    /// Falls back to an empty slice when not found (inner scopes like process/generate
    /// are not top-level design units and carry no context clause of their own).
    pub fn context_clauses_for(&self, scope_tree: &ScopeTree) -> &[UseClause] {
        self.design_units
            .iter()
            .find(|du| du.scope_tree.range == scope_tree.range)
            .map(|du| du.context_clauses.as_slice())
            .unwrap_or(&[])
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
                self.package_declaration_scope_trees
                    .values()
                    .find(|scope_tree| position_in_range(*pos, scope_tree.range))
            })
            .or_else(|| {
                self.package_body_scope_trees
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
            if let Some(package_name) = &scope_tree.package {
                if let Some(header) = self.package_declaration_scope_trees.get(package_name)
                    && let Some(decl) = header.get_declaration(name)
                {
                    return Some(decl);
                }
                if let Some(body) = self.package_body_scope_trees.get(package_name)
                    && let Some(decl) = body.get_declaration(name)
                {
                    return Some(decl);
                }
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

/// Collects identifier references from port/generic map aspects.
///
/// In port maps like `port map (clk => my_clk, data => my_data)`:
/// - `clk` and `data` are formal port names (not local usages)
/// - `my_clk` and `my_data` are actual signals (local usages)
///
/// This function only collects the actual parts (right-hand side of =>).
///
/// # Arguments
///
/// * `map_aspect_node` - Tree-sitter node of type `port_map_aspect` or `generic_map_aspect`
/// * `text` - Full source text
/// * `context` - Usage context for collected identifiers
/// * `references` - Mutable set to collect identifier names into
pub fn collect_identifiers_from_map_aspect(
    map_aspect_node: Node,
    text: &str,
    context: UsageContext,
    references: &mut HashSet<Usage>,
) {
    let mut cursor = map_aspect_node.walk();
    for child in map_aspect_node.children(&mut cursor) {
        if child.kind() == "association_element" {
            // association_element has: formal => actual
            // We only want to collect from the actual part
            let mut assoc_cursor = child.walk();
            let mut found_arrow = false;
            for part in child.children(&mut assoc_cursor) {
                if part.kind() == "=>" {
                    found_arrow = true;
                } else if found_arrow {
                    // After the arrow, collect identifiers from actual
                    collect_identifiers_recursive(part, text, context, references);
                    break;
                }
            }
        } else {
            // Recurse into other nodes (like association_list)
            collect_identifiers_from_map_aspect(child, text, context, references);
        }
    }
}
