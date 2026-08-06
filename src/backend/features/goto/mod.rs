use crate::analysis::{DeclType, OxideSymbolKind, ScopeKind};
use crate::backend::features::lookup::{LookupResult, ResolvedItem, lookup_symbol};
use crate::backend::{AnalysisMap, Location};
use tower_lsp::lsp_types::{Position, Url};

fn to_locations(results: impl IntoIterator<Item = LookupResult>) -> Vec<Location> {
    results
        .into_iter()
        .map(|res| Location {
            uri: res.source_uri,
            range: res.item.selection_range(),
        })
        .collect()
}

fn prefer_current_file(results: &mut Vec<&LookupResult>, current_uri: &Url) {
    if results.len() > 1 && results.iter().any(|r| r.source_uri == *current_uri) {
        results.retain(|r| r.source_uri == *current_uri);
    }
}

/// Resolves goto-definition when the cursor sits on the unit-name of a
/// direct entity instantiation (`label: entity lib.name`), bypassing the
/// generic dotted-name chain resolver, which has no concept of instantiation
/// syntax.
///
/// No JIT parse is needed either way: a shallow file's `Analysis.symbols`
/// entry for the entity already points at its name token (from the fast
/// regex scan), and a deep file's `entity_scope_trees` entry points at its
/// full declaration — `file_declares_entity`'s "check both" pattern, reused
/// here for the location itself rather than just existence.
///
/// Returns `None` when the cursor isn't on such a position, or the
/// instantiation's library/name doesn't resolve to a known file, so the
/// caller falls through to the existing chain/bare-word resolution.
pub fn resolve_instantiated_entity_location(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    pos: Position,
) -> Option<Location> {
    let analysis = analysis_map.get(current_uri)?;
    let inst = crate::analysis::find_instance_at(&analysis.scope_trees, pos)?;
    if inst.unit_kind != crate::analysis::InstantiatedUnitKind::Entity {
        return None;
    }
    let target_uri =
        crate::backend::units::resolve_entity_uris(analysis_map, inst, &analysis.library)
            .into_iter()
            .next()?;
    let name_lc = inst.component.to_lowercase();
    let target_analysis = analysis_map.get(&target_uri)?;
    let range = target_analysis
        .symbols
        .get(&name_lc)
        .map(|s| s.range)
        .or_else(|| {
            target_analysis
                .entity_scope_trees
                .get(&name_lc)
                .map(|t| t.range)
        })?;
    Some(Location {
        uri: target_uri,
        range,
    })
}

/// Applies goto-definition priority ordering to a pre-resolved result set.
///
/// Identical prioritization to [`lookup_definition`] but operates on results
/// already resolved by the caller (e.g. via `resolve_path_chain` for qualified
/// names like `pkg.member` or `work.pkg.member`).
///
/// # Logic
/// 1.  **Prioritization:**
///     *   Subprograms: Favors implementation bodies (`DeclType::Function`) over signatures.
///     *   Entities: Favors the Entity declaration over a Component declaration.
/// 2.  **Fallback:** If the preferred kind isn't found, returns remaining results.
pub fn apply_definition_priority(results: Vec<LookupResult>, current_uri: &Url) -> Vec<Location> {
    let mut entities = Vec::new();
    let mut components = Vec::new();
    let mut subprogram_bodies = Vec::new();
    let mut others = Vec::new();

    for res in &results {
        match &res.item {
            ResolvedItem::Symbol(s) => match s.kind {
                OxideSymbolKind::Entity => entities.push(res),
                OxideSymbolKind::Component => components.push(res),
                OxideSymbolKind::PackageBody | OxideSymbolKind::Architecture => {
                    subprogram_bodies.push(res)
                }
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

    prefer_current_file(&mut final_results, current_uri);
    to_locations(final_results.into_iter().cloned())
}

/// Applies goto-declaration priority ordering to a pre-resolved result set.
///
/// Identical prioritization to [`lookup_declaration`] but operates on results
/// already resolved by the caller.
///
/// # Logic
/// 1.  **Prioritization:**
///     *   Subprograms: Favors signatures (`DeclType::FunctionDeclaration`) over bodies.
///     *   Components: Favors Component declarations over Entity declarations.
/// 2.  **Fallback:** If the preferred kind isn't found (e.g. no component defined),
///     it falls back to the Entity declaration.
pub fn apply_declaration_priority(
    results: Vec<LookupResult>,
    current_uri: &Url,
) -> Vec<Location> {
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

    prefer_current_file(&mut final_results, current_uri);
    to_locations(final_results.into_iter().cloned())
}

/// Applies goto-implementation priority ordering to a pre-resolved result set.
///
/// Identical prioritization to [`lookup_implementation`] but operates on results
/// already resolved by the caller.
///
/// # Logic
/// 1.  **Direct Match:** Filters for `Architecture`, `PackageBody`, or subprogram
///     implementations (`DeclType::Function`).
/// 2.  **Entity-to-Architecture:** If the target resolves to an Entity, performs a
///     global search to find all Architectures associated with that Entity.
pub fn apply_implementation_priority(
    results: Vec<LookupResult>,
    analysis_map: &AnalysisMap,
) -> Vec<Location> {
    let mut implementations: Vec<LookupResult> = results
        .iter()
        .filter(|res| match &res.item {
            ResolvedItem::Symbol(s) => {
                matches!(s.kind, OxideSymbolKind::Architecture | OxideSymbolKind::PackageBody)
            }
            ResolvedItem::Declaration(d) => {
                matches!(d.decl_type, DeclType::Function | DeclType::Procedure)
            }
        })
        .cloned()
        .collect();

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
                        && scope
                            .entity
                            .as_deref()
                            .unwrap_or_default()
                            .eq_ignore_ascii_case(ent_name)
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
                            && let Some(detail) = &sym.detail
                        {
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

    to_locations(implementations)
}

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
    apply_definition_priority(results, current_uri)
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
    apply_declaration_priority(results, current_uri)
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
    apply_implementation_priority(results, analysis_map)
}

#[cfg(test)]
mod tests;
