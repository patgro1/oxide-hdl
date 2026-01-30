use tower_lsp::lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind, Url};

use crate::{
    analysis::{Analysis, Declaration, Instance, OxideSymbolKind, ParseLevel, ScopeTree, Symbol},
    backend::AnalysisMap,
};

/// Converts an internal [`Declaration`] to an LSP [`DocumentSymbol`].
///
/// This helper is used by the `document_symbol` handler to generate the data structure
/// required for the **Outline View** and **Breadcrumbs**.
///
/// # Arguments
///
/// * `declaration` - The internal declaration struct produced by the a scope tree
///
/// # Returns
///
/// A `DocumentSymbol` struct compliant with the Language Server Protocol.
pub fn to_document_symbol(declaration: &Declaration) -> DocumentSymbol {
    let mut details = declaration.type_info.base_type.clone();
    if let Some(constraints) = &declaration.type_info.constraints {
        details.push_str(constraints);
    }
    #[allow(deprecated)]
    DocumentSymbol {
        name: declaration.name.clone(),
        detail: Some(details.clone()),
        kind: SymbolKind::from(declaration.decl_type),
        tags: None,
        deprecated: None,
        range: declaration.range,
        selection_range: declaration.selection_range,
        children: None,
    }
}

/// Converts an internal [`Instance`] to an LSP [`DocumentSymbol`].
///
/// This helper is used by the `document_symbol` handler to generate the data structure
/// required for the **Outline View** and **Breadcrumbs**.
///
/// # Arguments
///
/// * `instance` - The internal instance struct produced by the a scope tree
///
/// # Returns
///
/// A `DocumentSymbol` struct compliant with the Language Server Protocol.
pub fn instance_to_document_symbol(instance: &Instance) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: instance.label.clone(),
        detail: Some(format!("Instance of {}", instance.component)),
        kind: SymbolKind::from(OxideSymbolKind::ComponentInstantiation),
        children: None,
        range: instance.range,
        selection_range: instance.selection_range,
        tags: None,
        deprecated: None,
    }
}

/// Generate list of symbols for the current analysis
///
/// # Arguments
///
/// * `analysis` - The analysis for the current document
///
/// # Returns
///
/// A `DocumentSymbol` struct  array of the symbols contained in the analysis
pub fn collect_document_symbol(analysis: &Analysis) -> Vec<DocumentSymbol> {
    // Flatten all the scope trees in a single vector
    let mut scope_trees: Vec<&ScopeTree> = Vec::new();
    scope_trees.extend(analysis.scope_trees.iter());
    scope_trees.extend(analysis.entity_scope_trees.values().collect::<Vec<_>>());
    // Now we need to sort then by start value to make sure the ordering is respected in the
    // document
    scope_trees.sort_by_key(|a| a.range.start);
    let doc_sym = scope_trees
        .iter()
        .map(|scope_tree| collect_scope_tree_symbols(scope_tree))
        .collect();
    eprintln!("DOC_SYM: {:?}", doc_sym);
    doc_sym
}

/// Recursively build the document symbols for a scope tree
///
/// # Arguments
/// * `scope_tree`: The scope tree to analyze
///
/// # Returns:
/// A `DocumentSymbol` vector compliant with LSP
pub fn collect_scope_tree_symbols(scope_tree: &ScopeTree) -> DocumentSymbol {
    let mut children = Vec::new();

    children.extend(scope_tree.declarations.iter().map(to_document_symbol));
    children.extend(scope_tree.children.iter().map(collect_scope_tree_symbols));
    children.extend(
        scope_tree
            .instantiations
            .iter()
            .map(instance_to_document_symbol),
    );
    children.sort_by_key(|s| s.range.start);

    let opt_children = {
        if !children.is_empty() {
            Some(children)
        } else {
            None
        }
    };
    #[allow(deprecated)]
    DocumentSymbol {
        name: scope_tree.name.clone().unwrap_or(format!(
            "{} (line {})",
            scope_tree.kind, scope_tree.range.start.line
        )),
        kind: SymbolKind::from(scope_tree.kind),
        detail: None,
        deprecated: None,
        range: scope_tree.range,
        selection_range: scope_tree.range,
        tags: None,
        children: opt_children,
    }
}

/// Generate list of all symbols in the analysis map
///
/// This follows some rules for our purpose:
/// We will scan recursively all symbols to find:
///   - Entities
///   - Packages
///   - Functions in packages
///   - Constant in packages
///   - Types in packages
///   - Struct in packages
///
/// # Arguments
///
/// * `analysis_map` - The map of symbols for the current document
/// * 'query' - A query used to filter symbols
///
/// # Returns
///
/// A `SymbolInformation` struct  array of the symbols contained in the workspace filter
/// against our rules and the query
pub fn collect_workspace_symb(analysis_map: &AnalysisMap, query: &str) -> Vec<SymbolInformation> {
    let mut items = vec![];
    for (uri, map) in analysis_map {
        for symbol in map.symbols.values() {
            collect_symbols_recursive(symbol, uri, None, map.parse_level, query, &mut items);
        }
    }
    items
}

/// Helper that concert a symbol to a SymbolInformation struct
///
/// # Arguments
/// `symbol` - The symbol to translate
/// `file_uri` - The location of the symbol
///
/// # Returns
/// The translated symbol information
fn to_symbol_information(symbol: &Symbol, file_uri: &Url) -> SymbolInformation {
    #[allow(deprecated)]
    SymbolInformation {
        name: symbol.name.clone(),
        kind: symbol.kind.into(),
        location: Location {
            uri: file_uri.clone(),
            range: symbol.range,
        },
        tags: None,
        container_name: None,
        deprecated: None,
    }
}

/// Helper to filter which symbols can be added to the workspace symbol list
///
/// Filering is made with these rules:
/// - If the symbol is an entity, a component or a package, it will always be shown.
/// - If the symbol is a Function, a Constant or a Struct:
///    - We show everything that was shalow parsed because these *MUST* be important
///      worspace symbols.
///    - If is was not shallow parsed, we make sure that the symbol is part of
///      a package
///
/// This way we filter out every symbol that can't be seen globally in the worspace
/// as local constant, signals, local functions etc.
/// # Arguments
///
/// * `symbol` - the symbol to be checked
///
/// # Returns
/// true if the symbol is to be shown in the global scope
fn should_include_symbol(
    symbol: &Symbol,
    parent_kind: Option<OxideSymbolKind>,
    parse_level: ParseLevel,
) -> bool {
    match symbol.kind {
        // Always include these at any level
        OxideSymbolKind::Entity | OxideSymbolKind::Package | OxideSymbolKind::Component => true,

        // Only include if parent is a Package
        OxideSymbolKind::Function | OxideSymbolKind::Constant | OxideSymbolKind::Struct => {
            match parse_level {
                ParseLevel::Shallow => true,
                ParseLevel::Deep => matches!(parent_kind, Some(OxideSymbolKind::Package)),
            }
        }

        _ => false,
    }
}

/// Recursively add symbol information on symbol that need to be shown on the workspace_symbols
///
/// # Argument
/// `symbol` - the root symbol to walk from
/// `file_uri` - the file in which the symbol is contained
/// `query` - The filter query from the LSP client
/// `results` - Mutable reference to the vector of symbols
fn collect_symbols_recursive(
    symbol: &Symbol,
    file_uri: &Url,
    parent_kind: Option<OxideSymbolKind>,
    parse_level: ParseLevel,
    query: &str,
    results: &mut Vec<SymbolInformation>,
) {
    // If the symbol is entity/packges,structure, etc
    //   and containes query
    if should_include_symbol(symbol, parent_kind, parse_level)
        && symbol.name.to_lowercase().contains(&query.to_lowercase())
    {
        //   add to the result
        results.push(to_symbol_information(symbol, file_uri))
    }
    if symbol.kind == OxideSymbolKind::Package {
        for children in &symbol.children {
            // If the symbol is a container for stuff (aka a package)
            //   recurse into its children
            collect_symbols_recursive(
                children,
                file_uri,
                Some(symbol.kind),
                parse_level,
                query,
                results,
            );
        }
    }
}
