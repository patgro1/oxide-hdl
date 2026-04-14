# Smart Proc/Func Call Completion — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add context-aware LSP completion inside function/procedure call argument lists (LHS parameter names, RHS in-scope values, filtered by already-supplied params), then port the "already-used filter" to instantiation port/generic maps.

**Architecture:** Three new `CompletionContext` variants drive detection and resolution. A paren-depth-aware `collect_used_param_names` helper filters already-bound parameter names. Detection happens in `handle_map_node` via a new `try_subprogram_call_context` function that intercepts `association_element`/`parenthesis_group` nodes before the existing instantiation logic. Resolution is wired into `complete_scope` before the existing `match context` block.

**Tech Stack:** Rust, tree-sitter VHDL grammar, tower-lsp. All changes are in `src/backend/features/completion/mod.rs` and `src/backend/features/completion/tests.rs`.

---

## File Map

| File | Changes |
|---|---|
| `src/backend/features/completion/mod.rs` | New constants, 3 new `CompletionContext` variants, 5 new helpers, extend `handle_map_node`, extend `complete_scope`, filter instantiation LHS |
| `src/backend/features/completion/tests.rs` | Unit tests for all new helpers, context detection tests, integration tests |

---

## Task 1: Add Node Kind Constants and CompletionContext Variants

**Files:**
- Modify: `src/backend/features/completion/mod.rs`

- [ ] **Step 1: Add node kind constants**

In the `mod node_kinds` block (around line 18), add:

```rust
pub const FUNCTION_CALL: &str = "function_call";
pub const PARENTHESIS_GROUP: &str = "parenthesis_group";
pub const ASSOCIATION_OR_RANGE_LIST: &str = "association_or_range_list";
```

- [ ] **Step 2: Add new CompletionContext variants**

In the `CompletionContext` enum (around line 53), add three new variants after `GenericMapRhs`:

```rust
/// Inside a subprogram call argument list with no arguments yet (empty or whitespace only).
/// Suggests both parameter names (LHS) and in-scope values (RHS), params first.
/// Payload: subprogram name (lowercase).
SubprogramCallBoth(String),

/// Inside a subprogram call argument list, before `=>` in named association mode.
/// Suggests: parameter names not yet supplied.
/// Payload: subprogram name (lowercase).
SubprogramCallLhs(String),

/// Inside a subprogram call argument list after `=>`, or in positional mode (args present, no `=>`).
/// Suggests: in-scope signals, variables, constants.
SubprogramCallRhs,
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check 2>&1 | head -30
```

Expected: warnings only (dead code on new variants until later tasks), no errors.

- [ ] **Step 4: Commit**

```bash
git add src/backend/features/completion/mod.rs
git commit -m "feat: add SubprogramCallBoth/Lhs/Rhs context variants and node kind constants"
```

---

## Task 2: `collect_used_param_names` — Paren-Depth-Aware Helper

**Files:**
- Modify: `src/backend/features/completion/mod.rs`
- Modify: `src/backend/features/completion/tests.rs`

- [ ] **Step 1: Write the failing tests**

In `tests.rs`, add a new test module after the existing unit tests (around line 100):

```rust
// --- Unit Tests for collect_used_param_names ---

#[test]
fn test_collect_used_param_names_empty() {
    // Empty parens: no content between ( and cursor
    assert_eq!(collect_used_param_names("func(", 4, 5), HashSet::new());
}

#[test]
fn test_collect_used_param_names_whitespace_only() {
    assert_eq!(collect_used_param_names("func(  ", 4, 7), HashSet::new());
}

#[test]
fn test_collect_used_param_names_single() {
    // "func(a => x, " — only "a" should be collected
    let text = "func(a => x, ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_multiple() {
    let text = "func(a => x, b => y, ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a", "b"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_aggregate_rhs() {
    // "others" inside aggregate must NOT be collected
    let text = "func(a => (others => '0'), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_array_aggregate_rhs() {
    // Array aggregate indexes like "0 =>" and "1 =>" must NOT be collected
    let text = "func(a => (0 => '1', 1 => '0'), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_nested_call() {
    // "x" is inside inner_func call, depth 2 — must NOT be collected
    let text = "func(a => inner_func(x => y), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_case_insensitive() {
    let text = "func(PARAM_A => x, ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["param_a"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_mixed_aggregate_and_named() {
    let text = "func(a => x, b => (others => '0'), ";
    assert_eq!(
        collect_used_param_names(text, 4, text.len()),
        ["a", "b"].iter().map(|s| s.to_string()).collect::<HashSet<_>>()
    );
}

#[test]
fn test_collect_used_param_names_positional_no_arrow() {
    // No "=>" at top level — positional args — nothing collected
    let text = "func(x, y, ";
    assert_eq!(collect_used_param_names(text, 4, text.len()), HashSet::new());
}
```

Add `use std::collections::HashSet;` at the top of the test file if not already present.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test collect_used_param_names 2>&1 | tail -20
```

Expected: compile error — `collect_used_param_names` not defined yet.

- [ ] **Step 3: Implement `collect_used_param_names`**

Add after the `is_rhs_of_association` function in `mod.rs` (around line 132):

```rust
/// Collects the names of parameters already bound in a subprogram call or map argument list.
///
/// Scans `text[open_paren_offset..cursor_offset]` tracking paren depth.
/// Only collects `identifier =>` patterns at depth 1 (directly inside the call's `(`),
/// ignoring `=>` tokens inside nested aggregates, inner calls, or qualified expressions.
///
/// # Arguments
/// * `text` - The full source text.
/// * `open_paren_offset` - Byte offset of the `(` that opens the argument list.
/// * `cursor_offset` - Byte offset of the cursor.
///
/// # Returns
/// A `HashSet<String>` of lowercased parameter names already supplied.
fn collect_used_param_names(
    text: &str,
    open_paren_offset: usize,
    cursor_offset: usize,
) -> std::collections::HashSet<String> {
    let mut used = std::collections::HashSet::new();
    let limit = cursor_offset.min(text.len());
    if open_paren_offset >= limit {
        return used;
    }
    let slice = &text[open_paren_offset..limit];
    let bytes = slice.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    let mut depth: usize = 0;

    while i < n {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b if depth == 1 && (b.is_ascii_alphabetic() || b == b'_') => {
                // Potential identifier at top level of the argument list
                let start = i;
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = &slice[start..i];
                // Skip whitespace, then check for =>
                let mut j = i;
                while j < n && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                if j + 1 < n && bytes[j] == b'=' && bytes[j + 1] == b'>' {
                    used.insert(ident.to_ascii_lowercase());
                }
                // Do NOT advance i here; the loop will continue from i (after ident chars consumed)
            }
            _ => {
                i += 1;
            }
        }
    }
    used
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test collect_used_param_names 2>&1 | tail -20
```

Expected: all 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: add paren-depth-aware collect_used_param_names helper"
```

---

## Task 3: `has_top_level_arrow` and `classify_call_args` Helpers

**Files:**
- Modify: `src/backend/features/completion/mod.rs`
- Modify: `src/backend/features/completion/tests.rs`

- [ ] **Step 1: Write the failing tests**

Add to `tests.rs`:

```rust
// --- Unit Tests for has_top_level_arrow ---

#[test]
fn test_has_top_level_arrow_simple() {
    assert!(has_top_level_arrow("a => x"));
    assert!(!has_top_level_arrow("a, b, c"));
    assert!(!has_top_level_arrow(""));
}

#[test]
fn test_has_top_level_arrow_nested_ignored() {
    // => inside parens is NOT at top level
    assert!(!has_top_level_arrow("(others => '0')"));
    // but one at top level + one nested
    assert!(has_top_level_arrow("a => (others => '0')"));
}

#[test]
fn test_has_top_level_arrow_nested_call() {
    assert!(!has_top_level_arrow("inner_func(x => y)"));
    assert!(has_top_level_arrow("a => inner_func(x => y)"));
}

// --- Unit Tests for classify_call_args ---

#[test]
fn test_classify_call_args_empty() {
    assert_eq!(
        classify_call_args("func(".to_string(), "func(", 4, 5),
        CompletionContext::SubprogramCallBoth("func".to_string())
    );
}

#[test]
fn test_classify_call_args_whitespace_only() {
    let text = "func(   ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallBoth("func".to_string())
    );
}

#[test]
fn test_classify_call_args_named_lhs() {
    // After comma in named mode, cursor is on LHS
    let text = "func(a => x, ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallLhs("func".to_string())
    );
}

#[test]
fn test_classify_call_args_named_rhs() {
    // After => in named mode
    let text = "func(a => ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallRhs
    );
}

#[test]
fn test_classify_call_args_positional() {
    // Args present but no => at top level
    let text = "func(x, ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallRhs
    );
}

#[test]
fn test_classify_call_args_aggregate_does_not_trigger_named() {
    // (others => '0') is a positional arg — no top-level =>
    let text = "func((others => '0'), ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallRhs
    );
}

#[test]
fn test_classify_call_args_named_after_aggregate_rhs_lhs() {
    // Named arg whose value is an aggregate — cursor is after comma, on LHS
    let text = "func(a => (others => '0'), ";
    assert_eq!(
        classify_call_args("func".to_string(), text, 4, text.len()),
        CompletionContext::SubprogramCallLhs("func".to_string())
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test "has_top_level_arrow|classify_call_args" 2>&1 | tail -20
```

Expected: compile errors — functions not defined.

- [ ] **Step 3: Implement `has_top_level_arrow`**

Add after `collect_used_param_names` in `mod.rs`:

```rust
/// Returns `true` if the text contains a `=>` token at paren depth 0.
///
/// Used to detect whether a call's argument list is using named association.
/// Ignores `=>` tokens inside nested parentheses (aggregates, inner calls).
///
/// # Arguments
/// * `text` - The inner content of a call's argument list (after the opening `(`).
fn has_top_level_arrow(text: &str) -> bool {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < n {
        match bytes[i] {
            b'(' => { depth += 1; i += 1; }
            b')' => { depth = depth.saturating_sub(1); i += 1; }
            b'=' if depth == 0 && i + 1 < n && bytes[i + 1] == b'>' => return true,
            _ => { i += 1; }
        }
    }
    false
}
```

- [ ] **Step 4: Implement `classify_call_args`**

Add after `has_top_level_arrow`:

```rust
/// Determines the appropriate `CompletionContext` for a cursor inside a subprogram call.
///
/// Three modes:
/// - **Both**: argument list is empty/whitespace — user hasn't committed to positional or named.
/// - **Lhs**: named association mode (`=>` present at top level), cursor is before `=>`.
/// - **Rhs**: after `=>` in named mode, OR positional mode (args present but no top-level `=>`).
///
/// # Arguments
/// * `name` - The subprogram name (lowercase), used as the context payload.
/// * `text` - The full source text.
/// * `open_paren_offset` - Byte offset of the `(` opening the argument list.
/// * `cursor_offset` - Byte offset of the cursor.
fn classify_call_args(
    name: String,
    text: &str,
    open_paren_offset: usize,
    cursor_offset: usize,
) -> CompletionContext {
    let limit = cursor_offset.min(text.len());
    let inner_start = (open_paren_offset + 1).min(limit);

    if inner_start >= limit || text[inner_start..limit].chars().all(char::is_whitespace) {
        return CompletionContext::SubprogramCallBoth(name);
    }

    let inner = &text[inner_start..limit];

    if has_top_level_arrow(inner) {
        if is_rhs_of_association(text, cursor_offset) {
            CompletionContext::SubprogramCallRhs
        } else {
            CompletionContext::SubprogramCallLhs(name)
        }
    } else {
        CompletionContext::SubprogramCallRhs
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test "has_top_level_arrow|classify_call_args" 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: add has_top_level_arrow and classify_call_args helpers"
```

---

## Task 4: Context Detection — Wire Into Upward Traversal

**Files:**
- Modify: `src/backend/features/completion/mod.rs`
- Modify: `src/backend/features/completion/tests.rs`

- [ ] **Step 1: Write the failing context detection tests**

Add to `tests.rs`. These use the existing `check_context` helper with full VHDL snippets. The function `my_func` is defined in the architecture and called in a process — tree-sitter will parse this even with the cursor marker removed.

```rust
// --- Context Detection Tests: Subprogram Call ---

// Each test below is a self-contained VHDL snippet with | as the cursor marker.

#[test]
fn test_context_subprogram_call_empty() {
    // Empty parens — offer both LHS and RHS
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(|);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallBoth("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_named_lhs_first_arg() {
    // Before => on the first arg
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(|a => 0, b => 1);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_named_rhs() {
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallRhs,
    );
}

#[test]
fn test_context_subprogram_call_named_lhs_after_comma() {
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_positional() {
    // Positional args — no => at top level → RHS only
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(0, |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallRhs,
    );
}

#[test]
fn test_context_subprogram_call_aggregate_in_named_rhs_then_lhs() {
    // Aggregate value for first arg — cursor is on LHS of second arg
    check_context(
        r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return a; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => (0 + 1), |);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_func".to_string()),
    );
}

#[test]
fn test_context_subprogram_call_nested_inner_wins() {
    // Cursor inside inner call — context should be for inner call, not outer
    check_context(
        r#"
architecture rtl of e is
    function outer(x : integer) return integer is begin return x; end function;
    function inner(p : integer) return integer is begin return p; end function;
begin
    process is variable v : integer; begin
        v := outer(inner(|));
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallBoth("inner".to_string()),
    );
}

#[test]
fn test_context_procedure_call_named_lhs() {
    check_context(
        r#"
architecture rtl of e is
    procedure my_proc(signal clk : in bit; constant n : in integer) is
    begin null; end procedure;
    signal sys_clk : bit;
begin
    process is begin
        my_proc(|clk => sys_clk, n => 8);
    end process;
end architecture;"#,
        CompletionContext::SubprogramCallLhs("my_proc".to_string()),
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test "test_context_subprogram_call|test_context_procedure_call" 2>&1 | tail -30
```

Expected: tests compile but fail — context returns `Process` or `Unresolved` instead of `SubprogramCall*`.

- [ ] **Step 3: Add `extract_subprogram_name` helper**

Add to `mod.rs` after `classify_call_args`:

```rust
/// Extracts the subprogram name from a `function_call` AST node.
///
/// The first `name` child of `function_call` holds the callee. This function
/// returns its last dot-separated segment (lowercased), e.g. `pkg.my_func` → `my_func`.
///
/// # Arguments
/// * `function_call_node` - The `function_call` tree-sitter node.
/// * `text` - The full source text.
fn extract_subprogram_name(function_call_node: Node, text: &str) -> String {
    let mut cursor = function_call_node.walk();
    for child in function_call_node.children(&mut cursor) {
        if child.kind() == NAME {
            let name_text = &text[child.start_byte()..child.end_byte()];
            return name_text
                .split('.')
                .last()
                .unwrap_or(name_text)
                .trim()
                .to_ascii_lowercase();
        }
    }
    String::new()
}
```

- [ ] **Step 4: Add `try_subprogram_call_context` helper**

Add to `mod.rs` after `extract_subprogram_name`:

```rust
/// Walks up from `node` looking for a `parenthesis_group` whose parent is `function_call`.
///
/// Returns `Some(CompletionContext)` if a subprogram call context is found.
/// Returns `None` if `port_map_aspect` or `generic_map_aspect` is encountered first
/// (meaning we are inside an instantiation map, not a subprogram call).
///
/// # Arguments
/// * `node` - Starting node (the current node in the upward traversal).
/// * `text` - Full source text.
/// * `cursor_offset` - Byte offset of the cursor.
fn try_subprogram_call_context(
    node: Node,
    text: &str,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let mut current = Some(node);
    while let Some(n) = current {
        let kind = n.kind();
        // Stop — this is an instantiation map, not a call
        if kind == PORT_MAP_ASPECT || kind == GENERIC_MAP_ASPECT {
            return None;
        }
        if kind == PARENTHESIS_GROUP {
            if let Some(parent) = n.parent() {
                if parent.kind() == FUNCTION_CALL {
                    let name = extract_subprogram_name(parent, text);
                    let open_paren_offset = n.start_byte();
                    return Some(classify_call_args(name, text, open_paren_offset, cursor_offset));
                }
            }
        }
        current = n.parent();
    }
    None
}
```

- [ ] **Step 5: Wire into `handle_map_node`**

In `handle_map_node` (around line 786), add a check at the top of the function, before the `match kind` block:

```rust
fn handle_map_node(
    node: Node,
    text: &str,
    point: Point,
    cursor_offset: usize,
) -> Option<CompletionContext> {
    let kind = node.kind();

    // Check for subprogram call context first.
    // This must precede association_element / association_list handling because those
    // nodes appear inside function_call argument lists too.
    if matches!(
        kind,
        ASSOCIATION_ELEMENT | ASSOCIATION_LIST | ASSOCIATION_OR_RANGE_LIST | PARENTHESIS_GROUP
    ) {
        if let Some(ctx) = try_subprogram_call_context(node, text, cursor_offset) {
            return Some(ctx);
        }
    }

    match kind {
        ERROR | SIGNAL_ASSIGNMENT => handle_error_or_assignment_node(node, text, cursor_offset),
        ASSOCIATION_ELEMENT => handle_association_element(node, text, point, cursor_offset),
        ASSOCIATION_LIST => handle_association_list(node, text, point, cursor_offset),
        // ... rest unchanged ...
    }
}
```

Note: `ASSOCIATION_LIST` in the existing code is `"association_list"`. The tree-sitter VHDL grammar uses `"association_or_range_list"` as the node kind inside `parenthesis_group`. Both are checked to be safe.

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test "test_context_subprogram_call|test_context_procedure_call" 2>&1 | tail -30
```

Expected: all 8 tests pass.

- [ ] **Step 7: Verify no existing tests regressed**

```bash
cargo test completion 2>&1 | tail -20
```

Expected: all previously passing tests still pass.

- [ ] **Step 8: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: detect subprogram call context in completion upward traversal"
```

---

## Task 5: Resolution — Wire Into `complete_scope`

**Files:**
- Modify: `src/backend/features/completion/mod.rs`
- Modify: `src/backend/features/completion/tests.rs`

- [ ] **Step 1: Write the failing integration tests**

Add to `tests.rs`. Use the existing `complete_in_arch` helper pattern:

```rust
// --- Integration Tests: SubprogramCall completion items ---

/// Helper for subprogram call completion tests.
/// Returns completion labels at the cursor position in the given arch code.
fn complete_subprogram_call(arch_code: &str) -> Vec<String> {
    use crate::backend::test_utils::parse_text;
    use tower_lsp::lsp_types::Url;
    use crate::backend::AnalysisMap;

    let arch_uri = Url::parse("file:///arch.vhd").unwrap();
    let (code, pos) = extract_cursor(arch_code);

    let _guard = SHARED_PARSER_LOCK.lock().unwrap();
    let arch_tree = parse_text(&code);
    let arch_root = arch_tree.root_node();
    let arch_analysis =
        crate::backend::syntax::parser::extract_document_symbols(&code, arch_root);
    drop(_guard);

    let mut analysis_map = AnalysisMap::new();
    analysis_map.insert(arch_uri.clone(), arch_analysis);

    let ctx = get_completion_context(&code, arch_root, pos);
    let items = complete_scope(&analysis_map, &arch_uri, &ctx, pos, &code, arch_root);
    labels(&items).iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_subprogram_lhs_offers_all_params_when_empty() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(|);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(names.contains(&"a".to_string()), "param 'a' should appear. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "param 'b' should appear. Got: {:?}", names);
    assert!(names.contains(&"c".to_string()), "param 'c' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_filters_already_supplied_param() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered out. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "param 'b' should appear. Got: {:?}", names);
    assert!(names.contains(&"c".to_string()), "param 'c' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_filters_multiple_supplied_params() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer; c : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, b => 1, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"b".to_string()), "'b' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"c".to_string()), "param 'c' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_all_params_filtered_when_all_supplied() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => 0, b => 1, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"b".to_string()), "'b' should be filtered. Got: {:?}", names);
}

#[test]
fn test_subprogram_lhs_aggregate_rhs_does_not_filter_wrong_param() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return 0; end function;
begin
    process is variable v : integer; begin
        v := my_func(a => (0 + 1), |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    assert!(!names.contains(&"a".to_string()), "'a' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"b".to_string()), "'b' should appear. Got: {:?}", names);
}

#[test]
fn test_subprogram_positional_offers_no_param_names() {
    let arch = r#"
architecture rtl of e is
    function my_func(a : integer; b : integer) return integer is
    begin return 0; end function;
    signal s : integer;
begin
    process is variable v : integer; begin
        v := my_func(0, |);
    end process;
end architecture;"#;

    let names = complete_subprogram_call(arch);
    // In positional mode, param names should not appear as completions
    assert!(!names.contains(&"a".to_string()), "'a' should not appear in positional mode. Got: {:?}", names);
    assert!(!names.contains(&"b".to_string()), "'b' should not appear in positional mode. Got: {:?}", names);
    // But in-scope signals/variables should appear
    assert!(names.contains(&"s".to_string()) || names.contains(&"v".to_string()),
        "in-scope items should appear in positional mode. Got: {:?}", names);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test "test_subprogram_lhs|test_subprogram_positional" 2>&1 | tail -20
```

Expected: tests compile but all fail — context resolves but `complete_scope` doesn't handle new variants yet.

- [ ] **Step 3: Add `find_call_open_paren` helper**

Add to `mod.rs` before `complete_scope`:

```rust
/// Scans backward from `cursor_offset` to find the byte offset of the `(`
/// that opens the current call's or map's argument list.
///
/// Tracks paren depth: the first unmatched `(` found scanning backward is the opener.
///
/// # Arguments
/// * `text` - Full source text.
/// * `cursor_offset` - Byte offset of the cursor.
///
/// # Returns
/// `Some(offset)` of the `(`, or `None` if not found.
fn find_call_open_paren(text: &str, cursor_offset: usize) -> Option<usize> {
    let limit = cursor_offset.min(text.len());
    let bytes = text.as_bytes();
    let mut depth: usize = 0;
    let mut i = limit;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}
```

- [ ] **Step 4: Wire `SubprogramCallLhs` / `SubprogramCallBoth` / `SubprogramCallRhs` into `complete_scope`**

In `complete_scope`, add the following block **before** the `match context {` statement (after the `local_scope_tree` binding, around line 1610). This handles both the LHS param filtering and the Both mode's LHS phase:

```rust
// --- Subprogram Call LHS: offer filtered parameter names ---
// This runs before the match so SubprogramCallBoth can fall through to the _ arm for RHS items.
if let CompletionContext::SubprogramCallLhs(name) | CompletionContext::SubprogramCallBoth(name) =
    context
{
    let cursor_offset = position_to_offset(text, position);
    let open_paren = find_call_open_paren(text, cursor_offset).unwrap_or(0);
    let used = collect_used_param_names(text, open_paren, cursor_offset);

    if let Some(tree) = &local_scope_tree {
        let innermost = tree.find_innermost_scope(&position);
        let header = tree
            .entity
            .as_ref()
            .and_then(|n| {
                current_analysis
                    .entity_scope_trees
                    .get(n)
                    .or_else(|| analysis_map.values().find_map(|a| a.entity_scope_trees.get(n)))
            })
            .or_else(|| {
                tree.package.as_ref().and_then(|n| {
                    current_analysis
                        .package_declaration_scope_trees
                        .get(n)
                        .or_else(|| current_analysis.package_body_scope_trees.get(n))
                })
            });

        if let Some(declarations) = tree.collect_visible_declarations(&innermost.range, header) {
            for decl in &declarations {
                if decl.name.eq_ignore_ascii_case(name)
                    && matches!(
                        decl.decl_type,
                        DeclType::Function
                            | DeclType::FunctionDeclaration
                            | DeclType::Procedure
                            | DeclType::ProcedureDeclaration
                    )
                {
                    if let Some(params) = &decl.parameters {
                        for param in params {
                            if !used.contains(&param.name.to_ascii_lowercase()) {
                                items.push(declaration_to_completion(param));
                            }
                        }
                    }
                }
            }
        }
    }

    // SubprogramCallLhs returns here (param names only).
    // SubprogramCallBoth falls through to the match `_` arm for RHS scope items.
    if matches!(context, CompletionContext::SubprogramCallLhs(_)) {
        items.sort_by(|a, b| a.label.cmp(&b.label));
        items.dedup_by(|a, b| a.label == b.label);
        return items;
    }
}
```

In the `match context {` block, ensure `SubprogramCallRhs` and `SubprogramCallBoth` are NOT explicitly matched — they will naturally fall into the `_` arm which already provides all visible declarations. No match arm needed for these two.

- [ ] **Step 5: Run integration tests**

```bash
cargo test "test_subprogram_lhs|test_subprogram_positional" 2>&1 | tail -30
```

Expected: all 6 tests pass.

- [ ] **Step 6: Run full completion test suite**

```bash
cargo test completion 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: resolve SubprogramCall contexts in complete_scope with param filtering"
```

---

## Task 6: Phase 2 — Filter Already-Used Params in Instantiation Completion

**Files:**
- Modify: `src/backend/features/completion/mod.rs`
- Modify: `src/backend/features/completion/tests.rs`

- [ ] **Step 1: Write the failing tests**

Add to `tests.rs`:

```rust
// --- Phase 2: Instantiation already-used param filtering ---

#[test]
fn test_port_map_lhs_filters_already_connected_port() {
    // clk is already connected — should NOT appear in LHS suggestions
    let pkg = "";  // no package needed
    let arch = r#"
architecture rtl of e is
    component dut is
        port (clk : in bit; data : in bit; q : out bit);
    end component;
    signal sys_clk : bit;
    signal d : bit;
    signal out_q : bit;
begin
    u1: dut port map (clk => sys_clk, |);
end architecture;"#;

    let items = complete_in_arch(pkg, arch, {
        // Position: inside the port map after the comma
        let line = arch.lines().enumerate()
            .find(|(_, l)| l.contains("|"))
            .map(|(i, _)| i as u32)
            .unwrap_or(0);
        Position { line, character: 37 }
    });
    let names = labels(&items);

    assert!(!names.contains(&"clk"), "'clk' should be filtered (already connected). Got: {:?}", names);
    assert!(names.contains(&"data"), "'data' should appear. Got: {:?}", names);
    assert!(names.contains(&"q"), "'q' should appear. Got: {:?}", names);
}

#[test]
fn test_port_map_lhs_filters_multiple_connected_ports() {
    let arch = r#"
architecture rtl of e is
    component dut is
        port (clk : in bit; data : in bit; q : out bit);
    end component;
    signal sys_clk : bit;
    signal d : bit;
begin
    u1: dut port map (clk => sys_clk, data => d, |);
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    assert!(!names.contains(&"clk"), "'clk' should be filtered. Got: {:?}", names);
    assert!(!names.contains(&"data"), "'data' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"q"), "'q' should still appear. Got: {:?}", names);
}

#[test]
fn test_port_map_lhs_aggregate_value_does_not_confuse_filter() {
    // port map (data => (others => '0'), | ) — "others" must not be filtered as a port name
    let arch = r#"
architecture rtl of e is
    component dut is
        port (data : in bit; q : out bit);
    end component;
begin
    u1: dut port map (data => (0 or 0), |);
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    // "data" should be filtered; "q" should still appear
    // The key thing is "others" should not be treated as a port name that's filtered
    assert!(!names.contains(&"others"), "'others' must not appear as a filtered port name. Got: {:?}", names);
}

#[test]
fn test_generic_map_lhs_filters_already_set_generic() {
    let arch = r#"
architecture rtl of e is
    component dut is
        generic (WIDTH : integer; DEPTH : integer);
        port (clk : in bit);
    end component;
begin
    u1: dut
        generic map (WIDTH => 8, |)
        port map (clk => '0');
end architecture;"#;

    let (_code, pos) = extract_cursor(arch);
    let items = complete_in_arch("", &arch.replace("|", ""), pos);
    let names = labels(&items);
    assert!(!names.contains(&"WIDTH"), "'WIDTH' should be filtered. Got: {:?}", names);
    assert!(names.contains(&"DEPTH") || names.contains(&"depth"), "'DEPTH' should appear. Got: {:?}", names);
}
```

Note: The test for multiple filtered ports uses a simpler approach with `extract_cursor` — refactor as needed once you see the actual positions. The aggregate test is the most important correctness check.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test "test_port_map_lhs_filter|test_generic_map_lhs_filter|test_port_map_lhs_aggregate" 2>&1 | tail -20
```

Expected: tests compile, some may pass trivially (if component resolution doesn't work in test context), the aggregate test should pass or be a no-op.

- [ ] **Step 3: Add `collect_used_param_names` filter to `PortMapLhs`/`GenericMapLhs` handler**

In `complete_scope`, inside the `CompletionContext::PortMapLhs(target_name) | CompletionContext::GenericMapLhs(target_name)` match arm, add filtering after each place where items are pushed.

The current structure (around line 1629) collects items in two places: the global entity lookup and the local component lookup. Add filtering to both.

At the top of the match arm body, compute the used names once:

```rust
CompletionContext::PortMapLhs(target_name)
| CompletionContext::GenericMapLhs(target_name) => {
    let is_generic = matches!(context, CompletionContext::GenericMapLhs(_));
    let target_lower = target_name.to_lowercase();

    // Compute already-connected port/generic names to filter from suggestions
    let cursor_offset = position_to_offset(text, position);
    let open_paren = find_call_open_paren(text, cursor_offset).unwrap_or(0);
    let used = collect_used_param_names(text, open_paren, cursor_offset);

    // First we look in the global lookup
    for analysis in analysis_map.values() {
        if let Some(entity_tree) = analysis.entity_scope_trees.get(&target_lower) {
            for decl in &entity_tree.declarations {
                let valid = if is_generic {
                    matches!(decl.decl_type, DeclType::Generic)
                } else {
                    matches!(decl.decl_type, DeclType::Port(_))
                };

                if valid && !used.contains(&decl.name.to_ascii_lowercase()) {  // ← ADD FILTER
                    items.push(declaration_to_completion(decl));
                }
            }
        }
    }

    // ... rest of the arm (local component lookup) — also add the same filter:
    // if valid && !used.contains(&param.name.to_ascii_lowercase()) {
    //     items.push(declaration_to_completion(param));
    // }
```

Apply the `!used.contains(...)` check to every place that pushes a completion item inside this match arm.

- [ ] **Step 4: Run tests**

```bash
cargo test "test_port_map_lhs|test_generic_map" 2>&1 | tail -30
```

Expected: tests pass. If the component lookup in tests doesn't resolve (because the test VHDL is minimal), the filter may not have items to filter — this is acceptable, the aggregate "others" test is the critical correctness check.

- [ ] **Step 5: Run full suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass, no regressions.

- [ ] **Step 6: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: filter already-connected ports/generics from instantiation LHS completions"
```

---

## Task 7: Final Verification

- [ ] **Step 1: Run the full test suite**

```bash
cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 2: Build release**

```bash
cargo build --release 2>&1 | tail -10
```

Expected: builds cleanly.

- [ ] **Step 3: Commit if any final fixups were needed, then tag**

```bash
git log --oneline feat/add_proc_call_smart_complete ^main
```

Expected: 6–7 commits covering all tasks.
