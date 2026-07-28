# End-to-End LSP Integration Tests

**Date:** 2026-07-27
**Branch:** `feat/direct-instantiation-libraries` (or a follow-up branch)
**Scope:** A checked-in fixture VHDL project plus a Rust harness that drives the real server binary over stdio and asserts on protocol responses.

---

## Motivation

Every feature in 0.6.6 and 0.7.0 was verified two ways: unit tests over hand-built
`AnalysisMap` values, and throwaway Python scripts that drove the real binary. The unit
tests are permanent but never exercise the server. The Python scripts exercised the
server but died with the session.

That gap is where the interesting bugs live. Three concrete examples from this work:

- The 0.6.6 buffer guard could only be proven by driving the binary — 465 unit tests
  passed happily while completion was dead in a real editor.
- Task 5's deliverable (JIT deep-parse on `didOpen`) has **no** automated coverage at
  all, because `ensure_dependencies_loaded` needs a live `tower_lsp::Client`.
- Tasks 5, 6 and 7 were implemented by agents that never saw each other's code. All
  516 unit tests pass, but their *combination* in a running server is untested.

This suite makes that class of verification permanent and automatic.

---

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Location | `tests/`, runs under plain `cargo test` | CI already runs `cargo test` on every push and PR — no workflow changes. Regressions are caught by default rather than when someone remembers. |
| Client | Hand-rolled over `serde_json` | Keeps the test an independent observer of the wire protocol. Using `lsp-types` would share type definitions with the server, so a wrong type would be wrong in both places and the test would agree with the bug. |
| Coverage | Regression-anchored | Every scenario traces to a shipped bug or a seam unit tests cannot reach. No speculative coverage. |
| Fixtures | Checked in, read-only | No scenario writes to disk — edits are in-memory `didChange`. Removes temp-dir copying entirely. |
| Isolation | One server process per scenario | Fixtures are tiny, indexing is ~100 ms. Buys full isolation for negligible cost and lets tests run in parallel. |

---

## Anti-Flake Design

Flaky tests in the default `cargo test` run would poison CI for everyone, so this is
the load-bearing part of the design. Three mechanisms, in order of importance:

**1. Use the server's own barrier.** `did_open` (`src/backend/mod.rs:430`) already
blocks until workspace indexing completes:

```rust
let mut rx = self.indexing_rx.clone();
while !*rx.borrow_and_update() {
    if rx.changed().await.is_err() { break; }
}
```

So the first `didOpen` is a real synchronisation point. No sleep is needed to wait for
indexing, and none should be added.

**2. Retry until the condition holds, never sleep-then-assert.** Every assertion goes
through one helper:

```rust
fn retry_until<T>(deadline: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T>
```

It re-issues the request until the condition is satisfied or the deadline passes.
In the healthy case this returns on the first attempt in milliseconds; it is only slow
when something is genuinely broken. A fixed `sleep(2)` is the opposite: always slow, and
still flaky on a loaded CI runner.

**3. No shared mutable state.** Each scenario spawns its own server against a read-only
fixture directory, so parallel execution is safe.

**Timeouts:** 15 s per assertion, 30 s per scenario. Generous enough for a cold CI
runner, short enough that a hang fails rather than blocking the job.

**Failure output is part of the design.** On failure the harness prints the captured
`window/logMessage` stream and the last response received. A CI failure reading
`assertion failed: got []` with no context is close to useless for a protocol test.

---

## Layout

```
tests/
  lsp_integration.rs        harness + scenarios (single file to start)
  fixtures/
    multi_lib/
      oxide.toml            [libraries] rtl_lib = ["rtl/**/*.vhd"]
      rtl/uart_tx.vhd       entity uart_tx  -> rtl_lib
      rtl/top.vhd           architecture instantiating entity work.uart_tx
      tb/uart_tx.vhd        a SECOND entity also named uart_tx -> work
    plain/
      rtl/uart_tx.vhd       no oxide.toml: everything is `work`
      rtl/top.vhd
```

The duplicate `uart_tx` name in `multi_lib` is deliberate — it is the only way to prove
library scoping actually discriminates. A fixture with unique names would pass whether
or not the library dimension worked.

Each file in `tests/` is its own crate, so a single file is simplest to start. If it
outgrows one file, extract the harness to `tests/common/mod.rs`.

---

## Harness Interface

Roughly 150 lines. Public surface:

```rust
struct Lsp { /* child process, reader thread, captured logs, next id */ }

impl Lsp {
    /// Spawns the server against `fixture_dir`, completes initialize/initialized.
    fn start(fixture_dir: &Path) -> Lsp;

    /// Sends a request, blocks for the matching response id.
    fn request(&mut self, method: &str, params: Value) -> Value;

    /// Sends a notification (no response expected).
    fn notify(&mut self, method: &str, params: Value);

    /// didOpen with the given text. Blocks until the server acknowledges,
    /// which also means indexing has finished.
    fn open(&mut self, uri: &str, text: &str);

    /// didChange with full-document replacement text.
    fn change(&mut self, uri: &str, version: i64, text: &str);

    /// Completion labels at a position, retried until non-empty or deadline.
    fn completion_labels(&mut self, uri: &str, line: u32, ch: u32) -> Vec<String>;

    /// All window/logMessage text captured since `start`.
    fn logs(&self) -> Vec<String>;
}

impl Drop for Lsp { /* kill the child; never leak processes on panic */ }
```

`Drop` matters: a panicking assertion must not leave orphaned server processes behind
on a CI runner.

---

## Scenarios

Seven, each named for the behaviour it protects.

| # | Scenario | Asserts | Protects |
|---|---|---|---|
| 1 | `completion_survives_unclosed_if` | open healthy `top.vhd`, `didChange` to add `if c = '1' then` with no `end if;`, request completion in the body → signals still offered | 0.6.6 buffer guard |
| 2 | `instantiated_entity_is_deep_parsed_on_open` | open `top.vhd` only → log contains `JIT Parse completed` for `uart_tx.vhd` | Task 5, currently untested |
| 3 | `library_scoping_picks_the_right_entity` | in `multi_lib`, open `rtl/top.vhd` (library `rtl_lib`) which instantiates `entity work.uart_tx` → the JIT-parse log names `rtl/uart_tx.vhd`, **not** `tb/uart_tx.vhd` | `resolve_entity_uris` discrimination, via its only real consumer |
| 4 | `library_prefix_lists_only_that_library` | type `u0: entity rtl_lib.` → offers `rtl_lib`'s entities, excludes `work`-only ones | Task 6 |
| 5 | `work_prefix_resolves_to_current_library` | `entity work.` from a file in `rtl_lib` → offers `rtl_lib`'s entities | the `work` self-reference rule |
| 6 | `cross_file_entity_offers_direct_form_snippet` | completion in an architecture body → an item whose insert text starts `entity ` | Task 7 |
| 7 | `unconfigured_workspace_behaves_as_before` | same completion against the `plain` fixture → entity still resolvable with no `[libraries]` | the zero-regression guarantee |

Scenarios 1 and 2 are pure regression locks on shipped behaviour. Scenarios 3–7 cover
the 0.7.0 seams that unit tests reach only through hand-built maps.

---

## Cost

Seven scenarios × one spawn + index each. Expect **10–20 s** added to `cargo test`,
against 0.19 s today. That is the accepted price of the `tests/` placement decision —
the inner loop slows, and in exchange CI cannot merge a PR that breaks the server.

If it becomes annoying locally, `cargo test --bins` still runs only the fast unit tests.

---

## What Is Actually Library-Aware

Worth stating precisely, because it is narrower than "library support" suggests and the
scenarios must not assert behaviour that does not exist. `resolve_entity_uris` has exactly
one production caller. Library-awareness reaches three places:

| Library-aware | Mechanism |
|---|---|
| JIT deep-parse target selection | `ensure_dependencies_loaded` → `resolve_entity_uris` |
| `entity <lib>.` entity listing | `LibraryUnits` → `entities_in_library` |
| `entity ` library listing, and `work.` vs `lib.` snippet prefix | `known_libraries`, `owner.library` comparison |

Still **name-only**, unchanged by 0.7.0: go-to-definition, hover, port-map completion
(which merges ports from every same-named entity), references, rename.

A scenario asserting that goto discriminates by library would fail — correctly, because
nothing wires the resolver into `lookup_definition`. That is deferred work, not a defect
in this suite.

## Out of Scope

- Broad LSP surface coverage (hover, references, rename, document symbols). Most has
  unit coverage; add scenarios here only when a real bug escapes.
- Multi-root workspaces, `include_workspace`, vendor library paths.
- Performance or large-workspace benchmarks — different problem, different harness.
- Windows/macOS specifics. CI runs `cargo test` on ubuntu only; the harness uses no
  platform-specific behaviour beyond process spawning, but it is verified on Linux only.

---

## Risks

**The harness is test infrastructure, and untested test infrastructure lies.** A bug in
framing or response matching could make every scenario pass vacuously. Mitigation: each
scenario must be verified to *fail* against a deliberately broken condition before being
accepted — the same mutation discipline used on the 0.6.6 guard, where reverting the fix
had to fail exactly the three bug-catching tests.

**Timing assumptions may not hold on a cold CI runner.** Mitigated by retry-until rather
than fixed sleeps, and by generous deadlines. If a scenario still proves flaky, the fix
is a longer deadline or a better sync point — never a `sleep`.
