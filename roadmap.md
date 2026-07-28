# Oxide HDL - Development Tracking

**Last Updated:** July 27, 2026

> Sections below marked v0.5/v0.6 predate the 0.6.x releases and are stale; the
> Libraries section and Known Bugs entries are current.

---

## 🚧 Current Work (v0.5)

### In Progress: The "Undeclared" Guard
**Goal:** Stop the user from using things that don't exist. This is the final semantic check needed before the analyzer is "trustworthy".

- [X] **Undeclared Identifier Diagnostics**
  - [X] Check `Reference` nodes against `ScopeTree`
  - [X] Flag unknown signals/variables/constants (Error Severity)
  - [X] Ignore known built-ins (std_logic, true/false)
- [X] **Undefined Type Diagnostics**
  - [X] Flag unknown types in signal declarations (e.g., `signal x : foo;`)
  - [X] Validate against IEEE library types (already available in `builtins.rs`)

---

## 🧭 Enhanced Navigation (v0.5.x)
**Goal:** Improve developer mobility between entities, architectures, and subprograms.

- [X] **Refine `goto_definition`**
  - [X] Prioritize Entity definitions over Component declarations.
  - [X] Fallback to Component only if the Entity is not found in the workspace.
  - [X] For Subprograms (Functions/Procedures): Jump directly to the subprogram body.
- [X] **Implement `goto_declaration`**
  - [X] Lead to the Component declaration.
  - [X] Fallback logic: If Component not found, fallback to Entity.
  - [X] For Subprograms: Match `goto_definition` behavior.
- [X] **Implement `goto_implementation`**
  - [X] Lead to the Architecture corresponding to an Entity.
  - [X] For Subprograms: Lead to the subprogram body (same as definition).

---

## ✅ Completed

### v0.5 Features (Ready)
- [x] **Package System & JIT**
  - [x] `builtins.rs`: Embedded IEEE libraries extraction
  - [x] `workspace.rs`: JIT parsing of dependencies
  - [x] `lookup.rs`: Cross-file symbol resolution
- [x] **Advanced Completion** (Previously planned for v0.8)
  - [x] Context-aware auto-complete
  - [x] Port Map completion (shows ports of the entity)
  - [x] Generic Map completion
  - [x] Dot access completion (`record.field`)

### v0.4 (December 2024)
- [x] Syntax error detection
- [x] Unused signal/constant/variable detection
- [x] Sensitivity list validation
- [x] Entity/Architecture scope extraction
- [x] Constants in type definitions marked as used

---

## 🗺️ Roadmap

### v0.6: Productivity & Smart Snippets
**Goal:** Make writing VHDL faster by automating the tedious parts using the Scope Tree.

- [X] **Smart Auto-Fill Snippets** ✨
  - [X] **Component Instantiation:** Selecting a component in completion triggers a snippet that types out `port map ( clk => $1, rst => $2 ... );`
  - [X] **Procedure/Function Calls:** Auto-fill parameter lists for subprograms.
- [X] **Rename Symbol**
  - [X] Rename identifier under cursor
  - [X] Update definition and all usages across the file
- [X] **Find All References**
  - [X] Show all locations where a signal/variable is used
- [ ] **Code Actions (Quick Fixes)**
  - [ ] "Remove unused signal" (using existing unused diagnostic)
  - [ ] "Add to sensitivity list" (using existing sensitivity diagnostic)

### v0.7: Libraries & Direct Instantiation — shipped 0.7.0

- [X] `[libraries]` config section mapping path globs to VHDL library names
- [X] Per-file library stamping at index time; `work` treated as a self-reference
- [X] Library-aware design-unit queries (`backend::units`)
- [X] JIT deep-parse of entities instantiated by an open document
- [X] Entity-name completion after a library prefix (`entity rtl_lib.`)
- [X] Workspace-wide instantiation snippets, emitted in direct form

**Deferred to 0.7.1 — library awareness does not yet reach every feature.**
`resolve_entity_uris` has one production caller (the JIT deep-parse), so these
still resolve by bare name across the whole workspace and are ambiguous when two
libraries hold the same entity name:

- [ ] **Library-aware go-to-definition and hover.** Blocked on two things:
  `goto_definition` resolves a bare word from the rope with no AST context, so it
  cannot tell an entity instantiation from a signal reference; and `Instance`
  records the statement range and the label range but not the *unit name* range,
  so there is no way to ask whether the cursor sits on the entity name. Fix is to
  add `unit_range` to `Instance`, add a `find_instance_at(analysis, pos)` helper,
  and check it before falling through to `lookup_symbol`. Roughly one task.
- [ ] **Library-aware port-map completion.** `PortMapLhs(String)` carries a bare
  name, so the library never reaches the resolution site. Changing the payload
  churns a large number of existing completion tests for a benefit that only
  appears with cross-library name collisions — lower value than the above.
- [ ] **Architecture resolution.** `Instance.architecture` is captured but read by
  nothing, so `entity work.cpu(behavioral)` cannot be validated or navigated. This
  is why `field architecture is never read` warns in a clean build.
- [ ] **Completion parity between entities and package components.** In the
  architecture-body context, `generate_entity_completions` iterates
  `entity_scope_trees`, which exist only for deep-parsed files — so a package
  component is always offered while a shallow-indexed entity is offered *not at
  all*. Two parts: (a) offer shallow entities by name, from the fast index;
  (b) implement `completionItem/resolve` (currently `resolve_provider: Some(false)`,
  no handler) to deep-parse just the selected entity and fill in its snippet.
  Measured cost of one deep parse is ~0.66 ms, so deferring is cheap; the eager
  cost that ruled `resolve` out originally only ever applied to parsing a whole
  library up front.
- [ ] **Ports and generics named after VHDL standard types are dropped.**
  Tree-sitter tags `WIDTH`, `positive`, `natural`, `line`, `text`,
  `severity_level` and `delay_length` as `library_type` rather than `identifier`,
  and `extract_decl_from_generic_clause` / `extract_decl_from_port_clause` only
  look for `identifier`. The interface silently loses those entries. Pre-existing,
  reproduces on 0.6.6. Fix mirrors the existing fallback at `builders.rs:1532`.
  Worth a 0.6.7 patch rather than waiting for the feature branch.
- [ ] **Configuration instantiation.** `configuration work.cfg` parses and is
  classified, but configurations are not indexed at all.

### v0.8: Advanced Safety
**Goal:** Catch functional bugs that compile fine but break hardware.

- [ ] **Latch Inference Warning** (If/Case without else)
- [ ] **Multiple Drivers** (Writing to same signal in multiple processes)
- [ ] **Range Validation** (e.g., assigning 8-bit to 4-bit, if statically determinable)

### Future / On Hold
- **Type Mismatches:** Full expression type inference is currently out of scope.
- **Background Worker/Disk Cache:** Current performance (<100ms) does not justify the complexity yet.

### Post 1.0

- **Parser / LSP Split** (`oxide-parser` + `oxide-parser-cli`)
  - Extract the parser and analysis layer into a standalone library crate.
  - Expose it to external consumers (Python, C++, shell) via a JSON CLI binary.
  - oxide-hdl becomes a thin LSP layer depending on the library — zero regression for LSP users.
  - See `parser-split.md` for the full design.

- **Overload Resolution for Sensitivity Analysis**
  - **Context:** When a procedure has multiple overloads with different parameter directions
    (e.g., two overloads where the 2nd param is `out`, and a third where it is `in`), the
    sensitivity checker currently falls back to conservative "direction unknown" treatment —
    no "missing" and no "unnecessary" warnings are emitted for any argument of that call.
  - **Goal:** Resolve which overload is actually being called by matching argument count and,
    ideally, argument types.  Once the correct overload is identified, parameter directions
    can be applied precisely, recovering accurate sensitivity diagnostics.
  - **Why it's hard:** Requires partial type inference on actual arguments to disambiguate
    overloads — the same problem as full type checking, just scoped to call sites.  May also
    need to handle implicit type conversions (e.g., `std_logic` ↔ `std_ulogic`).
  - **Status:** Deferred — the conservative fallback is correct and safe; this is a
    precision improvement, not a correctness fix.

---

## 🐛 Known Bugs & Concerns

### Medium Priority
1. **Overloaded Functions:**
   - **Issue:** Function/procedure resolution doesn't check signature types, only names.
   - **Sensitivity impact:** When multiple overloads exist, the sensitivity checker falls back
     to "direction unknown" — no false positives, but also no diagnostics for those arguments.
   - **Status:** Safe conservative fallback in place; full overload resolution tracked as post-1.0.
2. **Subprogram Deduplication (Hover):**
   - **Issue:** Hover might show both Specification and Body if both are indexed.
   - **Task:** Ensure `lookup_symbol` (find_all=false) or formatter prioritizes Body over Spec to avoid duplicates in tooltips.

3. **No partial scope-tree recovery:**
   - **Issue:** Any unclosed construct (an `if` awaiting `end if;`, an unclosed
     `process`/`generate`/`block`, or a missing `end architecture;`) makes
     `extract_document_symbols` return zero scope trees for the file.
   - **Mitigated in 0.6.6:** the last good analysis is retained rather than being
     overwritten, so completion/hover/goto keep working against slightly stale data.
   - **Not fixed:** freshly typed text still contributes nothing until the file
     parses. Real error recovery in `builders.rs` would fix this and also fix the
     related context-detection bug below.
4. **Completion context misdetected on an unclosed paren:**
   - **Issue:** With an open `port map (` followed by `end architecture;`,
     `get_completion_context` returns `Architecture` instead of `PortMapLhs`, so
     scope items are offered where ports should be.
   - **Impact:** Limited — most editors auto-close the paren, which avoids it.
   - Affects component and direct instantiation equally.

2. **Record Type Visibility:**
   - **Issue:** Dot completion relies on text heuristics in some edge cases.
   - **Status:** Working well enough.

---

## 📝 Notes

### Design Decisions Log

1. **Scope Trees as Single Source of Truth**
   - Built once, used for Validation, Hover, Goto, Rename, and Snippet Generation.

2. **JIT Parsing Strategy**
   - Only parse deep structure of files when requested.
   - Keeps startup fast without needing a background worker.

3. **Skipping Type Inference**
   - We focus on "Symbol Resolution" (finding the name) rather than "Type Resolution" (validating the expression).
