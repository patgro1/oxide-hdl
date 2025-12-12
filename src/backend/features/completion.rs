use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind,
};

use crate::analysis::{Analysis, OxideSymbolKind, Symbol};

/// Generates a list of completions for the current file scope.
///
/// For V1, this is a "flat" completion. It grabs every Signal, Port, Constant,
/// and Component declared in the file and offers them as suggestions.
///
/// # Arguments
/// * `analysis` - The deep analysis of the current file.
/// # Return
/// Vector of completion items
pub fn complete_local_scope(analysis: &Analysis) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Iterate over all top-level symbols
    for sym in analysis.symbols.values() {
        collect_symbols(sym, &mut items);
    }

    items
}

/// Recursive helper to flatten the symbol tree into suggestions.
/// # Arguments
/// * `symbol` - The top level symbol we want to collect symbols from
/// * `items` - Mutable reference to the a vector of completion items
fn collect_symbols(symbol: &Symbol, items: &mut Vec<CompletionItem>) {
    // Current symbol is a completion item
    if let Some(item) = symbol_to_completion(symbol) {
        items.push(item);
    }

    // Recurse into children
    for child in &symbol.children {
        collect_symbols(child, items);
    }
}

fn symbol_to_completion(symbol: &Symbol) -> Option<CompletionItem> {
    let kind = match symbol.kind {
        OxideSymbolKind::Entity => CompletionItemKind::INTERFACE,
        OxideSymbolKind::Architecture => return None, // Don't suggest Arch names in code usually
        OxideSymbolKind::Port => CompletionItemKind::FIELD,
        OxideSymbolKind::Signal => CompletionItemKind::VARIABLE,
        OxideSymbolKind::Constant => CompletionItemKind::CONSTANT,
        OxideSymbolKind::Struct => CompletionItemKind::STRUCT,
        OxideSymbolKind::Function => CompletionItemKind::FUNCTION,
        OxideSymbolKind::Process => return None, // Don't suggest Process labels usually
        OxideSymbolKind::Component => CompletionItemKind::CLASS,
        _ => CompletionItemKind::TEXT,
    };
    Some(CompletionItem {
        label: symbol.name.clone(),
        kind: Some(kind),
        detail: symbol.detail.clone(),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("**{}** ({})", symbol.name, symbol.kind),
        })),
        ..CompletionItem::default()
    })
}
