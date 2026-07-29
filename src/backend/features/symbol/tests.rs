use crate::analysis::{Instance, InstantiatedUnitKind};
use tower_lsp::lsp_types::Range;

fn instance(component: &str, architecture: Option<&str>) -> Instance {
    Instance {
        label: "u0".to_string(),
        component: component.to_string(),
        library: Some("work".to_string()),
        architecture: architecture.map(|a| a.to_string()),
        unit_kind: InstantiatedUnitKind::Entity,
        range: Range::default(),
        selection_range: Range::default(),
    }
}

#[test]
fn test_outline_detail_without_architecture() {
    let sym = super::instance_to_document_symbol(&instance("cpu", None));
    assert_eq!(sym.detail.as_deref(), Some("Instance of cpu"));
}

#[test]
fn test_outline_detail_shows_explicit_architecture() {
    // `u0: entity work.cpu(behavioral)` names an architecture. The outline should
    // say which one, since the whole point of the spec is that `cpu` has several.
    let sym = super::instance_to_document_symbol(&instance("cpu", Some("behavioral")));
    assert_eq!(sym.detail.as_deref(), Some("Instance of cpu(behavioral)"));
}

#[test]
fn test_outline_detail_preserves_source_case() {
    let sym = super::instance_to_document_symbol(&instance("Cpu", Some("Behavioral")));
    assert_eq!(sym.detail.as_deref(), Some("Instance of Cpu(Behavioral)"));
}
