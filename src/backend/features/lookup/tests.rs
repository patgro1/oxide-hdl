use crate::backend::AnalysisMap;
use crate::backend::features::lookup::resolve_path_chain;
use crate::backend::test_utils::parse_text;
use tower_lsp::lsp_types::{Position, Url};

fn setup_workspace(files: Vec<(&str, &str)>) -> (AnalysisMap, Vec<Url>) {
    let mut map = AnalysisMap::new();
    let mut uris = Vec::new();
    for (name, content) in &files {
        let uri = Url::parse(&format!("file:///{}", name)).unwrap();
        let tree = parse_text(content);
        let analysis =
            crate::backend::syntax::parser::extract_document_symbols(content, tree.root_node());
        map.insert(uri.clone(), analysis);
        uris.push(uri);
    }
    (map, uris)
}

fn two_pkg_workspace() -> (AnalysisMap, Vec<Url>) {
    setup_workspace(vec![
        (
            "pkg_a.vhd",
            "package pkg_a is\n  function my_func return boolean;\nend package;",
        ),
        (
            "pkg_b.vhd",
            "package pkg_b is\n  function my_func return boolean;\nend package;",
        ),
        (
            "top.vhd",
            "use work.pkg_a.my_func;\nuse work.pkg_b.my_func;\narchitecture rtl of top is\nbegin\nend architecture;",
        ),
    ])
}

// -----------------------------------------------------------------------
// Package-qualified access: pkg_a.my_func / pkg_b.my_func
// -----------------------------------------------------------------------

// Hovering/goto on the member of a qualified name should return exactly
// one location — the one in the specified package — even when an identically
// named symbol is also imported from another package.
#[test]
fn test_resolve_path_chain_pkg_a_no_ambiguity() {
    let (map, uris) = two_pkg_workspace();
    let pos = Position { line: 3, character: 0 };

    let results = resolve_path_chain(
        &["pkg_a".to_string(), "my_func".to_string()],
        &uris[2],
        &map,
        &pos,
    );

    assert_eq!(results.len(), 1, "got {}", results.len());
    assert_eq!(results[0].source_uri.path(), "/pkg_a.vhd");
}

#[test]
fn test_resolve_path_chain_pkg_b_no_ambiguity() {
    let (map, uris) = two_pkg_workspace();
    let pos = Position { line: 3, character: 0 };

    let results = resolve_path_chain(
        &["pkg_b".to_string(), "my_func".to_string()],
        &uris[2],
        &map,
        &pos,
    );

    assert_eq!(results.len(), 1, "got {}", results.len());
    assert_eq!(results[0].source_uri.path(), "/pkg_b.vhd");
}

// -----------------------------------------------------------------------
// Three-part chain: library.package.member  (work.pkg_a.my_func)
// -----------------------------------------------------------------------

// The grammar produces a flat [name] with multiple [selection] children,
// so extract_chain_at_pos yields ["work", "pkg_a", "my_func"].
// resolve_path_chain must strip the leading library segment.
#[test]
fn test_resolve_path_chain_work_prefix_stripped() {
    let (map, uris) = two_pkg_workspace();
    let pos = Position { line: 3, character: 0 };

    let results = resolve_path_chain(
        &["work".to_string(), "pkg_a".to_string(), "my_func".to_string()],
        &uris[2],
        &map,
        &pos,
    );

    assert_eq!(results.len(), 1, "got {}", results.len());
    assert_eq!(results[0].source_uri.path(), "/pkg_a.vhd");
}

// ieee is a known builtin library — same strip applies.
#[test]
fn test_resolve_path_chain_ieee_prefix_stripped() {
    let (map, uris) = setup_workspace(vec![
        (
            "std_logic_1164.vhd",
            // Minimal stub — just enough for the package to be indexed
            "package std_logic_1164 is\n  type std_logic is ('0', '1');\nend package;",
        ),
        (
            "top.vhd",
            "architecture rtl of top is\nbegin\nend architecture;",
        ),
    ]);
    let pos = Position { line: 1, character: 0 };

    let results = resolve_path_chain(
        &[
            "ieee".to_string(),
            "std_logic_1164".to_string(),
            "std_logic".to_string(),
        ],
        &uris[1],
        &map,
        &pos,
    );

    assert_eq!(results.len(), 1, "got {}", results.len());
    assert_eq!(results[0].source_uri.path(), "/std_logic_1164.vhd");
}

// -----------------------------------------------------------------------
// Regression: record field access must still work
// -----------------------------------------------------------------------

// resolve_path_chain for a local signal of record type should still find
// the field via the Declaration branch (not the new Package branch).
#[test]
fn test_resolve_path_chain_record_field_regression() {
    // A package that exports a record type with two fields.
    let pkg_src = concat!(
        "package types_pkg is\n",
        "  type point_t is record\n",
        "    x : integer;\n",
        "    y : integer;\n",
        "  end record;\n",
        "end package;\n",
    );
    // An architecture that imports the type and declares a signal of that type.
    let top_src = concat!(
        "use work.types_pkg.all;\n",
        "architecture rtl of top is\n",
        "  signal pt : point_t;\n",
        "begin\n",
        "end architecture;\n",
    );
    let (map, uris) = setup_workspace(vec![("types_pkg.vhd", pkg_src), ("top.vhd", top_src)]);
    let top_uri = &uris[1];

    // Cursor on 'pt.x' — position inside the architecture body.
    let pos = Position { line: 3, character: 0 };

    // pt is a local signal of type point_t; pt.x should resolve to the x field.
    let results = resolve_path_chain(
        &["pt".to_string(), "x".to_string()],
        top_uri,
        &map,
        &pos,
    );

    assert_eq!(results.len(), 1, "expected one field declaration, got {}", results.len());
    let name = results[0].item.name().to_lowercase();
    assert_eq!(name, "x", "expected field 'x', got '{}'", name);
}

// -----------------------------------------------------------------------
// goto-definition on a qualified function call must land on the body
// -----------------------------------------------------------------------

// pkg.calc where calc has both a declaration (package header) and a body
// (package body) — apply_definition_priority must pick the body.
#[test]
fn test_resolve_path_chain_qualified_fn_returns_body_for_definition() {
    use crate::backend::features::goto::apply_definition_priority;
    use crate::backend::features::lookup::ResolvedItem;

    let pkg_src = concat!(
        "package my_pkg is\n",
        "  function calc return boolean;\n",
        "end package;\n",
    );
    let body_src = concat!(
        "package body my_pkg is\n",
        "  function calc return boolean is\n",
        "  begin\n",
        "    return true;\n",
        "  end function;\n",
        "end package body;\n",
    );
    let top_src = concat!(
        "use work.my_pkg.all;\n",
        "architecture rtl of top is\n",
        "begin\n",
        "end architecture;\n",
    );
    let (map, uris) = setup_workspace(vec![
        ("my_pkg.vhd", pkg_src),
        ("my_pkg_body.vhd", body_src),
        ("top.vhd", top_src),
    ]);
    let top_uri = &uris[2];
    let pos = Position { line: 2, character: 0 };

    let results = resolve_path_chain(
        &["my_pkg".to_string(), "calc".to_string()],
        top_uri,
        &map,
        &pos,
    );

    // Both declaration and body should be returned by the chain.
    let has_body = results.iter().any(|r| {
        matches!(&r.item, ResolvedItem::Declaration(d) if matches!(d.decl_type, crate::analysis::DeclType::Function))
    });
    assert!(has_body, "expected a Function body in chain results, got {:?}", results);

    // apply_definition_priority must select the body.
    let locs = apply_definition_priority(results, top_uri);
    assert_eq!(locs.len(), 1, "expected exactly one location, got {}", locs.len());
    assert!(
        locs[0].uri.path().contains("my_pkg_body"),
        "expected body file, got: {}",
        locs[0].uri.path()
    );
}
