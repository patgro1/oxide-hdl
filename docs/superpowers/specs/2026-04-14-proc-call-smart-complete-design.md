# Smart Completion for Function/Procedure Call Arguments

**Date:** 2026-04-14  
**Branch:** `feat/add_proc_call_smart_complete`  
**Scope:** Two phases — (1) subprogram call argument completion, (2) port missing-param filtering ported to instantiation completion.

---

## Motivation

The LSP already provides smart, context-aware completion inside component/entity instantiation port and generic maps. The same level of intelligence should apply when typing arguments inside function and procedure calls — offering parameter names on the LHS of `=>`, in-scope values on the RHS, and filtering out parameters that have already been supplied.

---

## Phase 1: Function/Procedure Call Argument Completion

### Context Detection

Three new `CompletionContext` variants are added:

```rust
/// Inside a subprogram call argument list, no args typed yet.
/// Offer both parameter names (LHS) and in-scope values (RHS).
/// Payload: subprogram name (lowercase).
SubprogramCallBoth(String),

/// Inside a subprogram call argument list, before `=>` (named association LHS).
/// Payload: subprogram name (lowercase).
SubprogramCallLhs(String),

/// Inside a subprogram call argument list, after `=>`, or in positional mode.
SubprogramCallRhs,
```

### AST Structure

A function or procedure call in the tree-sitter VHDL grammar is represented as:

```
name
  function_call
    name                      ← subprogram name (identifier leaf)
    parenthesis_group
      association_or_range_list
        association_element   ← one per argument
```

`procedure_call_statement` wraps a `name` node which contains a `function_call`. The `function_call` node is the key anchor for detection in both cases.

### Detection Logic

Detection is integrated into `handle_upward_traversal` by checking for `parenthesis_group` whose parent is a `function_call`. When found:

1. Extract the subprogram name from `function_call`'s first `name` child (text content, lowercased).
2. Call `classify_call_args(text, open_paren_offset, cursor_offset)` to determine the calling convention in play:
   - **Empty** (only whitespace between `(` and cursor) → `SubprogramCallBoth(name)`
   - **Has `=>` at depth 1** (named association present) → apply `is_rhs_of_association` to determine `SubprogramCallLhs(name)` or `SubprogramCallRhs`
   - **Has content but no `=>` at depth 1** (positional mode) → `SubprogramCallRhs`

`open_paren_offset` is derived from the `parenthesis_group` node's start byte.

### The `collect_used_param_names` Helper

Extracts parameter names already bound in the argument list, **paren-depth aware** to correctly ignore `=>` inside nested aggregates, qualified expressions, or inner function calls.

```rust
fn collect_used_param_names(
    text: &str,
    open_paren_offset: usize,
    cursor_offset: usize,
) -> HashSet<String>
```

**Algorithm:**

```
depth = 0
i = open_paren_offset
scan text[i..cursor_offset] character by character:
  '(' → depth += 1
  ')' → depth -= 1
  at depth == 1:
    when we see: identifier followed (ignoring whitespace) by '=>'
    → collect identifier.to_lowercase() into result set
```

This correctly handles:
- `func(param_a => (others => '0'), |)` — `others` is at depth 2, ignored; `param_a` collected
- `func(param_a => inner(x => y), |)` — `x` is at depth 2, ignored; `param_a` collected
- `func(param_a => (0 => '1', 1 => '0'), |)` — array aggregate `=>` at depth 2, ignored

### Completion Resolution

Handled in the main completion dispatch:

| Context | What to offer |
|---|---|
| `SubprogramCallLhs(name)` | Parameters of `name` not in `collect_used_param_names` |
| `SubprogramCallRhs` | In-scope signals, variables, constants (same as `PortMapRhs`) |
| `SubprogramCallBoth(name)` | Both of the above, parameters first |

**Parameter resolution:** Look up `name` in visible scope declarations. If found as `DeclType::Function`, `DeclType::FunctionDeclaration`, `DeclType::Procedure`, or `DeclType::ProcedureDeclaration`, iterate `declaration.parameters` and filter against `collect_used_param_names`. Convert each remaining parameter to a `CompletionItem` using the existing `declaration_to_completion`.

**Overloaded subprograms:** If multiple declarations with the same name exist (overloads), union all their parameter sets. Duplicates (same param name) are deduplicated by name.

---

## Phase 2: Missing-Param Filtering for Instantiation Completion

Reuse `collect_used_param_names` in the existing `PortMapLhs` and `GenericMapLhs` handlers. The open-paren offset is derived from the relevant `port_map_aspect` or `generic_map_aspect` node's text.

After collecting suggestions from the entity/component declaration, filter out any port or generic whose name is already in the used-names set.

This is a pure addition to existing handlers — no new context variants, no structural changes.

---

## Known Limitation: `is_rhs_of_association` and Nested Commas

The existing `is_rhs_of_association` uses `rfind(',')` and `rfind('(')` which can be confused by commas inside nested aggregates like `func(param_a => (a, b), |)`. This is a pre-existing issue not introduced by this feature. It is documented here but not fixed in this scope.

---

## Testing Requirements

Testing must be thorough. The following cases must all have explicit tests.

### `collect_used_param_names` Unit Tests

| Input text (from `(` to cursor) | Expected result |
|---|---|
| `(` (empty, no content) | `{}` |
| `(  ` (only whitespace) | `{}` |
| `(a => x, ` | `{"a"}` |
| `(a => x, b => y, ` | `{"a", "b"}` |
| `(a => (others => '0'), ` | `{"a"}` — `others` ignored (depth 2) |
| `(a => (0 => '1', 1 => '0'), ` | `{"a"}` — array aggregate `=>` ignored |
| `(a => inner_func(x => y), ` | `{"a"}` — `x` is inside inner call (depth 2) |
| `(PARAM_A => x, ` | `{"param_a"}` — case-insensitive |
| `(a => x, b => (others => '0'), ` | `{"a", "b"}` |
| `(x, y, ` (positional, no `=>`) | `{}` |

### Context Detection Integration Tests

Each test provides a VHDL snippet with a cursor marker (`|`) and asserts the returned `CompletionContext`.

| Scenario | Context |
|---|---|
| `func(\|)` — empty parens | `SubprogramCallBoth("func")` |
| `func(  \|  )` — whitespace only | `SubprogramCallBoth("func")` |
| `func(\|a => x)` — cursor before first named | `SubprogramCallLhs("func")` |
| `func(a => \|)` — cursor after `=>` | `SubprogramCallRhs` |
| `func(a => x, \|)` — after comma, named mode | `SubprogramCallLhs("func")` |
| `func(a => x, b => \|)` — after `=>` in second arg | `SubprogramCallRhs` |
| `func(x, \|)` — positional, no `=>` | `SubprogramCallRhs` |
| `func(x, y, \|)` — multiple positional | `SubprogramCallRhs` |
| `func(a => (others => '0'), \|)` — aggregate RHS | `SubprogramCallLhs("func")` |
| `func(a => (others => '0'), b => \|)` — after `=>` past aggregate | `SubprogramCallRhs` |
| `proc_call(a => \|` — no closing paren (broken AST) | `SubprogramCallRhs` |
| `func_a(func_b(\|))` — nested call, cursor in inner | `SubprogramCallBoth("func_b")` — innermost wins |
| `func_a(func_b(a => \|))` — nested, RHS of inner | `SubprogramCallRhs` — scoped to `func_b` |
| `func_a(func_b(a => x, \|))` — nested, LHS of inner after one arg | `SubprogramCallLhs("func_b")` |

### Completion Item Tests

These test the full pipeline: context → lookup → filter → items returned.

| Scenario | Items offered |
|---|---|
| `func(\|)`, func has params `a`, `b`, `c` | LHS: `a`, `b`, `c`; RHS: in-scope values |
| `func(a => x, \|)`, func has `a`, `b`, `c` | LHS: `b`, `c` only (`a` filtered) |
| `func(a => x, b => y, \|)` | LHS: `c` only |
| `func(a => x, b => y, c => x, \|)` | LHS: empty (all used) |
| `func(a => (others => '0'), \|)` | LHS: `b`, `c` (`a` filtered, `others` not) |
| `func(x, \|)` — positional | RHS only, no param names |
| Overloaded `func`: sig1 has `a,b`, sig2 has `a,c` | LHS: `a`, `b`, `c` (union, deduped) |

### Phase 2: Instantiation Filtering Tests

| Scenario | Items offered |
|---|---|
| `port map (clk => \|)` connected | `clk` filtered from LHS suggestions |
| `port map (clk => sys_clk, data => \|)` | `clk` filtered, `data` present |
| `port map (data => (others => '0'), \|)` | `data` filtered, `others` not |
| `generic map (WIDTH => 8, \|)` | `WIDTH` filtered |

---

## Implementation Touchpoints

| File | Change |
|---|---|
| `src/backend/features/completion/mod.rs` | New `CompletionContext` variants, new helpers `classify_call_args`, `collect_used_param_names`; extend `handle_upward_traversal` and `handle_map_node` |
| `src/backend/features/completion/mod.rs` | Extend main dispatch (`get_completion_items` or equivalent) for new context variants |
| `src/backend/features/completion/mod.rs` | Filter used params in existing `PortMapLhs`/`GenericMapLhs` handlers |
| `src/backend/features/completion/tests.rs` | All test cases above |
