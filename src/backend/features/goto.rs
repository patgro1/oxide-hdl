use crate::analysis::OxideSymbolKind;
use crate::backend::{AnalysisMap, Location};
use tower_lsp::lsp_types::Url;

pub fn lookup_definition(
    target: &str,
    current_uri: &Url,
    analysis_map: &AnalysisMap,
) -> Vec<Location> {
    let target = target.to_lowercase();
    let mut locations = Vec::new();
    if let Some(analysis) = analysis_map.get(current_uri)
        && let Some(sym) = analysis.find_symbol(&target)
    {
        locations.push(Location {
            uri: current_uri.clone(),
            range: sym.range,
        });
    }
    if locations.is_empty() {
        for (file_uri, analysis) in analysis_map.iter() {
            if let Some(symbol) = analysis.symbols.get(&target) {
                locations.push(Location {
                    uri: file_uri.clone(),
                    range: symbol.range,
                });
            }
            // Nested match
            for root_sym in analysis.symbols.values() {
                if root_sym.kind == OxideSymbolKind::Package
                    && let Some(child) = root_sym.find_recursive(&target)
                {
                    locations.push(Location {
                        uri: file_uri.clone(),
                        range: child.range,
                    });
                }
            }
        }
    }
    locations
}
