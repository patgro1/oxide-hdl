//! Library-aware design-unit queries over the workspace analysis map.
//!
//! These are pure, synchronous functions over an immutable [`AnalysisMap`] — no
//! locks, no I/O. They are the single place that knows how a VHDL library name
//! maps onto indexed files, and they are consumed by completion and by the JIT
//! parse scheduler.
//!
//! # The `work` rule
//!
//! `work` is not a library. It is a self-reference to whichever library the
//! current design unit is being compiled into. `entity work.cpu` written inside a
//! file belonging to `rtl_lib` therefore resolves against `rtl_lib`.
//!
//! # Graceful degradation
//!
//! When a library-scoped lookup finds nothing, these functions fall back to a
//! name-only search across every file. A workspace with no `[libraries]` section
//! has every file in `work`, so this fallback makes the library dimension a
//! complete no-op there — matching pre-library behaviour exactly.

use crate::analysis::{Analysis, Instance, InstantiatedUnitKind, OxideSymbolKind};
use crate::backend::AnalysisMap;
use tower_lsp::lsp_types::Url;

/// Returns `true` if `analysis` declares an entity called `name_lc`.
///
/// Checks both parse levels: shallow files record entities in `symbols`, deep
/// files in `entity_scope_trees`. A file may be either at any given moment.
///
/// # Arguments
/// * `analysis` - The per-file analysis to inspect.
/// * `name_lc` - Entity name, already lowercased.
pub fn file_declares_entity(analysis: &Analysis, name_lc: &str) -> bool {
    if analysis.entity_scope_trees.contains_key(name_lc) {
        return true;
    }
    analysis
        .symbols
        .get(name_lc)
        .is_some_and(|s| s.kind == OxideSymbolKind::Entity)
}

/// Resolves a direct entity instantiation to the file(s) declaring that entity.
///
/// Returns an empty vector for component and configuration instantiations —
/// components resolve through component declarations, and configurations are not
/// resolved by Oxide HDL. More than one URI means the entity is declared twice in
/// the same library, which is a genuine (currently unreported) design error.
///
/// # Arguments
/// * `map` - The workspace analysis map.
/// * `inst` - The instantiation to resolve.
/// * `current_library` - Library of the file containing `inst`, used to expand `work`.
///
/// # Returns
/// Matching file URIs, sorted for stable ordering.
pub fn resolve_entity_uris(map: &AnalysisMap, inst: &Instance, current_library: &str) -> Vec<Url> {
    if inst.unit_kind != InstantiatedUnitKind::Entity {
        return Vec::new();
    }

    let name_lc = inst.component.to_lowercase();
    if name_lc.is_empty() {
        return Vec::new();
    }

    // `work` means "the library this file compiles into". Normalize BEFORE matching:
    // `Instance.library` holds source text, so `ENTITY WORK.CPU` must still be
    // recognised as the self-reference rather than treated as a library named "work".
    let library_lc = inst.library.as_deref().map(str::to_lowercase);
    let effective_library = match library_lc.as_deref() {
        Some("work") => Some(current_library.to_lowercase()),
        Some(other) => Some(other.to_string()),
        None => None,
    };

    if let Some(library) = effective_library {
        let mut scoped: Vec<Url> = map
            .iter()
            .filter(|(_, a)| a.library == library && file_declares_entity(a, &name_lc))
            .map(|(u, _)| u.clone())
            .collect();
        if !scoped.is_empty() {
            scoped.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            return scoped;
        }
    }

    // Fallback: name-only search. Keeps unconfigured workspaces working exactly
    // as they did before libraries existed.
    let mut any: Vec<Url> = map
        .iter()
        .filter(|(_, a)| file_declares_entity(a, &name_lc))
        .map(|(u, _)| u.clone())
        .collect();
    any.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    any
}

/// Lists every entity declared in `library`.
///
/// Served entirely from the shallow index — no deep parsing — so it is safe to
/// call for a library holding hundreds of files.
///
/// # Arguments
/// * `map` - The workspace analysis map.
/// * `library` - Library name; matched case-insensitively.
///
/// # Returns
/// `(entity name, declaring file)` pairs, sorted by entity name and deduplicated.
pub fn entities_in_library(map: &AnalysisMap, library: &str) -> Vec<(String, Url)> {
    let library_lc = library.to_lowercase();
    let mut out: Vec<(String, Url)> = Vec::new();

    for (uri, analysis) in map.iter() {
        if analysis.library != library_lc {
            continue;
        }
        for tree_name in analysis.entity_scope_trees.keys() {
            out.push((tree_name.clone(), uri.clone()));
        }
        for symbol in analysis.symbols.values() {
            if symbol.kind == OxideSymbolKind::Entity {
                out.push((symbol.name.to_lowercase(), uri.clone()));
            }
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Lists every library name present in the index, sorted and deduplicated.
///
/// With no `[libraries]` configured this returns just `["work"]`.
pub fn known_libraries(map: &AnalysisMap) -> Vec<String> {
    let mut libs: Vec<String> = map.values().map(|a| a.library.clone()).collect();
    libs.sort();
    libs.dedup();
    libs
}

#[cfg(test)]
mod tests {
    use crate::analysis::{
        Analysis, Instance, InstantiatedUnitKind, OxideSymbolKind, ParseLevel, Symbol,
    };
    use crate::backend::AnalysisMap;
    use tower_lsp::lsp_types::{Range, Url};

    /// Builds a shallow-indexed Analysis declaring one entity, in a given library.
    fn shallow_entity(library: &str, entity: &str) -> Analysis {
        let mut a = Analysis::new();
        a.library = library.to_string();
        a.parse_level = ParseLevel::Shallow;
        a.symbols.insert(
            entity.to_lowercase(),
            Symbol {
                name: entity.to_string(),
                kind: OxideSymbolKind::Entity,
                detail: Some("Entity".to_string()),
                range: Range::default(),
                children: Vec::new(),
            },
        );
        a
    }

    fn inst(library: Option<&str>, name: &str, kind: InstantiatedUnitKind) -> Instance {
        Instance {
            label: "u0".to_string(),
            component: name.to_string(),
            library: library.map(|l| l.to_string()),
            architecture: None,
            unit_kind: kind,
            range: Range::default(),
            selection_range: Range::default(),
        }
    }

    fn uri(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn map_of(entries: Vec<(&str, Analysis)>) -> AnalysisMap {
        let mut m = AnalysisMap::new();
        for (u, a) in entries {
            m.insert(uri(u), a);
        }
        m
    }

    #[test]
    fn test_resolves_entity_in_named_library() {
        let m = map_of(vec![
            ("file:///a/cpu.vhd", shallow_entity("rtl_lib", "cpu")),
            ("file:///b/cpu.vhd", shallow_entity("other_lib", "cpu")),
        ]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("rtl_lib"), "cpu", InstantiatedUnitKind::Entity),
            "work",
        );
        assert_eq!(got, vec![uri("file:///a/cpu.vhd")]);
    }

    #[test]
    fn test_work_resolves_to_current_files_library() {
        // `entity work.cpu` written from a file in other_lib must hit other_lib's cpu.
        let m = map_of(vec![
            ("file:///a/cpu.vhd", shallow_entity("rtl_lib", "cpu")),
            ("file:///b/cpu.vhd", shallow_entity("other_lib", "cpu")),
        ]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("work"), "cpu", InstantiatedUnitKind::Entity),
            "other_lib",
        );
        assert_eq!(got, vec![uri("file:///b/cpu.vhd")]);
    }

    #[test]
    fn test_uppercase_work_still_expands_to_current_library() {
        // `Instance.library` preserves source case, so the `work` self-reference must
        // be matched case-insensitively. Getting this wrong searches for a library
        // literally named "work" instead of expanding — a silent wrong answer.
        let m = map_of(vec![
            ("file:///a/cpu.vhd", shallow_entity("rtl_lib", "cpu")),
            ("file:///b/cpu.vhd", shallow_entity("other_lib", "cpu")),
        ]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("WORK"), "cpu", InstantiatedUnitKind::Entity),
            "other_lib",
        );
        assert_eq!(got, vec![uri("file:///b/cpu.vhd")]);
    }

    #[test]
    fn test_unconfigured_workspace_falls_back_to_name_search() {
        // Everything is in `work`; `entity mylib.cpu` names a library nobody declares.
        // Rather than fail, fall back to the legacy name-only search.
        let m = map_of(vec![("file:///a/cpu.vhd", shallow_entity("work", "cpu"))]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("mylib"), "cpu", InstantiatedUnitKind::Entity),
            "work",
        );
        assert_eq!(got, vec![uri("file:///a/cpu.vhd")]);
    }

    #[test]
    fn test_component_instantiation_resolves_to_nothing() {
        // Components resolve through component declarations, not this function.
        let m = map_of(vec![("file:///a/cpu.vhd", shallow_entity("work", "cpu"))]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(None, "cpu", InstantiatedUnitKind::Component),
            "work",
        );
        assert!(got.is_empty(), "expected no resolution, got {got:?}");
    }

    #[test]
    fn test_unknown_entity_resolves_to_nothing() {
        let m = map_of(vec![("file:///a/cpu.vhd", shallow_entity("work", "cpu"))]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("work"), "nonexistent", InstantiatedUnitKind::Entity),
            "work",
        );
        assert!(got.is_empty());
    }

    #[test]
    fn test_duplicate_entity_in_one_library_returns_both_sorted() {
        let m = map_of(vec![
            ("file:///z/cpu.vhd", shallow_entity("rtl_lib", "cpu")),
            ("file:///a/cpu.vhd", shallow_entity("rtl_lib", "cpu")),
        ]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("rtl_lib"), "cpu", InstantiatedUnitKind::Entity),
            "work",
        );
        assert_eq!(
            got,
            vec![uri("file:///a/cpu.vhd"), uri("file:///z/cpu.vhd")],
            "results must be sorted for stability"
        );
    }

    #[test]
    fn test_resolution_is_case_insensitive() {
        let m = map_of(vec![("file:///a/cpu.vhd", shallow_entity("rtl_lib", "CPU"))]);
        let got = super::resolve_entity_uris(
            &m,
            &inst(Some("RTL_LIB"), "Cpu", InstantiatedUnitKind::Entity),
            "work",
        );
        assert_eq!(got, vec![uri("file:///a/cpu.vhd")]);
    }

    #[test]
    fn test_entities_in_library_lists_and_sorts() {
        let m = map_of(vec![
            ("file:///a/uart.vhd", shallow_entity("rtl_lib", "uart_tx")),
            ("file:///b/cpu.vhd", shallow_entity("rtl_lib", "cpu")),
            ("file:///c/x.vhd", shallow_entity("other_lib", "excluded")),
        ]);
        let got: Vec<String> = super::entities_in_library(&m, "rtl_lib")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(got, vec!["cpu".to_string(), "uart_tx".to_string()]);
    }

    #[test]
    fn test_entities_in_library_empty_for_unknown_library() {
        let m = map_of(vec![("file:///a/cpu.vhd", shallow_entity("work", "cpu"))]);
        assert!(super::entities_in_library(&m, "nope").is_empty());
    }

    #[test]
    fn test_known_libraries_deduped_and_sorted() {
        let m = map_of(vec![
            ("file:///a.vhd", shallow_entity("zeta", "a")),
            ("file:///b.vhd", shallow_entity("alpha", "b")),
            ("file:///c.vhd", shallow_entity("alpha", "c")),
        ]);
        assert_eq!(
            super::known_libraries(&m),
            vec!["alpha".to_string(), "zeta".to_string()]
        );
    }

    #[test]
    fn test_file_declares_entity_finds_deep_parsed_entity() {
        // A deep-parsed file exposes entities via entity_scope_trees, not symbols.
        // Parse real source rather than hand-building a ScopeTree — there is no
        // root constructor, only `ScopeTree::new(kind, &Node)`.
        //
        // NOTE: `test_utils::parse_text` acquires SHARED_PARSER_LOCK itself, so this
        // must NOT take the guard first — std::sync::Mutex is not reentrant and doing
        // so deadlocks the test.
        let src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
        let tree = crate::backend::test_utils::parse_text(src);
        let a = crate::backend::syntax::parser::extract_document_symbols(src, tree.root_node());

        assert!(
            a.entity_scope_trees.contains_key("uart_tx"),
            "fixture must produce an entity scope tree"
        );
        assert!(super::file_declares_entity(&a, "uart_tx"));
        assert!(!super::file_declares_entity(&a, "other"));
    }
}
