use crate::backend::AnalysisMap;
use crate::backend::features::hover::{format_hover_result, resolve_hover};
use crate::backend::test_utils::parse_text;
use tower_lsp::lsp_types::{Position, Url};

fn setup(files: Vec<(&str, &str)>) -> (AnalysisMap, Vec<Url>) {
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

#[test]
fn hover_on_deep_instantiated_entity_shows_real_signature() {
    let target_src = "entity uart_rx is\n  port (clk : in std_logic);\nend entity;\n";
    let current_src =
        "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n";
    let (map, uris) = setup(vec![("uart_rx.vhd", target_src), ("top.vhd", current_src)]);
    let current_uri = &uris[1];

    let tree = parse_text(current_src);
    // Cursor inside the "uart_rx" token (spans chars 20..27 on this line —
    // the "u0:" label is shorter than Task 2's "u_uart:" fixture, so the
    // offset differs from that test).
    let pos = Position {
        line: 3,
        character: 23,
    };

    let results = resolve_hover(&map, current_uri, current_src, tree.root_node(), pos);
    assert_eq!(results.len(), 1, "expected exactly one hover candidate");
    let md = format_hover_result(&results[0]);
    assert!(md.contains("uart_rx"), "got: {md}");
    assert!(
        md.contains("clk"),
        "expected the real port to show up, got: {md}"
    );
    assert!(
        !md.contains("void"),
        "must not degrade to the bare-symbol format, got: {md}"
    );
}

#[test]
fn hover_on_shallow_instantiated_entity_still_points_at_definition_uri() {
    let (map, uris) = setup(vec![
        (
            "uart_rx.vhd",
            "entity uart_rx is\n  port (clk : in std_logic);\nend entity;\n",
        ),
        (
            "top.vhd",
            "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n",
        ),
    ]);
    // Force the target file back to a shallow, symbols-only analysis, as it
    // would be before anything JIT-parses it.
    let mut map = map;
    let target_uri = uris[0].clone();
    let current_uri = uris[1].clone();
    let mut shallow = crate::analysis::Analysis::new();
    shallow.library = "work".to_string();
    shallow.parse_level = crate::analysis::ParseLevel::Shallow;
    shallow.symbols.insert(
        "uart_rx".to_string(),
        crate::analysis::Symbol {
            name: "uart_rx".to_string(),
            kind: crate::analysis::OxideSymbolKind::Entity,
            detail: Some("Entity".to_string()),
            range: tower_lsp::lsp_types::Range::default(),
            children: Vec::new(),
        },
    );
    map.insert(target_uri.clone(), shallow);

    let current_src =
        "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n";
    let tree = parse_text(current_src);
    // Cursor inside the "uart_rx" token (chars 20..27 on this line).
    let pos = Position {
        line: 3,
        character: 23,
    };

    let results = resolve_hover(&map, &current_uri, current_src, tree.root_node(), pos);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].definition_uri, Some(target_uri));
}

#[test]
fn hover_on_ordinary_dotted_access_is_unaffected() {
    // A record field access must still go through the existing chain path —
    // the new instance-aware check must not intercept it.
    let src = "architecture rtl of top is\n  type rec_t is record\n    field1 : integer;\n  end record;\n  signal my_rec : rec_t;\nbegin\n  my_rec.field1 <= 1;\nend architecture;\n";
    let (map, uris) = setup(vec![("top.vhd", src)]);
    let tree = parse_text(src);
    let pos = Position {
        line: 6,
        character: 9,
    };

    let results = resolve_hover(&map, &uris[0], src, tree.root_node(), pos);
    // Whatever the existing chain resolver does here (may be empty, may find
    // the field) — the point is it's unchanged by this feature. Just assert
    // we didn't crash and didn't silently swallow it into an entity hover.
    for res in &results {
        let md = format_hover_result(res);
        assert!(
            !md.contains("entity uart_rx"),
            "must not treat a record access as an instantiation, got: {md}"
        );
    }
}
