use crate::analysis::OxideSymbolKind;
use crate::backend::{AnalysisMap, Location};
use tower_lsp::lsp_types::Url;

/// Resolves the definition location(s) for a given symbol.
///
/// This function implements the "Go to Definition" logic, handling scoping rules
/// and multiple candidates (overloading).
///
/// # Logic
///
/// 1.  **Local Search (Priority):** Checks the current file first. If the symbol is defined locally
///     (e.g., a signal in the current architecture), it returns that location and stops searching.
///     This ensures local variables shadow global names.
/// 2.  **Global Search (Fallback):** If not found locally, it scans the entire workspace index.
///     * **Top-Level:** Matches Entities and Packages.
///     * **Nested:** Matches Functions/Procedures defined inside Packages (by crawling the package hierarchy).
///
/// # Arguments
///
/// * `target` - The identifier string to look up (e.g., "clk", "my_func").
/// * `current_uri` - The URI of the file where the request originated (used for local search).
/// * `analysis_map` - The global index of all parsed files.
///
/// # Returns
///
/// A `Vec<Location>`. Returns a vector because VHDL supports overloading (multiple functions
/// with the same name), so a single lookup might result in multiple valid definitions.
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
                    && let Some(child) = root_sym.find_child(&target)
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
