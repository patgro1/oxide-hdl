use tower_lsp::lsp_types::DocumentSymbol;

use crate::analysis::Analysis;

/// Recursively converts an internal [`Symbol`] to an LSP [`DocumentSymbol`].
///
/// This helper is used by the `document_symbol` handler to generate the data structure
/// required for the **Outline View** and **Breadcrumbs**.
///
/// # Behavior
/// * **Recursion:** It traverses the `children` vector of the symbol and converts them
///   depth-first.
/// * **Sorting:** It sorts children by their start position (`range.start`). This ensures
///   that the Outline View lists items in the order they appear in the file, which is
///   critical for readability in VHDL (e.g., ports appearing in order).
///
/// # Arguments
///
/// * `sym` - The internal symbol struct produced by the parser or scanner.
///
/// # Returns
///
/// A `DocumentSymbol` struct compliant with the Language Server Protocol.
pub fn to_document_symbol(sym: &crate::analysis::Symbol) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: sym.name.clone(),
        detail: sym.detail.clone(),
        kind: sym.kind.into(),
        tags: None,
        deprecated: None,
        range: sym.range,
        selection_range: sym.range,
        children: if sym.children.is_empty() {
            None
        } else {
            let mut children_list: Vec<DocumentSymbol> =
                sym.children.iter().map(to_document_symbol).collect();
            children_list.sort_by(|a, b| a.range.start.cmp(&b.range.start));
            Some(children_list)
        },
    }
}

/// Generate list of symbols for the current analysis
///
/// # Arguments
///
/// * `analysis` - The map of symbols for the current document
///
/// # Returns
///
/// A `DocumentSymbol` struct  array of the symbols contained in the analysis
pub fn collect_document_symbol(analysis: &Analysis) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    for sym in analysis.symbols.values() {
        symbols.push(to_document_symbol(sym))
    }
    symbols
}
