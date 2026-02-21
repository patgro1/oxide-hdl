use crate::analysis::{DeclType, OxideSymbolKind, ScopeKind};
use crate::backend::features::lookup::{ResolvedItem, lookup_symbol, LookupResult};
use crate::backend::{AnalysisMap, Location};
use tower_lsp::lsp_types::{Position, Url};

/// Resolves the definition location(s) for a given symbol.
///
/// This function implements the "Go to Definition" logic, prioritizing the 
/// implementation (body) over the interface (declaration).
///
/// # Logic
/// 1.  **Prioritization:** 
///     *   Subprograms: Favors implementation bodies (`DeclType::Function`) over signatures.
///     *   Entities: Favors the Entity declaration over a Component declaration.
/// 2.  **Fallback:** If the preferred implementation isn't found, it returns available declarations.
pub fn lookup_definition(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: Position,
) -> Vec<Location> {
    let results = lookup_symbol(target, current_uri, analysis_map, &pos, true);

    let mut entities = Vec::new();
    let mut components = Vec::new();
    let mut subprogram_bodies = Vec::new();
    let mut others = Vec::new();

    for res in &results {
        match &res.item {
            ResolvedItem::Symbol(s) => match s.kind {
                OxideSymbolKind::Entity => entities.push(res),
                OxideSymbolKind::Component => components.push(res),
                OxideSymbolKind::PackageBody | OxideSymbolKind::Architecture => subprogram_bodies.push(res),
                _ => others.push(res),
            },
            ResolvedItem::Declaration(d) => match d.decl_type {
                DeclType::Function | DeclType::Procedure => {
                    subprogram_bodies.push(res);
                }
                DeclType::Component => components.push(res),
                DeclType::FunctionDeclaration | DeclType::ProcedureDeclaration => {
                    others.push(res);
                }
                _ => others.push(res),
            },
        }
    }

    let mut final_results = if !subprogram_bodies.is_empty() {
        subprogram_bodies
    } else if !entities.is_empty() {
        entities
    } else if !components.is_empty() {
        components
    } else {
        others
    };

    // If we have multiple results, try to prioritize the one in the current file
    if final_results.len() > 1 && final_results.iter().any(|res| res.source_uri == *current_uri) {
        final_results.retain(|res| res.source_uri == *current_uri);
    }

    final_results
        .into_iter()
        .map(|res| Location {
            uri: res.source_uri.clone(),
            range: res.item.selection_range(),
        })
        .collect()
}

/// Resolves the declaration location(s) for a given symbol.
///
/// This function implements the "Go to Declaration" logic, prioritizing the 
/// interface/specification over the implementation body.
///
/// # Logic
/// 1.  **Prioritization:** 
///     *   Subprograms: Favors signatures (`DeclType::FunctionDeclaration`) over bodies.
///     *   Components: Favors Component declarations over Entity declarations.
/// 2.  **Fallback:** If the preferred declaration isn't found (e.g. no component defined), 
///     it falls back to the Entity declaration.
pub fn lookup_declaration(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: Position,
) -> Vec<Location> {
    let results = lookup_symbol(target, current_uri, analysis_map, &pos, true);

    let mut components = Vec::new();
    let mut subprogram_specs = Vec::new();
    let mut entities = Vec::new();
    let mut others = Vec::new();

    for res in &results {
        match &res.item {
            ResolvedItem::Symbol(s) => match s.kind {
                OxideSymbolKind::Component => components.push(res),
                OxideSymbolKind::Package => subprogram_specs.push(res),
                OxideSymbolKind::Entity => entities.push(res),
                _ => others.push(res),
            },
            ResolvedItem::Declaration(d) => match d.decl_type {
                DeclType::Component => components.push(res),
                DeclType::FunctionDeclaration | DeclType::ProcedureDeclaration => {
                    subprogram_specs.push(res);
                }
                DeclType::Function | DeclType::Procedure => {
                    others.push(res);
                }
                _ => others.push(res),
            },
        }
    }

    let mut final_results = if !components.is_empty() {
        components
    } else if !subprogram_specs.is_empty() {
        subprogram_specs
    } else if !entities.is_empty() {
        entities
    } else {
        others
    };

    if final_results.len() > 1 && final_results.iter().any(|res| res.source_uri == *current_uri) {
        final_results.retain(|res| res.source_uri == *current_uri);
    }

    final_results
        .into_iter()
        .map(|res| Location {
            uri: res.source_uri.clone(),
            range: res.item.selection_range(),
        })
        .collect()
}

/// Resolves the implementation location(s) for a given symbol.
///
/// This function implements the "Go to Implementation" logic, specifically 
/// targeting Architecture bodies and subprogram bodies.
///
/// # Logic
/// 1.  **Direct Match:** Searches for `Architecture`, `PackageBody`, or subprogram implementations (`DeclType::Function`).
/// 2.  **Entity-to-Architecture:** If the target is an Entity name, it performs a 
///     global search to find all Architectures associated with that Entity.
pub fn lookup_implementation(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: Position,
) -> Vec<Location> {
    let results = lookup_symbol(target, current_uri, analysis_map, &pos, true);

    let mut implementations = Vec::new();

    for res in &results {
        let is_impl = match &res.item {
            ResolvedItem::Symbol(s) => matches!(
                s.kind,
                OxideSymbolKind::Architecture | OxideSymbolKind::PackageBody
            ),
            ResolvedItem::Declaration(d) => {
                matches!(d.decl_type, DeclType::Function | DeclType::Procedure)
            }
        };

        if is_impl {
            implementations.push(LookupResult {
                item: res.item.clone(),
                source_uri: res.source_uri.clone(),
            });
        }
    }

    if implementations.is_empty() {
        let entities: Vec<_> = results
            .iter()
            .filter(|res| match &res.item {
                ResolvedItem::Symbol(s) => s.kind == OxideSymbolKind::Entity,
                _ => false,
            })
            .collect();

        for ent_res in entities {
            let ent_name = match &ent_res.item {
                ResolvedItem::Symbol(s) => &s.name,
                _ => unreachable!(),
            };

            for (uri, analysis) in analysis_map.iter() {
                // A. Check Deep-Parsed Architectures
                for scope in &analysis.scope_trees {
                    if scope.kind == ScopeKind::Architecture
                        && scope.entity.as_deref().unwrap_or_default().eq_ignore_ascii_case(ent_name)
                    {
                        implementations.push(LookupResult {
                            item: ResolvedItem::Symbol(crate::analysis::Symbol {
                                name: scope.name.clone().unwrap_or_default(),
                                kind: OxideSymbolKind::Architecture,
                                detail: None,
                                range: scope.range,
                                children: vec![],
                            }),
                            source_uri: uri.clone(),
                        });
                    }
                }

                // B. Check Shallow-Indexed Architectures (Fallback)
                if implementations.iter().all(|res| &res.source_uri != uri) {
                    let target_detail = format!("Architecture of {}", ent_name).to_lowercase();
                    for sym in analysis.symbols.values() {
                        if sym.kind == OxideSymbolKind::Architecture
                            && let Some(detail) = &sym.detail {
                                // Detail is "Architecture of <entity_name>"
                                if detail.to_lowercase() == target_detail {
                                    implementations.push(LookupResult {
                                        item: ResolvedItem::Symbol(sym.clone()),
                                        source_uri: uri.clone(),
                                    });
                                }
                            }
                    }
                }
            }
        }
    }

    implementations
        .into_iter()
        .map(|res| Location {
            uri: res.source_uri,
            range: res.item.selection_range(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
