use tower_lsp::lsp_types::{Position, Range, Url};

use crate::{
    analysis::{Analysis, Declaration, OxideSymbolKind, ScopeTree, Symbol},
    backend::AnalysisMap,
};

/// Unified result that can hold either a Rich Declaration or a Generic Symbol
#[derive(Debug, Clone)]
pub enum ResolvedItem {
    /// A precise declaration (Signal, Port, Var)
    Declaration(Declaration),
    /// A generic sybol (entity, package or regex match)
    Symbol(Symbol),
}

impl ResolvedItem {
    pub fn range(&self) -> Range {
        match self {
            ResolvedItem::Declaration(d) => d.range,
            ResolvedItem::Symbol(s) => s.range,
        }
    }

    pub fn selection_range(&self) -> Range {
        match self {
            ResolvedItem::Declaration(d) => d.selection_range,
            ResolvedItem::Symbol(s) => s.range, // Symbol do not store a selection range
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ResolvedItem::Declaration(d) => &d.name,
            ResolvedItem::Symbol(s) => &s.name,
        }
    }
}

/// Result of a lookup
#[derive(Debug)]
pub struct LookupResult {
    pub item: ResolvedItem,
    pub source_uri: Url,
}

pub fn lookup_symbol(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: &Position,
) -> Vec<LookupResult> {
    let mut results = Vec::new();
    let target_lc = target.to_lowercase();
    if let Some(analysis) = analysis_map.get(current_uri) {
        // 1. Local lookup
        if let Some(scope) = analysis.find_scope_tree_at(pos) {
            let entity_scope = analysis.entity_scope_trees.values().next();
            if let Some(locals) = scope.collect_visible_declarations(&scope.range, entity_scope) {
                for decl in locals {
                    if decl.name.to_lowercase() == target_lc {
                        results.push(LookupResult {
                            item: ResolvedItem::Declaration(decl),
                            source_uri: current_uri.clone(),
                        });
                    }
                }
            }
        }
        // If we find anything, cut short the search and return
        if !results.is_empty() {
            return results;
        }

        // 2. Imports
        resolve_imports_for_symbol(analysis, &target_lc, analysis_map, &mut results);
    }

    if results.is_empty() {
        resolve_global_toplevel_symbols(&target_lc, analysis_map, &mut results);
    }

    results
}

/// Resolves 'use' clauses by looking up packages in the global map
fn resolve_imports_for_symbol(
    analysis: &Analysis,
    target: &str,
    map: &AnalysisMap,
    results: &mut Vec<LookupResult>,
) {
    for clause in &analysis.use_clauses {
        let pkg_name = &clause.name;
        for (uri, global_analysis) in map.iter() {
            if let Some(pkg_scope) = global_analysis
                .package_scope_trees
                .get(&pkg_name.to_lowercase())
            {
                let check_scope_decls = |scope: &ScopeTree, output: &mut Vec<LookupResult>| {
                    for decl in &scope.declarations {
                        if decl.name.eq_ignore_ascii_case(target) {
                            output.push(LookupResult {
                                item: ResolvedItem::Declaration(decl.clone()),
                                source_uri: uri.clone(),
                            });
                        }
                    }
                };
                if clause.all_import {
                    check_scope_decls(pkg_scope, results);
                } else if let Some(imported_sym) = &clause.imported_symbol
                    && imported_sym.eq_ignore_ascii_case(target)
                {
                    check_scope_decls(pkg_scope, results);
                }
            } else if let Some(pkg_sym) = global_analysis.symbols.get(&pkg_name.to_lowercase())
                && pkg_sym.kind == OxideSymbolKind::Package
            {
                let find_and_push = |t: &str, output: &mut Vec<LookupResult>| {
                    if let Some(child) = pkg_sym
                        .children
                        .iter()
                        .find(|c| c.name.eq_ignore_ascii_case(t))
                    {
                        output.push(LookupResult {
                            item: ResolvedItem::Symbol(child.clone()),
                            source_uri: uri.clone(),
                        });
                    }
                };
                if clause.all_import {
                    find_and_push(target, results);
                } else if let Some(imported_sym) = &clause.imported_symbol
                    && imported_sym.eq_ignore_ascii_case(target)
                {
                    find_and_push(target, results);
                }
            }
        }
    }
}

/// Fallback to global top-level entities of packages definition
fn resolve_global_toplevel_symbols(
    target: &str,
    map: &AnalysisMap,
    results: &mut Vec<LookupResult>,
) {
    for (uri, analysis) in map.iter() {
        if let Some(sym) = analysis.symbols.get(target)
            && matches!(sym.kind, OxideSymbolKind::Entity | OxideSymbolKind::Package)
        {
            results.push(LookupResult {
                item: ResolvedItem::Symbol(sym.clone()),
                source_uri: uri.clone(),
            });
        }

        if let Some(scope) = analysis.entity_scope_trees.get(target) {
            results.push(LookupResult {
                item: ResolvedItem::Symbol(Symbol {
                    name: scope.name.clone().unwrap_or_default(),
                    kind: OxideSymbolKind::Entity,
                    detail: None,
                    range: scope.range,
                    children: vec![],
                }),
                source_uri: uri.clone(),
            });
        }

        if let Some(scope) = analysis.package_scope_trees.get(target) {
            results.push(LookupResult {
                item: ResolvedItem::Symbol(Symbol {
                    name: scope.name.clone().unwrap_or_default(),
                    kind: OxideSymbolKind::Package,
                    detail: None,
                    range: scope.range,
                    children: vec![],
                }),
                source_uri: uri.clone(),
            });
        }
    }
}
