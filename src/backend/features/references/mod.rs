//! Implementation for the textDocument/references feature.

use crate::analysis::{Analysis, ScopeKind, ScopeTree, Usage};
use tower_lsp::lsp_types::{Location, ReferenceParams, Url};

#[cfg(test)]
mod tests;

/// Finds all local references for the symbol under the cursor.
///
/// This currently only searches within the document provided in the `Analysis`
/// structure, as a cross-file reference search is not yet supported.
///
/// # Arguments
/// * `params` - The LSP ReferenceParams, containing the document and position.
/// * `analysis` - The resolved document analysis containing scope trees.
/// * `uri` - The URI of the document being searched.
/// * `word` - The word at the cursor position.
///
/// # Returns
/// A list of LSP `Location` objects corresponding to usages of the resolved symbol.
pub fn find_references(
    params: &ReferenceParams,
    analysis: &Analysis,
    uri: &Url,
    word: &str,
) -> Vec<Location> {
    let mut locations = Vec::new();
    let position = params.text_document_position.position;
    let decl = analysis.find_declaration_at(word, &position);
    if let Some(decl) = decl {
        // References should be find from the scope tree of the declaration and below
        if let Some(scope_tree) = analysis.find_scope_tree_at(&decl.range.start) {
            let scope_tree = scope_tree.find_innermost_scope(&decl.range.start);
            let base_scopes: Vec<&ScopeTree> = if scope_tree.kind == ScopeKind::Entity {
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
            // First we push the declaration
            locations.push(Location {
                uri: uri.clone(),
                range: decl.range,
            });
            let references = base_scopes
                .iter()
                .flat_map(|st| collect_all_usage(word, st));
            for single_ref in references {
                locations.push(Location {
                    uri: uri.clone(),
                    range: single_ref.range,
                });
            }
        }
    }

    locations
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
