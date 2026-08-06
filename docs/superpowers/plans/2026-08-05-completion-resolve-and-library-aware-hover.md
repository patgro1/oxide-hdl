# Completion Resolve & Library-Aware Hover/Goto Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close two 0.7.1-deferred gaps in direct-instantiation support: entity completions missing their generic/port-map snippet on first touch, and hover/goto misresolving the instantiated entity's name.

**Architecture:** Feature 1 adds a `completionItem/resolve` handler that JIT-deep-parses the target entity's file lazily, only for the item the client highlights, and fills in the snippet the same way the eager path already does. Feature 2 adds a `unit_range` to `Instance` so the parser can pinpoint the unit-name token, a `find_instance_at` lookup to find the instantiation under the cursor, and wires that into `resolve_hover`/`goto_definition` ahead of the generic dotted-name chain resolver that currently mishandles this position.

**Tech Stack:** Rust, tower-lsp 0.20, tree-sitter, serde_json.

## Global Constraints

- No new parsing path: reuse `workspace::ensure_fully_parsed` for JIT upgrades (already used by `hover()`), and `units::resolve_entity_uris` for entity resolution (already used by `ensure_dependencies_loaded`).
- Every new function that doesn't need a live `tower_lsp::Client` must be pure and unit-tested directly — mirrors this codebase's existing pattern of thin async trait methods delegating to pure, tested functions (`resolve_hover`, `apply_definition_priority`, etc.).
- No unrelated refactoring. `format_component_hover` (Declaration-based, used for VHDL `component` declarations) is untouched — it operates on a different data shape (`Declaration.parameters`) than the Symbol-based entity-hover path added here and shares no code with it.
- Ordering: the new instance-aware checks in `resolve_hover` and `goto_definition` must run **before** the existing `get_qualified_chain_at_pos`/chain-resolution attempt, since that path currently returns a non-empty but wrong result for this exact cursor position.

---

## Task 1: `completionItem/resolve` fills in the entity snippet lazily

**Files:**
- Modify: `src/backend/features/completion/mod.rs:2413-2454` (the `LibraryUnits` match arm)
- Modify: `src/backend/mod.rs:340-352` (capabilities), and add a new handler after `completion()` (currently ends at line 919)
- Test: `src/backend/features/completion/tests.rs`

**Interfaces:**
- Produces: `pub fn decode_entity_snippet_data(item: &CompletionItem) -> Option<(Url, String)>` and `pub fn apply_entity_snippet(item: CompletionItem, uri: &Url, name: &str, analysis_map: &AnalysisMap) -> CompletionItem`, both in `completion/mod.rs`, both pure.

- [ ] **Step 1: Write the failing tests for the pure helpers**

Add to `src/backend/features/completion/tests.rs`, right after `test_library_units_completion_lists_entities_of_that_library` (which already builds the fixtures this reuses — `shallow_lib`, `deep_entity_analysis`):

```rust
#[test]
fn test_library_units_shallow_item_carries_resolve_data() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();
    let entity_uri = Url::parse("file:///a.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(entity_uri.clone(), shallow_lib("rtl_lib", &["uart_tx"]));
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();
    let ctx = get_completion_context(text, root, pos);
    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);

    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("expected uart_tx in the list");
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
    let data = item.data.clone().expect("shallow item should carry resolve data");
    assert_eq!(data["uri"], entity_uri.to_string());
    assert_eq!(data["name"], "uart_tx");
}

#[test]
fn test_library_units_deep_item_carries_no_resolve_data() {
    use crate::backend::AnalysisMap;
    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();
    let ctx = get_completion_context(text, root, pos);
    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);

    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("expected uart_tx in the list");
    assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    assert!(item.data.is_none(), "deep item needs no resolve data");
}

#[test]
fn test_decode_entity_snippet_data_roundtrip() {
    let uri = Url::parse("file:///a.vhd").unwrap();
    let item = CompletionItem {
        data: Some(serde_json::json!({"uri": uri.to_string(), "name": "uart_tx"})),
        ..Default::default()
    };
    let (decoded_uri, decoded_name) =
        decode_entity_snippet_data(&item).expect("expected valid data to decode");
    assert_eq!(decoded_uri, uri);
    assert_eq!(decoded_name, "uart_tx");
}

#[test]
fn test_decode_entity_snippet_data_missing_returns_none() {
    let item = CompletionItem::default();
    assert!(decode_entity_snippet_data(&item).is_none());
}

#[test]
fn test_apply_entity_snippet_fills_in_snippet_once_deep() {
    use crate::backend::AnalysisMap;
    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let entity_uri = Url::parse("file:///a.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        entity_uri.clone(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );

    let item = CompletionItem {
        label: "uart_tx".to_string(),
        insert_text: Some("uart_tx".to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        ..Default::default()
    };

    let resolved = apply_entity_snippet(item, &entity_uri, "uart_tx", &map);
    assert_eq!(resolved.insert_text_format, Some(InsertTextFormat::SNIPPET));
    assert!(
        resolved.insert_text.unwrap().contains("port map"),
        "expected the real port-map snippet"
    );
}

#[test]
fn test_apply_entity_snippet_still_shallow_returns_unchanged() {
    use crate::backend::AnalysisMap;
    let entity_uri = Url::parse("file:///a.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(entity_uri.clone(), shallow_lib("rtl_lib", &["uart_tx"]));

    let item = CompletionItem {
        label: "uart_tx".to_string(),
        insert_text: Some("uart_tx".to_string()),
        insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
        data: Some(serde_json::json!({"uri": entity_uri.to_string(), "name": "uart_tx"})),
        ..Default::default()
    };

    let resolved = apply_entity_snippet(item, &entity_uri, "uart_tx", &map);
    assert_eq!(resolved.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test backend::features::completion::tests::test_library_units_shallow_item_carries_resolve_data backend::features::completion::tests::test_decode_entity_snippet_data_roundtrip backend::features::completion::tests::test_apply_entity_snippet_fills_in_snippet_once_deep`
Expected: FAIL to compile — `decode_entity_snippet_data`/`apply_entity_snippet` don't exist yet, and `item.data` is never set.

- [ ] **Step 3: Implement the pure helpers**

Add near `generate_instantiation_snippet` in `src/backend/features/completion/mod.rs` (top of file already imports `CompletionItem`, `InsertTextFormat`, `Url` — confirm these three are in the existing `use tower_lsp::lsp_types::{...}` block at the top of the file; add any missing ones there):

```rust
#[derive(serde::Deserialize)]
struct EntitySnippetData {
    uri: String,
    name: String,
}

/// Decodes the `(uri, name)` pair a shallow `LibraryUnits` completion item
/// stashed in its `data` field, so `completionItem/resolve` knows which file
/// to JIT-parse and which entity to look up once it's parsed.
///
/// Returns `None` for items with no data (deep items never carry any) or
/// malformed data — callers treat both as "nothing to resolve".
pub fn decode_entity_snippet_data(item: &CompletionItem) -> Option<(Url, String)> {
    let data = item.data.clone()?;
    let payload: EntitySnippetData = serde_json::from_value(data).ok()?;
    let uri = Url::parse(&payload.uri).ok()?;
    Some((uri, payload.name))
}

/// Fills in `item`'s real generic/port-map snippet from `uri`'s entity scope
/// tree, if it's there. Returns `item` unchanged when the entity still isn't
/// deep-parsed (caller upgrades it first) or is gone entirely.
pub fn apply_entity_snippet(
    mut item: CompletionItem,
    uri: &Url,
    name: &str,
    analysis_map: &AnalysisMap,
) -> CompletionItem {
    if let Some(tree) = analysis_map
        .get(uri)
        .and_then(|a| a.entity_scope_trees.get(name))
    {
        item.insert_text = Some(generate_instantiation_snippet(name, tree));
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
    }
    item
}
```

- [ ] **Step 4: Wire `data` into the `LibraryUnits` arm**

In `src/backend/features/completion/mod.rs`, replace the `None` branch of the `match deep_tree` in the `LibraryUnits` arm (currently `None => (name.clone(), InsertTextFormat::PLAIN_TEXT),`) and the subsequent `CompletionItem` construction:

```rust
                    let (insert_text, format, data) = match deep_tree {
                        Some(tree) => (
                            generate_instantiation_snippet(&name, tree),
                            InsertTextFormat::SNIPPET,
                            None,
                        ),
                        None => (
                            name.clone(),
                            InsertTextFormat::PLAIN_TEXT,
                            Some(serde_json::json!({
                                "uri": entity_uri.to_string(),
                                "name": name.clone(),
                            })),
                        ),
                    };

                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::CLASS),
                        label: name.clone(),
                        detail: Some(format!("entity in {}", target)),
                        filter_text: Some(name.clone()),
                        insert_text: Some(insert_text),
                        insert_text_format: Some(format),
                        data,
                        ..Default::default()
                    });
```

- [ ] **Step 5: Run the completion tests to verify they pass**

Run: `cargo test backend::features::completion::tests::test_library_units_shallow_item_carries_resolve_data backend::features::completion::tests::test_library_units_deep_item_carries_no_resolve_data backend::features::completion::tests::test_decode_entity_snippet_data_roundtrip backend::features::completion::tests::test_decode_entity_snippet_data_missing_returns_none backend::features::completion::tests::test_apply_entity_snippet_fills_in_snippet_once_deep backend::features::completion::tests::test_apply_entity_snippet_still_shallow_returns_unchanged`
Expected: PASS (all 6).

- [ ] **Step 6: Flip the capability flag and add the `completion_resolve` handler**

In `src/backend/mod.rs`, change line 341 from `resolve_provider: Some(false),` to `resolve_provider: Some(true),` inside the `completion_provider: Some(CompletionOptions { ... })` block (the OTHER `resolve_provider: Some(false)` at line 375 is `CodeActionOptions` — leave that one alone).

Add `CompletionItem` to the existing `use tower_lsp::lsp_types::{...}` block (line 28-39) alongside `CompletionList, CompletionOptions, CompletionParams, CompletionResponse,`.

Add this handler directly after `completion()` (which currently ends at line 919 with `}`), before `prepare_rename`:

```rust
    /// Handles `completionItem/resolve`: fills in the real generic/port-map
    /// snippet for a `LibraryUnits` completion item whose target entity
    /// wasn't deep-parsed yet when the list was built.
    ///
    /// Items with no `data` (already resolved, or from any other completion
    /// path) pass through unchanged — this is a no-op for everything except
    /// the specific shallow-entity case Task 1 of the 0.7.1 plan closes.
    async fn completion_resolve(&self, item: CompletionItem) -> Result<CompletionItem> {
        let Some((uri, name)) = features::completion::decode_entity_snippet_data(&item) else {
            return Ok(item);
        };

        let lib_matcher = {
            let config_guard = self.config.read().await;
            crate::config::LibraryMatcher::from_config(
                &config_guard.clone().unwrap_or_else(OxideConfig::default),
            )
        };
        workspace::ensure_fully_parsed(
            &self.client,
            &self.analysis_map,
            &self.parser,
            &uri,
            &lib_matcher,
        )
        .await;

        let map = self.analysis_map.read().await;
        Ok(features::completion::apply_entity_snippet(item, &uri, &name, &map))
    }
```

- [ ] **Step 7: Build and run the full test suite**

Run: `cargo build && cargo test`
Expected: builds clean, all tests pass (this also exercises anything relying on `CompletionOptions`/`CompletionItem` construction elsewhere still compiling).

- [ ] **Step 8: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs src/backend/mod.rs
git commit -m "feat: resolve entity snippets lazily via completionItem/resolve

Shallow-indexed entities after \`entity lib.\` now carry resolve data instead
of staying a bare name forever. The client's completionItem/resolve call
JIT-deep-parses the target file (reusing workspace::ensure_fully_parsed) and
fills in the real generic/port-map snippet, so the gap only ever shows up
for the single highlighted item, not the whole list."
```

---

## Task 2: `Instance.unit_range` — pinpoint the unit-name token

**Files:**
- Modify: `src/analysis/types.rs` (the `Instance` struct, `~line 445-465`)
- Modify: `src/analysis/builders.rs:1224-1288` (`create_instance_from_node`)
- Modify: `src/backend/units.rs` (test helper `inst()`, `~line 171`)
- Modify: `src/backend/features/symbol/tests.rs` (test helper `instance()`, `~line 5`)
- Test: `src/analysis/tests/builders_tests.rs`

**Interfaces:**
- Produces: `Instance.unit_range: Option<Range>` — the span of just the identifier naming the entity/component/configuration, `None` when there's no identifiable name token. Every other task in this plan that touches `Instance` reads this field.

- [ ] **Step 1: Write the failing test**

Add to `src/analysis/tests/builders_tests.rs`, right after `test_instantiation_entity_work` (~line 864):

```rust
#[test]
fn test_instantiation_unit_range_points_at_name_token_only() {
    let code = "\narchitecture rtl of test is\nbegin\n    u_uart: entity work.uart_rx\n        port map (\n            clk => clk\n        );\nend architecture;\n";
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let inst = &analysis.scope_trees[0].instantiations[0];
    let range = inst
        .unit_range
        .expect("expected a unit_range for `entity work.uart_rx`");
    assert_eq!(range.start.line, 3);
    assert_eq!(range.start.character, 24);
    assert_eq!(range.end.line, 3);
    assert_eq!(range.end.character, 31);

    // Sanity check: that span is exactly the text "uart_rx", not "work" or
    // the whole `work.uart_rx`.
    let lines: Vec<&str> = code.lines().collect();
    let line = lines[range.start.line as usize];
    let slice = &line[range.start.character as usize..range.end.character as usize];
    assert_eq!(slice, "uart_rx");
}

#[test]
fn test_instantiation_unit_range_plain_component_form() {
    let code = "\narchitecture rtl of test is\nbegin\n    u_fifo: fifo_comp\n        port map (\n            clk => clk\n        );\nend architecture;\n";
    let tree = parse_text(code);
    let root = tree.root_node();
    let analysis = extract_document_symbols(code, root);

    let inst = &analysis.scope_trees[0].instantiations[0];
    let range = inst
        .unit_range
        .expect("expected a unit_range for the plain component form");

    let lines: Vec<&str> = code.lines().collect();
    let line = lines[range.start.line as usize];
    let slice = &line[range.start.character as usize..range.end.character as usize];
    assert_eq!(slice, "fifo_comp");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test analysis::tests::builders_tests::test_instantiation_unit_range_points_at_name_token_only analysis::tests::builders_tests::test_instantiation_unit_range_plain_component_form`
Expected: FAIL to compile — `Instance` has no field `unit_range` yet.

- [ ] **Step 3: Add the field to `Instance`**

In `src/analysis/types.rs`, add after `pub selection_range: Range,` (the last field in the `Instance` struct, ~line 464):

```rust
    /// Range of the unit-name token only — the identifier naming the
    /// entity/component/configuration, e.g. `uart_rx` in `entity work.uart_rx`.
    /// Distinct from `range` (the whole statement) and `selection_range` (the
    /// label). `None` when no identifiable name token exists.
    pub unit_range: Option<Range>,
```

- [ ] **Step 4: Populate it in `create_instance_from_node`**

In `src/analysis/builders.rs`, replace the body of `create_instance_from_node` (lines 1224-1288) with:

```rust
fn create_instance_from_node(node: Node, text: &str) -> Instance {
    let mut label = "".to_string();
    let mut selection_range = node_to_range(node);
    if let Some(label_decl) = find_child(node, "label_declaration")
        && let Some(label_node) = find_child(label_decl, "label")
    {
        label = text[label_node.byte_range()].to_string();
        selection_range = node_to_range(label_node);
    }
    // The plain form (`u0: cpu`) has no `instantiated_unit` node — the `component`
    // field hangs directly off the statement. Every other form nests one.
    let unit = find_child(node, "instantiated_unit").unwrap_or(node);

    let (unit_kind, name_node) = if let Some(n) = unit.child_by_field_name("entity") {
        (InstantiatedUnitKind::Entity, Some(n))
    } else if let Some(n) = unit.child_by_field_name("configuration") {
        (InstantiatedUnitKind::Configuration, Some(n))
    } else if let Some(n) = unit.child_by_field_name("component") {
        (InstantiatedUnitKind::Component, Some(n))
    } else {
        (InstantiatedUnitKind::Component, find_child(unit, "name"))
    };

    // `work` is a distinct grammar node (`library_namespace`); any other library
    // is just the first identifier of a dotted `name`, with the unit in a `selection`.
    let mut library = unit
        .child_by_field_name("library")
        .map(|n| text[n.byte_range()].to_string());

    let mut component = "".to_string();
    let mut unit_range = None;
    if let Some(name) = name_node {
        let selections: Vec<Node> = name
            .children(&mut name.walk())
            .filter(|c| c.kind() == "selection")
            .collect();

        if let Some(last) = selections.last() {
            // Dotted name: unit is the final segment, library the leading identifier.
            if let Some(iden) = find_child(*last, "identifier") {
                component = text[iden.byte_range()].to_string();
                unit_range = Some(node_to_range(iden));
            }
            if library.is_none()
                && let Some(iden) = find_child(name, "identifier")
            {
                library = Some(text[iden.byte_range()].to_string());
            }
        } else if let Some(iden) = find_child(name, "identifier") {
            component = text[iden.byte_range()].to_string();
            unit_range = Some(node_to_range(iden));
        }
    }

    let architecture = unit
        .child_by_field_name("architecture")
        .map(|n| text[n.byte_range()].to_string());

    Instance {
        label,
        component,
        library,
        architecture,
        unit_kind,
        range: node_to_range(node),
        selection_range,
        unit_range,
    }
}
```

- [ ] **Step 5: Fix the two test-fixture construction sites**

In `src/backend/units.rs`, in the `inst()` test helper (~line 171-180), add `unit_range: None,` after `selection_range: Range::default(),`.

In `src/backend/features/symbol/tests.rs`, in the `instance()` test helper (~line 5-14), add `unit_range: None,` after `selection_range: Range::default(),`.

- [ ] **Step 6: Run the tests to verify they pass, and the crate still builds**

Run: `cargo build && cargo test`
Expected: builds clean (both fixture sites fixed), the two new tests pass, and every pre-existing instantiation test (`test_instantiation_simple_component`, `test_instantiation_entity_work`, `test_instantiation_entity_with_architecture`, etc.) still passes unchanged.

- [ ] **Step 7: Commit**

```bash
git add src/analysis/types.rs src/analysis/builders.rs src/analysis/tests/builders_tests.rs src/backend/units.rs src/backend/features/symbol/tests.rs
git commit -m "feat: capture the unit-name token range on Instance

Adds unit_range, populated wherever the parser already extracts the
component/entity name from an identifier node. Foundation for resolving
hover/goto on the instantiated unit's name instead of the whole statement."
```

---

## Task 3: `find_instance_at` — locate the instantiation under the cursor

**Files:**
- Modify: `src/analysis/scope_tree.rs`
- Test: `src/analysis/scope_tree.rs` (inline `#[cfg(test)] mod tests`, ~line 354)

**Interfaces:**
- Consumes: `Instance.unit_range` (Task 2), `ScopeTree.collect_all_instantiations()` (existing), `position_in_range` (existing, imported at top of this file).
- Produces: `pub fn find_instance_at(scope_trees: &[ScopeTree], pos: Position) -> Option<&Instance>`. Tasks 4 and 5 both call this.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `src/analysis/scope_tree.rs` (after `is_attr_applied_no_attr`, ~line 406), extending the existing `use super::*;` imports already in scope:

```rust
    fn make_instance(component: &str, unit_range: Option<Range>) -> Instance {
        Instance {
            label: "u0".to_string(),
            component: component.to_string(),
            library: Some("work".to_string()),
            architecture: None,
            unit_kind: crate::analysis::InstantiatedUnitKind::Entity,
            range: Range::default(),
            selection_range: Range::default(),
            unit_range,
        }
    }

    fn range_at(line: u32, start: u32, end: u32) -> Range {
        Range {
            start: Position {
                line,
                character: start,
            },
            end: Position {
                line,
                character: end,
            },
        }
    }

    #[test]
    fn find_instance_at_hits_when_pos_inside_unit_range() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope.instantiations.push(make_instance("uart_rx", Some(range_at(3, 24, 31))));

        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 3,
                character: 27,
            },
        );
        assert_eq!(found.map(|i| i.component.as_str()), Some("uart_rx"));
    }

    #[test]
    fn find_instance_at_misses_just_outside_unit_range() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope.instantiations.push(make_instance("uart_rx", Some(range_at(3, 24, 31))));

        // position_in_range treats the range's end as inclusive, so the true
        // boundary is one character past 31 — use 32 to land unambiguously
        // outside on either interpretation.
        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 3,
                character: 32,
            },
        );
        assert!(found.is_none());
    }

    #[test]
    fn find_instance_at_ignores_instantiations_with_no_unit_range() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope.instantiations.push(make_instance("uart_rx", None));

        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(found.is_none());
    }

    #[test]
    fn find_instance_at_picks_the_right_one_among_several() {
        let mut scope = make_test_scope("mark_debug", &[]);
        scope
            .instantiations
            .push(make_instance("uart_rx", Some(range_at(3, 24, 31))));
        scope
            .instantiations
            .push(make_instance("cpu", Some(range_at(5, 10, 13))));

        let found = find_instance_at(
            std::slice::from_ref(&scope),
            Position {
                line: 5,
                character: 11,
            },
        );
        assert_eq!(found.map(|i| i.component.as_str()), Some("cpu"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test analysis::scope_tree::tests::find_instance_at_hits_when_pos_inside_unit_range`
Expected: FAIL to compile — `find_instance_at` doesn't exist, and `make_test_scope` doesn't set `instantiations` to anything mutable yet (it's already a field on the struct literal, so this compiles once the function exists — only the missing function blocks it).

- [ ] **Step 3: Implement `find_instance_at`**

Add to `src/analysis/scope_tree.rs`, after `collect_all_instantiations` (inside `impl ScopeTree`, ~line 306) — as a free function at module level, right after the `impl ScopeTree` block closes (~line 352, before `#[cfg(test)]`):

```rust
/// Finds the instantiation whose unit-name token contains `pos`, searching
/// every scope tree in `scope_trees` and their nested generate/block children
/// (via `collect_all_instantiations`).
///
/// Used to detect when the cursor sits on the entity/component name inside
/// `label: entity lib.name`, so hover and goto-definition can resolve it as
/// an instantiation instead of falling through to the generic dotted-name
/// chain resolver, which has no concept of instantiation syntax.
pub fn find_instance_at(scope_trees: &[ScopeTree], pos: Position) -> Option<&Instance> {
    for tree in scope_trees {
        for inst in tree.collect_all_instantiations() {
            if let Some(range) = inst.unit_range
                && position_in_range(pos, range)
            {
                return Some(inst);
            }
        }
    }
    None
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test analysis::scope_tree::tests`
Expected: PASS (all, including the 4 new ones and the 5 pre-existing `is_attr_applied_*` ones).

- [ ] **Step 5: Commit**

```bash
git add src/analysis/scope_tree.rs
git commit -m "feat: add find_instance_at to locate the instantiation under the cursor

Scans unit_range across every scope tree (including nested generate/block
children). Hover and goto will use this to detect the unit-name position
before falling back to generic dotted-name resolution."
```

---

## Task 4: Hover resolves the instantiated entity's real signature

**Files:**
- Modify: `src/backend/features/hover/mod.rs`
- Modify: `src/backend/features/hover/tests.rs` (currently an empty, undeclared file)

**Interfaces:**
- Consumes: `find_instance_at` (Task 3), `units::resolve_entity_uris` (existing).
- Produces: `format_entity_hover(sym: &Symbol) -> String`, `resolve_instantiated_entity_hover(analysis_map, current_uri, pos) -> Option<HoverResolution>`, both in `hover/mod.rs`. `resolve_hover` now calls the latter first.

- [ ] **Step 1: Wire up the orphaned test file**

`src/backend/features/hover/tests.rs` exists but is empty and undeclared (0 lines) — the same state `symbol/tests.rs` was in before it was wired up in the most recent commit. Add to the end of `src/backend/features/hover/mod.rs` (after the last function, `format_component_hover`, currently ending at line 619):

```rust

#[cfg(test)]
mod tests;
```

- [ ] **Step 2: Write the failing tests**

Write `src/backend/features/hover/tests.rs`:

```rust
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
    let current_src = "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n";
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
    assert!(md.contains("clk"), "expected the real port to show up, got: {md}");
    assert!(!md.contains("void"), "must not degrade to the bare-symbol format, got: {md}");
}

#[test]
fn hover_on_shallow_instantiated_entity_still_points_at_definition_uri() {
    let (map, uris) = setup(vec![
        ("uart_rx.vhd", "entity uart_rx is\n  port (clk : in std_logic);\nend entity;\n"),
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

    let current_src = "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n";
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
        assert!(!md.contains("entity uart_rx"), "must not treat a record access as an instantiation, got: {md}");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test backend::features::hover::tests`
Expected: FAIL — `hover_on_deep_instantiated_entity_shows_real_signature` and `hover_on_shallow_instantiated_entity_still_points_at_definition_uri` fail their assertions (today's behavior is the `void`/chain-resolution bug); `hover_on_ordinary_dotted_access_is_unaffected` should already pass (nothing to change there) — that's expected and fine at this stage.

- [ ] **Step 4: Extract the shared Generics/Ports body and add `format_entity_hover`**

In `src/backend/features/hover/mod.rs`, replace `format_instantiation_hover` (currently lines 122-167) with:

```rust
pub fn format_instantiation_hover(instance_name: &str, definition: &Symbol) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "**{}** (Instance of `{}`)\n\n",
        instance_name, definition.name
    ));
    md.push_str("```vhdl\n");
    md.push_str(&format!("entity {} is\n", definition.name));
    md.push_str(&format_entity_interface_body(&definition.children));
    md.push_str("end entity;\n");
    md.push_str("\n```");
    md
}

/// Formats a rich hover tooltip for the entity name inside a direct
/// instantiation (`label: entity lib.name`), e.g. hovering `uart_rx` itself
/// rather than the label `u0`.
///
/// # Arguments
/// * `sym` - An `Entity`-kind symbol with its generics/ports as `children`
///   (built by `build_entity_symbol`).
pub fn format_entity_hover(sym: &Symbol) -> String {
    let mut md = String::new();
    md.push_str(&format!("**{}**\n\n", sym.name));
    md.push_str("```vhdl\n");
    md.push_str(&format!("entity {} is\n", sym.name));
    md.push_str(&format_entity_interface_body(&sym.children));
    md.push_str("end entity;\n");
    md.push_str("\n```");
    md
}

/// Renders the `generics (...)`/`ports (...)` body shared by
/// `format_instantiation_hover` and `format_entity_hover` — both work from a
/// flat list of `Generic`/`Constant`/`Port`-kind child symbols.
fn format_entity_interface_body(children: &[Symbol]) -> String {
    let mut md = String::new();
    let generics: Vec<&Symbol> = children
        .iter()
        .filter(|c| c.kind == OxideSymbolKind::Generic || c.kind == OxideSymbolKind::Constant)
        .collect();
    if !generics.is_empty() {
        md.push_str("generics (\n");
        for (i, g) in generics.iter().enumerate() {
            let type_info = g.detail.as_deref().unwrap_or("?");
            let sep = if i < generics.len() - 1 { ";" } else { "" };
            md.push_str(&format!("    {} : {}{}\n", g.name, type_info, sep));
        }
        md.push_str(");\n");
    }
    let ports: Vec<&Symbol> = children
        .iter()
        .filter(|c| c.kind == OxideSymbolKind::Port)
        .collect();
    if !ports.is_empty() {
        md.push_str("ports (\n");
        for (i, p) in ports.iter().enumerate() {
            let type_info = p.detail.as_deref().unwrap_or("?");
            let sep = if i < ports.len() - 1 { ";" } else { "" };
            md.push_str(&format!("    {} : {}{}\n", p.name, type_info, sep));
        }
        md.push_str(");\n");
    }
    md
}
```

- [ ] **Step 5: Add the `Entity` dispatch arm**

In `src/backend/features/hover/mod.rs`, in `format_hover_result` (~line 33-42), change:

```rust
        ResolvedItem::Symbol(s) => match s.kind {
            OxideSymbolKind::ComponentInstantiation => format_instantiation_hover(&s.name, s),
            OxideSymbolKind::Function | OxideSymbolKind::Process => format_function_hover(s),
            _ => format_basic(s),
        },
```

to:

```rust
        ResolvedItem::Symbol(s) => match s.kind {
            OxideSymbolKind::ComponentInstantiation => format_instantiation_hover(&s.name, s),
            OxideSymbolKind::Function | OxideSymbolKind::Process => format_function_hover(s),
            OxideSymbolKind::Entity => format_entity_hover(s),
            _ => format_basic(s),
        },
```

- [ ] **Step 6: Add the entity-symbol builder and the instance-aware resolver**

Add to `src/backend/features/hover/mod.rs`, right before `resolve_hover` (~line 260):

```rust
/// Converts a generic/port `Declaration` from a deep-parsed entity's scope
/// tree into a child `Symbol`, with its direction/type/default pre-formatted
/// into `detail` — the shape `format_entity_interface_body` expects.
fn declaration_to_child_symbol(decl: &Declaration) -> Symbol {
    let type_str = format_type_info(&decl.type_info);
    let default_part = decl
        .default_value
        .as_ref()
        .map(|v| format!(" := {}", v))
        .unwrap_or_default();
    let dir_str = if let DeclType::Port(d) = decl.decl_type {
        format!("{} ", d)
    } else {
        String::new()
    };
    let kind = if matches!(decl.decl_type, DeclType::Generic) {
        OxideSymbolKind::Generic
    } else {
        OxideSymbolKind::Port
    };
    Symbol {
        name: decl.name.clone(),
        kind,
        detail: Some(format!("{}{}{}", dir_str, type_str, default_part)),
        range: decl.range,
        children: Vec::new(),
    }
}

/// Builds an `Entity`-kind `Symbol` for `name` from a deep-parsed entity's
/// `ScopeTree`, with its generics/ports copied in as children so
/// `format_entity_hover` can render the real interface.
fn build_entity_symbol(name: &str, tree: &crate::analysis::ScopeTree) -> Symbol {
    let children = tree
        .declarations
        .iter()
        .filter(|d| matches!(d.decl_type, DeclType::Generic | DeclType::Port(_)))
        .map(declaration_to_child_symbol)
        .collect();
    Symbol {
        name: name.to_string(),
        kind: OxideSymbolKind::Entity,
        detail: None,
        range: tree.range,
        children,
    }
}

/// Resolves hover when the cursor sits on the unit-name of a direct entity
/// instantiation (`label: entity lib.name`), bypassing the generic
/// dotted-name chain resolver, which has no concept of instantiation syntax
/// and would otherwise return a meaningless `entity : void` result.
///
/// When the target entity is still shallow-indexed, returns a minimal result
/// that still carries the correct `definition_uri` — `Backend::hover`'s
/// existing needs-JIT check upgrades it and calls `resolve_hover` again,
/// at which point this function finds the now-deep entity and returns the
/// full signature. No new JIT-parse plumbing needed.
///
/// Returns `None` when the cursor isn't on such a position, or the
/// instantiation's library/name doesn't resolve to a known file — the caller
/// falls through to the existing chain/bare-word resolution unchanged.
fn resolve_instantiated_entity_hover(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    pos: Position,
) -> Option<HoverResolution> {
    let analysis = analysis_map.get(current_uri)?;
    let inst = crate::analysis::find_instance_at(&analysis.scope_trees, pos)?;
    if inst.unit_kind != crate::analysis::InstantiatedUnitKind::Entity {
        return None;
    }
    let target_uri = crate::backend::units::resolve_entity_uris(analysis_map, inst, &analysis.library)
        .into_iter()
        .next()?;
    let target_analysis = analysis_map.get(&target_uri)?;
    let name_lc = inst.component.to_lowercase();
    let sym = match target_analysis.entity_scope_trees.get(&name_lc) {
        Some(tree) => build_entity_symbol(&inst.component, tree),
        None => Symbol {
            name: inst.component.clone(),
            kind: OxideSymbolKind::Entity,
            detail: Some("Entity".to_string()),
            range: tower_lsp::lsp_types::Range::default(),
            children: Vec::new(),
        },
    };
    Some(HoverResolution {
        item: ResolvedItem::Symbol(sym),
        definition_uri: Some(target_uri),
    })
}
```

- [ ] **Step 7: Call it first from `resolve_hover`**

In `src/backend/features/hover/mod.rs`, change the start of `resolve_hover` (~line 260-268) from:

```rust
pub fn resolve_hover(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    text: &str,
    root_node: Node,
    pos: Position,
) -> Vec<HoverResolution> {
    let chain = get_qualified_chain_at_pos(root_node, text, pos);
    if !chain.is_empty() {
```

to:

```rust
pub fn resolve_hover(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    text: &str,
    root_node: Node,
    pos: Position,
) -> Vec<HoverResolution> {
    if let Some(res) = resolve_instantiated_entity_hover(analysis_map, current_uri, pos) {
        return vec![res];
    }

    let chain = get_qualified_chain_at_pos(root_node, text, pos);
    if !chain.is_empty() {
```

- [ ] **Step 8: Add the missing imports**

At the top of `src/backend/features/hover/mod.rs`, extend the existing `crate::{ analysis::{...}, backend::{...} }` import block:

```rust
use crate::{
    analysis::{DeclType, Declaration, OxideSymbolKind, Symbol, TypeInfo},
    backend::{
        AnalysisMap,
        features::lookup::{ResolvedItem, lookup_symbol, resolve_path_chain},
    },
};
```

becomes:

```rust
use crate::{
    analysis::{DeclType, Declaration, OxideSymbolKind, Symbol, TypeInfo},
    backend::{
        AnalysisMap,
        features::lookup::{ResolvedItem, lookup_symbol, resolve_path_chain},
    },
};
```

(No change needed here — `crate::analysis::find_instance_at`, `crate::analysis::InstantiatedUnitKind`, `crate::analysis::ScopeTree`, and `crate::backend::units::resolve_entity_uris` are all called fully-qualified inline in Step 6, so no new `use` lines are required. Leave this block as-is.)

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test backend::features::hover::tests`
Expected: PASS (all 3).

- [ ] **Step 10: Run the full suite**

Run: `cargo build && cargo test`
Expected: builds clean, all tests pass — including every pre-existing hover-adjacent test elsewhere (`ComponentInstantiation`/label hover is untouched, since `format_instantiation_hover`'s public signature and output are unchanged).

- [ ] **Step 11: Commit**

```bash
git add src/backend/features/hover/mod.rs src/backend/features/hover/tests.rs
git commit -m "feat: hover on a direct instantiation's entity name shows its real signature

resolve_hover now checks find_instance_at before falling into the generic
dotted-name chain resolver, which had no concept of instantiation syntax and
returned a degenerate 'entity : void' result. Reuses the existing needs-JIT
upgrade path in Backend::hover unchanged — a shallow target just returns a
minimal result with the right definition_uri, and gets the full signature on
the second pass once ensure_fully_parsed has run."
```

---

## Task 5: Goto-definition jumps to the instantiated entity's real declaration

**Files:**
- Modify: `src/backend/features/goto/mod.rs`
- Modify: `src/backend/mod.rs:548-600` (`goto_definition`)
- Test: `src/backend/features/goto/tests.rs`

**Interfaces:**
- Consumes: `find_instance_at` (Task 3), `units::resolve_entity_uris` (existing).
- Produces: `pub fn resolve_instantiated_entity_location(analysis_map, current_uri, pos) -> Option<Location>` in `goto/mod.rs`.

- [ ] **Step 1: Write the failing tests**

Add to `src/backend/features/goto/tests.rs`, after `setup_workspace` (~line 82, wherever its closing brace is) or anywhere after it's defined:

```rust
#[test]
fn goto_instantiated_entity_name_resolves_to_deep_entity_declaration() {
    let files = vec![
        (
            "uart_rx.vhd",
            "entity uart_rx is\n  port (clk : in std_logic);\nend entity;\n",
        ),
        (
            "top.vhd",
            "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n",
        ),
    ];
    let (map, uris) = setup_workspace(files);
    let target_uri = &uris[0];
    let current_uri = &uris[1];

    // Cursor inside the "uart_rx" token (chars 20..27 on this line — the
    // "u0:" label is shorter than Task 2's "u_uart:" fixture).
    let pos = Position {
        line: 3,
        character: 23,
    };

    let loc = crate::backend::features::goto::resolve_instantiated_entity_location(
        &map,
        current_uri,
        pos,
    )
    .expect("expected a resolved location");
    assert_eq!(&loc.uri, target_uri);
}

#[test]
fn goto_instantiated_entity_name_misses_off_the_unit_range() {
    let files = vec![
        (
            "uart_rx.vhd",
            "entity uart_rx is\n  port (clk : in std_logic);\nend entity;\n",
        ),
        (
            "top.vhd",
            "\narchitecture rtl of top is\nbegin\n    u0: entity work.uart_rx\nend architecture;\n",
        ),
    ];
    let (map, uris) = setup_workspace(files);
    let current_uri = &uris[1];

    // Cursor on the label "u0", not the entity name.
    let pos = Position {
        line: 3,
        character: 6,
    };

    let loc = crate::backend::features::goto::resolve_instantiated_entity_location(
        &map,
        current_uri,
        pos,
    );
    assert!(loc.is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test backend::features::goto::tests::goto_instantiated_entity_name_resolves_to_deep_entity_declaration`
Expected: FAIL to compile — `resolve_instantiated_entity_location` doesn't exist yet.

- [ ] **Step 3: Implement the pure resolver**

Add to `src/backend/features/goto/mod.rs`, after `prefer_current_file` (~line 20):

```rust
/// Resolves goto-definition when the cursor sits on the unit-name of a
/// direct entity instantiation (`label: entity lib.name`), bypassing the
/// generic dotted-name chain resolver, which has no concept of instantiation
/// syntax.
///
/// No JIT parse is needed either way: a shallow file's `Analysis.symbols`
/// entry for the entity already points at its name token (from the fast
/// regex scan), and a deep file's `entity_scope_trees` entry points at its
/// full declaration — `file_declares_entity`'s "check both" pattern, reused
/// here for the location itself rather than just existence.
///
/// Returns `None` when the cursor isn't on such a position, or the
/// instantiation's library/name doesn't resolve to a known file, so the
/// caller falls through to the existing chain/bare-word resolution.
pub fn resolve_instantiated_entity_location(
    analysis_map: &AnalysisMap,
    current_uri: &Url,
    pos: Position,
) -> Option<Location> {
    let analysis = analysis_map.get(current_uri)?;
    let inst = crate::analysis::find_instance_at(&analysis.scope_trees, pos)?;
    if inst.unit_kind != crate::analysis::InstantiatedUnitKind::Entity {
        return None;
    }
    let target_uri = crate::backend::units::resolve_entity_uris(analysis_map, inst, &analysis.library)
        .into_iter()
        .next()?;
    let name_lc = inst.component.to_lowercase();
    let target_analysis = analysis_map.get(&target_uri)?;
    let range = target_analysis
        .symbols
        .get(&name_lc)
        .map(|s| s.range)
        .or_else(|| target_analysis.entity_scope_trees.get(&name_lc).map(|t| t.range))?;
    Some(Location {
        uri: target_uri,
        range,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test backend::features::goto::tests::goto_instantiated_entity_name_resolves_to_deep_entity_declaration backend::features::goto::tests::goto_instantiated_entity_name_misses_off_the_unit_range`
Expected: PASS.

- [ ] **Step 5: Wire it into `Backend::goto_definition`**

In `src/backend/mod.rs`, in `goto_definition` (~line 548-569), insert right after the `tree` block and before the `chain` computation:

```rust
        let tree = {
            let mut parser = self.parser.lock().await;
            let lang = unsafe { crate::tree_sitter_vhdl() };
            let _ = parser.set_language(&lang);
            parser.parse(&text, None).unwrap()
        };

        {
            let map = self.analysis_map.read().await;
            if let Some(loc) =
                features::goto::resolve_instantiated_entity_location(&map, &uri, position)
            {
                return Ok(Some(GotoDefinitionResponse::Array(vec![loc])));
            }
        }

        let chain = hover::get_qualified_chain_at_pos(tree.root_node(), &text, position);
```

- [ ] **Step 6: Run the full suite**

Run: `cargo build && cargo test`
Expected: builds clean, all tests pass — including every pre-existing goto test (`test_goto_definition_qualified_pkg_name_no_ambiguity`, etc.), since this new check only ever fires when `find_instance_at` hits, which none of the existing fixtures trigger.

- [ ] **Step 7: Commit**

```bash
git add src/backend/features/goto/mod.rs src/backend/mod.rs src/backend/features/goto/tests.rs
git commit -m "feat: goto-definition jumps to the real entity from a direct instantiation

Same find_instance_at check as hover, ahead of the generic chain resolver.
No JIT parse needed for goto — file_declares_entity's shallow-or-deep lookup
already gives a usable location either way."
```

---

## Manual verification (both features, real editor)

Unit tests cover the pure logic; the two things they cannot cover are the live `completionItem/resolve` round-trip and the two-pass JIT-upgrade hover flow end-to-end (both need a real `tower_lsp::Client`, matching this codebase's existing gap for `hover()`/`ensure_dependencies_loaded` — see `docs/superpowers/specs/2026-07-27-lsp-integration-tests-design.md`, still unimplemented). After all 5 tasks:

1. Open a workspace with a `[libraries]` section and at least one entity that nothing currently instantiates.
2. Type `label: entity <lib>.<partial-name>` and confirm the completion list appears; arrow down to the untouched entity and confirm the popup preview (or the inserted text on accept) shows the real generic/port-map snippet, not a bare name.
3. Hover the entity name (not the label) in an existing `label: entity <lib>.<name>` instantiation and confirm the tooltip shows real generics/ports instead of `entity : void`.
4. Goto-definition (or Ctrl/Cmd-click) on that same entity name and confirm it jumps to the entity's declaration in its own file, not nowhere / not the label.
