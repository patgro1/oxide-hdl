# Direct-Instantiation Completion & Hover Parity

**Date:** 2026-08-05
**Branch:** `feat/direct-instantiation-libraries`
**Scope:** Two independent fixes, both closing gaps in the 0.7.0 direct-instantiation feature that `roadmap.md` already deferred to 0.7.1:

1. `completionItem/resolve` for entity snippets after `entity lib.` (completion parity).
2. Library-aware hover/goto on the instantiated entity's name (library-aware navigation).

They touch disjoint code (`completion/mod.rs` vs `hover/mod.rs` + `backend/mod.rs::goto_definition` + `analysis/{types,builders,scope_tree}.rs`) and ship as two independent changes in one pass.

---

## Motivation

Reproduced live against a running `oxide-hdl` server driven through its real LSP
client (Neovim, via `--remote-expr`/`buf_request_sync`), not just unit tests:

- Completing `entity rtl_lib.f` lists every entity in the library (shallow-name
  fallback already works, from 0.7.0's Task 7), but an entity nobody has
  instantiated yet in any *currently open* document — e.g. `fifo_sync`, before
  anything JIT-deep-parses its file — completes as a bare name with no
  generic/port-map snippet. The gap is real but self-healing: once anything
  triggers a deep parse of that file (opening it, instantiating it elsewhere in
  an open buffer, hovering a reference to it), the entity stays deep-parsed for
  the server's lifetime and the snippet appears from then on. This is exactly
  the first-touch experience new users hit.
- Hovering the entity name inside `label: entity lib.name` (as opposed to the
  label itself) returns a garbled `entity  :  void` instead of the entity's real
  generics/ports, even when the entity **is** already deep-parsed. Root cause:
  `resolve_hover`/`goto_definition` see `lib.name` as a generic dotted-name chain
  (the same code path used for `record.field` or `package.symbol`) and resolve it
  through `get_qualified_chain_at_pos` + `resolve_path_chain`, which has no concept
  of "this identifier is the unit-name position of a direct instantiation." It
  matches *something* by name, but formats it as a bare symbol with no type info.

Both were called out in `roadmap.md`'s "Deferred to 0.7.1" list under different
bullets ("Completion parity between entities and package components" only
covers the immediately-after-label context today, not this one; "Library-aware
go-to-definition and hover" is the second one, unchanged from that description).

---

## Feature 1 — `completionItem/resolve` for entity snippets

### Decisions

| Decision | Choice | Why |
|---|---|---|
| Where the fix lives | `complete_scope`'s `LibraryUnits` arm only (`completion/mod.rs:2413`) | This is the exact path the bug reproduces on. The immediately-after-label path (`generate_entity_completions`) has a *different* gap (missing shallow entities entirely, not missing snippets) and is out of scope for this pass. |
| Resolve payload | `CompletionItem.data = {"uri": <entity file URI>, "name": <entity name>}` | Both values are already in scope at the point the plain-text item is built (`entity_uri`, `name`); no re-derivation needed at resolve time. |
| Who gets `data` | Only items that fell into the `None => (name.clone(), PLAIN_TEXT)` branch | Already-deep entities already have their real snippet; attaching resolve data to them would be dead weight the client might still call resolve on for no benefit. |
| Deep-parse mechanism | Reuse `workspace::ensure_fully_parsed` | Already the exact function `hover()` uses for the same "shallow → deep on demand" upgrade; no new parsing path. |
| Capability advertisement | `completionProvider.resolveProvider = true` in `initialize()` | Currently hardcoded `Some(false)`; clients won't call resolve at all until this flips. |

### Data flow

1. Client requests `textDocument/completion` after `entity rtl_lib.` → server returns the full name list immediately (unchanged cost: shallow index only).
2. Client highlights one item as the user navigates the popup and calls `completionItem/resolve` for *that* item only.
3. `completion_resolve` decodes `item.data`, calls `ensure_fully_parsed(uri)` (no-op if already deep — the function already short-circuits on non-`Shallow` files), re-reads `entity_scope_trees.get(&name)`, and if present, regenerates `item.insert_text`/`insert_text_format` via the existing `generate_instantiation_snippet`.
4. If the entry still isn't found after parsing (e.g. file deleted since the list was built), return the item unchanged — no error surfaced to the client.

### Error handling

- Malformed/missing `data`: return the item unchanged (defensive — should never happen since only server-authored items carry this field).
- Target file unreadable or now empty of the named entity: return the item unchanged.

### Testing

- `completion/tests.rs`: assert `data` is present on shallow `LibraryUnits` items and absent on already-deep ones (regression guard for the "only attach when needed" decision).
- New test driving `completion_resolve` directly: build a fixture where the target entity's file is registered as `ParseLevel::Shallow`, call `completion_resolve` with the matching `data`, assert the returned item's `insert_text_format == SNIPPET` and its content matches `generate_instantiation_snippet`'s output for that entity.

---

## Feature 2 — library-aware hover/goto on the instantiated unit name

### Decisions

| Decision | Choice | Why |
|---|---|---|
| New data on `Instance` | `pub unit_range: Option<Range>` | Span of just the unit-name token (`pipe` in `entity lib.pipe`), distinct from `range` (whole statement) and `selection_range` (label). `None` when no identifiable name token exists. |
| Where it's populated | `create_instance_from_node` (`analysis/builders.rs`), at both existing sites that assign `component` from an identifier node | No new tree-sitter traversal — the identifier node is already in hand at assignment time; just also capture its range. |
| Lookup helper | `find_instance_at(scope_trees: &[ScopeTree], pos: Position) -> Option<&Instance>` in `analysis/scope_tree.rs`, beside `collect_all_instantiations` | Same traversal shape as the existing helper; keeps instantiation-scanning logic in one place. |
| Scope of the check | Only `unit_kind == Entity` with a library present | Component/configuration instantiations resolve through component declarations, not `resolve_entity_uris`, and aren't part of this gap. |
| Resolution | Reuse `units::resolve_entity_uris(map, instance, &analysis.library)` | Already the exact function `ensure_dependencies_loaded` uses for this same instantiation shape — same semantics for `work` self-reference, case-insensitivity, etc. |
| Hover formatting | Extract the Generics/Ports reconstruction body out of `format_instantiation_hover` into a shared `format_entity_interface(sym: &Symbol) -> String`, called by both the existing label-hover case and the new unit-name case | Avoids duplicating the generic/port rendering logic; the unit-name case renders `**name**\n\n<interface>` without the "(Instance of ...)" framing, since there's no separate label here — the identifier *is* the entity name. |
| Ordering | `find_instance_at` check runs **before** `get_qualified_chain_at_pos` in both `resolve_hover` and `goto_definition` | The generic chain path currently returns a non-empty (wrong) result for this exact position; it must not get first refusal. |
| Shallow targets | Reuse the existing `needs_jit`/`ensure_fully_parsed` upgrade already in `Backend::hover` | No new JIT-parse path; the instance-aware branch just needs to request the upgrade for its own resolved `definition_uri` the same way. |

### Data flow

- **Hover:** cursor position → `find_instance_at` on the current document's scope trees → hit → `resolve_entity_uris` → target `Url` → if `Shallow`, `ensure_fully_parsed` → look up the entity's definition `Symbol`/`Declaration` → `format_entity_interface` → `HoverResolution`. Miss → fall through to today's `get_qualified_chain_at_pos` path unchanged (record/package access keeps working exactly as it does now).
- **Goto:** same `find_instance_at` hit → `resolve_entity_uris` → build `GotoDefinitionResponse::Array` from the target entity's header range. Miss → fall through to today's chain-then-bare-word path unchanged.

### Error handling

- `find_instance_at` miss: fall through to existing resolution — zero behavior change for every case that isn't this specific gap.
- `resolve_entity_uris` returns empty (unresolvable library/name, e.g. mid-typing): fall through the same way.
- Multiple candidate URIs (ambiguous name across libraries — the pre-existing multi-library caveat noted elsewhere in `roadmap.md`): take the first, consistent with how `ensure_dependencies_loaded` already handles this list today.

### Testing

- `analysis/scope_tree.rs` unit tests for `find_instance_at`: hit on a single instantiation, miss just outside the range, correct pick among multiple instantiations in one file.
- `analysis/builders.rs` unit tests: `unit_range` populated correctly for both the dotted (`entity lib.name`) and plain (`entity name`, `work.name`) forms; `None` where there's no name token to point at.
- Hover/goto integration tests (extending the LSP integration suite from `2026-07-27-lsp-integration-tests-design.md`): instantiate a shallow entity by direct form in an open document, hover/goto the unit name, assert a real signature/location comes back instead of the `void` degenerate case.

---

## Out of scope (unchanged by this pass)

- The immediately-after-label completion gap (`generate_entity_completions` missing shallow entities entirely) — separate roadmap bullet, separate fix.
- Component-instantiation hover/goto — already works through a different path, untouched here.
- Cross-library ambiguity resolution for `resolve_entity_uris` — pre-existing caveat, not addressed by either feature.
