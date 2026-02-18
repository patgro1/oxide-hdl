use std::collections::HashSet;

use tower_lsp::lsp_types::{Position, Range, Url};

use crate::{
    analysis::{Analysis, DeclType, Declaration, OxideSymbolKind, ScopeTree, Symbol, TypeInfo},
    backend::AnalysisMap,
};

/// Unified result that can hold either a Rich Declaration or a Generic Symbol
#[derive(Debug, Clone)]
pub enum ResolvedItem {
    /// A precise declaration (Signal, Port, Var)
    Declaration(Declaration),
    /// A generic symbol (entity, package or regex match)
    Symbol(Symbol),
}

impl ResolvedItem {
    /// Returns the source location range of the resolved item.
    pub fn range(&self) -> Range {
        match self {
            ResolvedItem::Declaration(d) => d.range,
            ResolvedItem::Symbol(s) => s.range,
        }
    }

    /// Returns the selection range for navigation (name location only).
    pub fn selection_range(&self) -> Range {
        match self {
            ResolvedItem::Declaration(d) => d.selection_range,
            ResolvedItem::Symbol(s) => s.range, // Symbol do not store a selection range
        }
    }

    /// Returns the name of the resolved item.
    #[allow(dead_code)]
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

/// Looks up a symbol by name using hierarchical resolution.
///
/// Resolution order:
/// 1. Local scope - signals, variables, constants visible at the cursor position
/// 2. Imported packages - symbols from `use` clauses (Headers only)
/// 3. Global top-level - entities and packages by name
///
/// # Arguments
/// * `target` - The symbol name to look up (case-insensitive).
/// * `current_uri` - The URI of the file containing the cursor.
/// * `analysis_map` - The global analysis map for cross-file lookups.
/// * `pos` - The cursor position for scope resolution.
/// * `find_all` - If true, continues searching all files even if local matches are found. 
///   Useful for prioritization logic in navigation.
///
/// # Returns
/// A vector of all matching lookup results with source URIs.
pub fn lookup_symbol(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: &Position,
    find_all: bool,
) -> Vec<LookupResult> {
    let mut results = Vec::new();
    let target_lc = target.to_lowercase();
    let analysis = analysis_map.get(current_uri);

    if let Some(analysis) = analysis {
        // 1. Local lookup
        if let Some(root_scope) = analysis.find_scope_tree_at(pos) {
            let innermost = root_scope.find_innermost_scope(pos);
            let header_scope = root_scope
                .entity
                .as_ref()
                .and_then(|name| analysis.entity_scope_trees.get(&name.to_lowercase()))
                .or_else(|| {
                    root_scope
                        .package
                        .as_ref()
                        .and_then(|name| {
                            analysis.package_declaration_scope_trees.get(&name.to_lowercase())
                        })
                });
            if let Some(locals) =
                root_scope.collect_visible_declarations(&innermost.range, header_scope)
            {
                for decl in locals {
                    if decl.name.to_lowercase() == target_lc {
                        results.push(LookupResult {
                            item: ResolvedItem::Declaration(decl),
                            source_uri: current_uri.clone(),
                        });
                    }
                }
            }
            let chain = root_scope.collect_scope_chain(pos);
            for s in chain.iter().rev() {
                if let Some(inst) = s
                    .instantiations
                    .iter()
                    .find(|i| i.label.eq_ignore_ascii_case(&target_lc))
                {
                    results.push(LookupResult {
                        item: ResolvedItem::Declaration(Declaration {
                            name: inst.label.clone(),
                            decl_type: DeclType::Component,
                            range: inst.range,
                            selection_range: inst.selection_range,
                            type_info: TypeInfo {
                                base_type: inst.component.clone(),
                                constraints: None,
                                is_array: false,
                            },
                            default_value: None,
                            doc_comment: None,
                            parameters: None,
                        }),
                        source_uri: current_uri.clone(),
                    });
                    break;
                }
            }
        }
        if !results.is_empty() && !find_all {
            return results;
        }

        // 2. Imports (Strict: Headers only)
        resolve_imports_for_symbol(analysis, &target_lc, analysis_map, &mut results);
        if !results.is_empty() && !find_all {
            return results;
        }
    }

    // 3. Global search (Entities, Package Headers, and Package Bodies if find_all)
    if results.is_empty() || find_all {
        resolve_global_toplevel_symbols(&target_lc, analysis_map, &mut results);
    }

    deduplicate_results(results)
}

/// Looks up a procedure or function declaration by name.
///
/// Uses `lookup_symbol` internally but filters results to only return
/// declarations that are functions or procedures. Useful for resolving
/// subprogram calls to get parameter information.
///
/// # Arguments
/// * `name` - The procedure/function name to look up.
/// * `uri` - The URI of the current file.
/// * `map` - The global analysis map.
/// * `pos` - The cursor position for scope context.
///
/// # Returns
/// The declaration if a matching procedure/function is found.
pub fn lookup_procedure_declaration(
    name: &str,
    uri: &Url,
    map: &AnalysisMap,
    pos: &Position,
) -> Option<Declaration> {
    let results = lookup_symbol(name, uri, map, pos, false);

    results.into_iter().find_map(|res| match res.item {
        ResolvedItem::Declaration(decl) => match decl.decl_type {
            DeclType::Procedure
            | DeclType::Function
            | DeclType::ProcedureDeclaration
            | DeclType::FunctionDeclaration => Some(decl),
            _ => None,
        },
        ResolvedItem::Symbol(_) => None,
    })
}

/// Resolves a symbol through imported packages from `use` clauses.
///
/// For each `use` clause in the current file's analysis, searches the referenced
/// package for matching symbols. 
/// 
/// VHDL Rule: Only Package DECLARATIONS (Headers) are searched for imported symbols.
///
/// # Arguments
/// * `analysis` - The analysis of the current file containing `use` clauses.
/// * `target` - The symbol name to find (lowercase).
/// * `map` - The global analysis map to search packages in.
/// * `results` - Mutable vector to append found results to.
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
                .package_declaration_scope_trees
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
                && (pkg_sym.kind == OxideSymbolKind::Package || pkg_sym.kind == OxideSymbolKind::PackageBody)
            {
                // Shallow Fallback: If we found the package (or body) via regex, 
                // check if the target symbol exists anywhere in that file's flat symbol map.
                let find_and_push = |t: &str, output: &mut Vec<LookupResult>| {
                    if let Some(sym) = global_analysis.symbols.get(&t.to_lowercase()) {
                        output.push(LookupResult {
                            item: ResolvedItem::Symbol(sym.clone()),
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

/// Resolves a symbol against global top-level definitions (entities and packages).
///
/// This is the fallback resolution when local and import lookups fail. Searches
/// for entities and packages by name across all files in the workspace.
///
/// Checks both shallow-indexed `symbols` and deep-parsed `entity_scope_trees`
/// and `package_scope_trees` to ensure consistent results regardless of parse level.
///
/// # Arguments
/// * `target` - The symbol name to find (lowercase).
/// * `map` - The global analysis map to search.
/// * `results` - Mutable vector to append found results to.
fn resolve_global_toplevel_symbols(
    target: &str,
    map: &AnalysisMap,
    results: &mut Vec<LookupResult>,
) {
    for (uri, analysis) in map.iter() {
        if let Some(sym) = analysis.symbols.get(target)
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

        if let Some(scope) = analysis.package_declaration_scope_trees.get(target) {
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

        if let Some(scope) = analysis.package_body_scope_trees.get(target) {
            results.push(LookupResult {
                item: ResolvedItem::Symbol(Symbol {
                    name: scope.name.clone().unwrap_or_default(),
                    kind: OxideSymbolKind::PackageBody,
                    detail: None,
                    range: scope.range,
                    children: vec![],
                }),
                source_uri: uri.clone(),
            });
        }
        
        // Broad search for subprograms inside packages (Deep Parse fallback)
        for scope in analysis.package_declaration_scope_trees.values() {
            for decl in &scope.declarations {
                if decl.name.eq_ignore_ascii_case(target) {
                    results.push(LookupResult {
                        item: ResolvedItem::Declaration(decl.clone()),
                        source_uri: uri.clone(),
                    });
                }
            }
        }
        for scope in analysis.package_body_scope_trees.values() {
            for decl in &scope.declarations {
                if decl.name.eq_ignore_ascii_case(target) {
                    results.push(LookupResult {
                        item: ResolvedItem::Declaration(decl.clone()),
                        source_uri: uri.clone(),
                    });
                }
            }
        }
    }
}

/// Maps a logical VHDL library name to the list of physical cache folders to search.
///
/// Handles the fact that "ieee" often contains "synopsys" legacy packages,
/// so multiple folders may need to be searched for a single logical library.
///
/// # Arguments
/// * `logical_lib` - The VHDL library name (e.g., "ieee", "std", "unisim").
///
/// # Returns
/// A list of cache folder names to search, or empty if not a known builtin.
fn get_builtin_search_paths(logical_lib: &str) -> Vec<&'static str> {
    match logical_lib.to_lowercase().as_str() {
        "ieee" => vec!["ieee", "synopsys", "ieee2008"], // Search all three for "ieee"
        "std" => vec!["std"],
        "unisim" => vec!["unisim"],
        _ => vec![], // Not a builtin known to us
    }
}

/// Attempts to find a package file in the internal temporary cache.
///
/// Searches the oxide_lsp_cache directory in the system temp folder for
/// standard library packages (IEEE, STD, etc.). Tries exact filename match
/// first, then falls back to prefix matching.
///
/// # Arguments
/// * `library_name` - The VHDL library name (e.g., "ieee").
/// * `package_name` - The package name to find (e.g., "std_logic_1164").
///
/// # Returns
/// The URL of the package file if found in the cache.
fn resolve_internal_lib(library_name: &str, package_name: &str) -> Option<Url> {
    let mut cache_root = std::env::temp_dir();
    cache_root.push("oxide_lsp_cache");
    let folders = get_builtin_search_paths(library_name);
    if folders.is_empty() {
        return None;
    }

    for folder in folders {
        let mut candidate_dir = cache_root.clone();
        candidate_dir.push(folder);

        if !candidate_dir.exists() {
            // TRACE 2: Directory missing?
            continue;
        }
        let exact_path = candidate_dir.join(format!("{}.vhdl", package_name.to_lowercase()));
        if exact_path.exists() {
            return Url::from_file_path(exact_path).ok();
        }

        if let Ok(entries) = std::fs::read_dir(&candidate_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(stem) = path.file_stem() {
                    let stem_str = stem.to_string_lossy().to_lowercase();
                    // If filename starts with package name or equals package name
                    if stem_str == package_name.to_lowercase() {
                        return Url::from_file_path(path).ok();
                    }
                }
            }
        }
    }

    None
}

/// Resolves a VHDL use clause to a file URI for JIT loading.
///
/// Given a library and package name from a `use` clause, attempts to find
/// the corresponding source file. Currently checks the internal cache for
/// standard libraries; user-configured library paths are planned.
///
/// # Arguments
/// * `library_name` - The library name from the use clause (e.g., "ieee", "work").
/// * `package_name` - The package name to resolve (e.g., "std_logic_1164").
///
/// # Returns
/// The file URI if the package can be resolved, `None` otherwise.
pub fn resolve_import_uri(
    library_name: &str,
    package_name: &str,
    // _user_config_libs: &HashMap<String, PathBuf>,
) -> Option<Url> {
    // 1. Check Internal Cache
    if let Some(url) = resolve_internal_lib(library_name, package_name) {
        return Some(url);
    }

    // 2. Check User Configuration
    // If the user defined [libraries] unisim = "/path/to/xilinx"
    // if let Some(lib_path) = user_config_libs.get(&library_name.to_lowercase()) {
    //     if let Some(url) = resolve_vendor_filename(package_name, lib_path) {
    //         return Some(url);
    //     }
    // }

    None
}

/// Removes duplicate lookup results based on source location.
///
/// Deduplication is based on (URI, line, character) to handle cases where
/// the same symbol might be found through multiple resolution paths
/// (e.g., both via import and global search).
///
/// # Arguments
/// * `results` - Vector of lookup results that may contain duplicates.
///
/// # Returns
/// A deduplicated vector preserving the first occurrence of each unique location.
fn deduplicate_results(results: Vec<LookupResult>) -> Vec<LookupResult> {
    let mut unique = Vec::new();
    let mut seen = HashSet::new();

    for res in results {
        let range = res.item.range();
        let signature = (
            res.source_uri.to_string(),
            range.start.line,
            range.start.character,
        );
        if seen.insert(signature) {
            unique.push(res);
        }
    }
    unique
}

/// Resolves a chain of identifiers (e.g., `["my_rec", "sub", "field"]`) to its definition.
///
/// Starting from the first identifier, each subsequent part is resolved by looking up the
/// type of the current declaration and finding matching fields in its `parameters` list.
/// This handles nested record field access (e.g., `my_rec.sub.field`).
///
/// For hover: pass the full chain up to (and including) the hovered identifier.
/// For completion: pass the prefix chain (the part before the dot).
///
/// # Arguments
/// * `chain` - Ordered list of identifiers to traverse (e.g., `["rec", "field"]`).
/// * `current_uri` - The URI of the file containing the cursor.
/// * `analysis_map` - The global analysis map for cross-file lookups.
/// * `pos` - The cursor position for scope resolution.
///
/// # Returns
/// A vector of matching `LookupResult`s for the final identifier in the chain.
pub fn resolve_path_chain(
    chain: &[String],
    current_uri: &Url,
    analysis_map: &AnalysisMap,
    pos: &Position,
) -> Vec<LookupResult> {
    if chain.is_empty() {
        return vec![];
    }
    let root_name = &chain[0];
    let mut current_results = lookup_symbol(root_name, current_uri, analysis_map, pos, false);

    for (_, part) in chain.iter().enumerate().skip(1) {
        let mut next_results = Vec::new();
        for res in current_results {
            match &res.item {
                // If it is a declation, we need to check its type for the fields
                ResolvedItem::Declaration(decl) => {
                    let type_name = &decl.type_info.base_type;
                    let type_defs = lookup_symbol(type_name, current_uri, analysis_map, pos, false);
                    for type_res in type_defs {
                        if let ResolvedItem::Declaration(type_decl) = type_res.item
                            && let Some(fields) = &type_decl.parameters
                            && let Some(field) =
                                fields.iter().find(|f| f.name.eq_ignore_ascii_case(part))
                        {
                            next_results.push(LookupResult {
                                item: ResolvedItem::Declaration(field.clone()),
                                source_uri: type_res.source_uri.clone(),
                            });
                        }
                    }
                }
                ResolvedItem::Symbol(sym) => {
                    if let Some(child) = sym
                        .children
                        .iter()
                        .find(|c| c.name.eq_ignore_ascii_case(part))
                    {
                        next_results.push(LookupResult {
                            item: ResolvedItem::Symbol(child.clone()),
                            source_uri: res.source_uri.clone(),
                        });
                    }
                }
            }
        }
        current_results = next_results;
        if current_results.is_empty() {
            break;
        }
    }

    current_results
}
