# Library-Aware Direct Entity Instantiation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Oxide HDL a first-class tool for codebases that use VHDL direct entity instantiation (`u0: entity rtl_lib.uart_tx port map (...)`) instead of component declarations — by adding a library dimension to the index, resolving instantiated entities through it, deep-parsing them on demand, and completing entity names after a library prefix.

**Architecture:** A `[libraries]` section in `oxide.toml` maps path globs to VHDL library names. Each `Analysis` is stamped with the library its file belongs to at index time. `Instance` stops discarding the library/architecture the grammar already gives us. A new `src/backend/units.rs` module provides pure query functions over `&AnalysisMap` (`resolve_entity_uris`, `entities_in_library`, `known_libraries`) that treat `work` as a self-reference to the current file's library and fall back to today's name-only search whenever a library is unconfigured. Completion and JIT parsing then consume those queries.

**Tech Stack:** Rust 2024 edition, tree-sitter VHDL grammar (`jpt13653903/tree-sitter-vhdl`), tower-lsp 0.20, globset, regex, lazy_static, tokio.

## Global Constraints

- **Zero regression when `[libraries]` is absent.** Every file defaults to library `work`, so `work.foo` resolves workspace-wide exactly as it does today. All 465 pre-existing tests must stay green after every task.
- **Never deep-parse speculatively at scale.** A library may hold 500+ entities. Entity-name completion must be served from the shallow index only. Deep parsing happens for entities actually instantiated in an open file, or on demand for port-map completion (which already works this way).
- **Conservative diagnostics.** This plan adds no new diagnostics. Unknown entities, missing ports, and bad architecture specs stay silent — that is deferred work, listed in "Out of Scope".
- **Library names are case-insensitive**, normalized to lowercase everywhere. VHDL identifiers are case-insensitive; the codebase already lowercases map keys.
- **Additive struct changes only.** `Analysis` and `Instance` are each constructed in exactly one place (`Analysis::new()` at `src/analysis/mod.rs:71`, `create_instance_from_node` in `src/analysis/builders.rs`), so adding fields ripples nowhere. Do not remove `Instance::component`.
- Run `cargo test` (not a filtered subset) before every commit.

---

## Verified Grammar Facts

These were confirmed by dumping `to_sexp()` against the vendored grammar. The extraction code in Task 3 depends on them being exactly right. **Do not assume symmetry — there is none.**

| Source | Shape |
|---|---|
| `u0: entity work.cpu` | `(component_instantiation_statement (label_declaration (label)) (instantiated_unit library: (library_namespace) entity: (name (identifier))))` |
| `u0: entity mylib.cpu` | `(component_instantiation_statement (label_declaration (label)) (instantiated_unit entity: (name (identifier) (selection (identifier)))))` |
| `u0: entity work.cpu(behavioral)` | `(instantiated_unit library: (library_namespace) entity: (name (identifier)) architecture: (identifier))` |
| `u0: entity mylib.cpu(rtl)` | `(instantiated_unit entity: (name (identifier) (selection (identifier))) architecture: (identifier))` — the `architecture:` field survives on the no-`library:` path |
| `u0: entity a.b.c` | `(instantiated_unit entity: (name (identifier) (selection (identifier)) (selection (identifier))))` |
| `u0: configuration work.cfg` | `(instantiated_unit configuration: (name (identifier) (selection (identifier))))` |
| `u0: component cpu` | `(instantiated_unit component: (name (identifier)))` |
| `u0: cpu` (plain) | `(component_instantiation_statement (label_declaration (label)) component: (name (identifier)))` — **no `instantiated_unit` node at all** |

Two traps:

1. **`work` is a keyword to this grammar, other libraries are not.** `work.cpu` puts `work` in a `library:` field and leaves `name` with a bare identifier. `mylib.cpu` has **no** `library:` field — `mylib` is the name's first `identifier` and `cpu` is inside a `selection`. Both paths must be handled.
2. **The plain form has no `instantiated_unit`.** The `component:` field hangs off the statement directly. This is why `create_instance_from_node` currently does `find_child(node, "instantiated_unit").unwrap_or(node)`.

3. **`work` is only a `library_namespace` after `entity`.** After `configuration` it is *not* — `configuration work.cfg` parses as `configuration: (name (identifier) (selection (identifier)))`, with `work` as the name's plain first identifier and no `library:` field at all. Same keyword, two tree shapes. The extraction below handles it by falling through to the name's leading identifier when no `library:` field is present; do not "simplify" that away on the assumption that `work` always produces a `library_namespace`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `src/config.rs` | `oxide.toml` schema + glob compilation. Gains the `[libraries]` schema and `LibraryMatcher`, which owns path→library resolution. | Modify |
| `src/analysis/mod.rs` | `Analysis` struct. Gains a `library` field. | Modify |
| `src/analysis/types.rs` | `Instance` struct + new `InstantiatedUnitKind` enum. | Modify |
| `src/analysis/builders.rs` | `create_instance_from_node` stops discarding library/architecture/kind. | Modify |
| `src/analysis/scope_tree.rs` | Gains `collect_all_instantiations`, mirroring the existing `collect_all_use_clauses`. | Modify |
| `src/backend/units.rs` | **New.** Pure design-unit queries over `&AnalysisMap`: entity resolution, per-library entity listing, library enumeration. No I/O, no locks, fully unit-testable. | Create |
| `src/backend/workspace.rs` | Stamps `Analysis.library` at index time and preserves it across deep parses. | Modify |
| `src/backend/mod.rs` | Registers `units` module; extends `ensure_dependencies_loaded` to JIT-parse instantiated entities. | Modify |
| `src/backend/features/completion/mod.rs` | Two new contexts (`InstantiationLibrary`, `LibraryUnits`), their detection and resolution; workspace-wide direct-form instantiation snippets. | Modify |
| `src/backend/features/completion/tests.rs` | Tests for the above. | Modify |
| `src/analysis/tests/builders_tests.rs` | Tests for `Instance` extraction across all seven grammar forms. | Modify |

`src/backend/units.rs` is a new file rather than an addition to `workspace.rs` (~700 lines, about indexing and I/O) because these are pure synchronous queries with no async or filesystem surface, and they need to be callable from completion without touching tokio.

---

## Out of Scope (deliberate, do not implement)

- **New diagnostics** — unknown entity, missing/unknown port in a direct instantiation, nonexistent architecture, duplicate entity in a library. All become *possible* after this plan; none are in it.
- **Library-aware port-map completion.** `PortMapLhs(String)` keeps a bare-name payload. Making it library-aware means changing the enum payload, which churns many tests in `completion/tests.rs`, and only changes behavior when two libraries hold a same-named entity. Follow-up work.
- **Architecture resolution.** Task 3 *captures* `Instance.architecture` because it is free, but nothing consumes it yet. Goto/validation on `(behavioral)` is follow-up.
- **Configuration instantiation.** Task 3 captures `InstantiatedUnitKind::Configuration`; no resolution is built for it.
- **Cross-file rename.** `src/backend/features/rename/mod.rs` is file-scoped by design and stays that way.
- **`completionItem/resolve`.** Not needed — see Task 6, where snippets are emitted only for already-deep-parsed entities.

## Known Pre-existing Gaps (verified, deliberately not fixed here)

Both were found while validating this plan. Neither is introduced by it, and both affect
component and direct instantiation equally, so fixing them inside a direct-instantiation
branch would blur what the branch is for.

- **Context misdetection on an unclosed `(`.** With an open `port map (` followed by
  `end architecture;`, `get_completion_context` returns `Architecture` instead of
  `PortMapLhs`, so scope items are offered where ports should be. Most editors auto-close
  the paren, which lands you in the working case, limiting how often this bites.

- **No partial scope-tree recovery.** Any unclosed construct makes `extract_document_symbols`
  return zero scope trees. The 0.6.6 buffer guard stops that from *destroying* the stored
  analysis, but the freshly typed text still contributes nothing until it parses. Real
  recovery — building partial trees from broken ASTs in `builders.rs` — would fix both this
  and the item above, and is a substantial piece of work in its own right.

---

## Task 1: Library Configuration and Path Matching

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces:
  - `OxideConfig.libraries: HashMap<String, Vec<String>>` — deserialized from `[libraries]`.
  - `OxideConfig.root: PathBuf` — `#[serde(skip)]`, set by `load()`, empty for `default()`.
  - `pub struct LibraryMatcher` with:
    - `pub fn new(entries: Vec<(String, Vec<String>)>, root: PathBuf) -> Self`
    - `pub fn from_config(config: &OxideConfig) -> Self`
    - `pub fn library_for(&self, path: &Path) -> String` — returns a lowercase library name, or `"work"`.
  - `LibraryMatcher` derives `Clone` and `Debug`.

The `oxide.toml` schema this adds:

```toml
[libraries]
rtl_lib = ["rtl/**/*.vhd", "common/**/*.vhd"]
unisim    = ["/opt/Xilinx/**/unisims/**/*.vhd"]
```

Each glob is matched twice: against the path made relative to `root`, and against the full absolute path. The absolute attempt is what makes vendor libraries outside the workspace work. When several libraries match one file, the alphabetically-first library name wins — `HashMap` iteration order is nondeterministic, so sorting at construction is what makes the result stable.

- [ ] **Step 1: Write the failing tests**

Append to the end of `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn matcher(entries: &[(&str, &[&str])], root: &str) -> LibraryMatcher {
        LibraryMatcher::new(
            entries
                .iter()
                .map(|(name, globs)| {
                    (
                        name.to_string(),
                        globs.iter().map(|g| g.to_string()).collect(),
                    )
                })
                .collect(),
            PathBuf::from(root),
        )
    }

    #[test]
    fn test_no_libraries_configured_yields_work() {
        let m = matcher(&[], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "work");
    }

    #[test]
    fn test_relative_glob_match() {
        let m = matcher(&[("rtl_lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(
            m.library_for(&PathBuf::from("/ws/rtl/core/cpu.vhd")),
            "rtl_lib"
        );
    }

    #[test]
    fn test_non_matching_path_falls_back_to_work() {
        let m = matcher(&[("rtl_lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/tb/cpu_tb.vhd")), "work");
    }

    #[test]
    fn test_absolute_glob_matches_outside_workspace() {
        let m = matcher(&[("unisim", &["/opt/Xilinx/**/unisims/**/*.vhd"])], "/ws");
        assert_eq!(
            m.library_for(&PathBuf::from("/opt/Xilinx/2024/data/unisims/prims/BUFG.vhd")),
            "unisim"
        );
    }

    #[test]
    fn test_library_name_is_lowercased() {
        let m = matcher(&[("Rtl_Lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "rtl_lib");
    }

    #[test]
    fn test_ambiguous_match_picks_alphabetically_first() {
        // Both patterns match; "alpha" must win deterministically over "zeta".
        let m = matcher(
            &[("zeta", &["rtl/**/*.vhd"]), ("alpha", &["rtl/**/*.vhd"])],
            "/ws",
        );
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "alpha");
    }

    #[test]
    fn test_star_does_not_cross_directory_separator() {
        // globset's `*` crosses `/` by DEFAULT. For library assignment that is a
        // silent-misassignment footgun, so library globs are built with
        // `literal_separator(true)`. `rtl/*.vhd` must mean "directly in rtl/".
        let m = matcher(&[("rtl_lib", &["rtl/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/top.vhd")), "rtl_lib");
        assert_eq!(
            m.library_for(&PathBuf::from("/ws/rtl/core/cpu.vhd")),
            "work",
            "`*` must not descend into subdirectories"
        );
    }

    #[test]
    fn test_double_star_matches_zero_directories() {
        // `rtl/**/*.vhd` must also match a file sitting directly in rtl/, otherwise
        // every top-level file silently falls into `work`.
        let m = matcher(&[("rtl_lib", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/top.vhd")), "rtl_lib");
        assert_eq!(
            m.library_for(&PathBuf::from("/ws/rtl/a/b/deep.vhd")),
            "rtl_lib"
        );
    }

    #[test]
    fn test_invalid_glob_is_skipped_not_panicking() {
        // "[" is an unterminated character class — must be skipped silently.
        let m = matcher(&[("broken", &["["]), ("good", &["rtl/**/*.vhd"])], "/ws");
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "good");
    }

    #[test]
    fn test_default_config_has_empty_libraries() {
        let c = OxideConfig::default();
        assert!(c.libraries.is_empty());
        assert_eq!(c.root, PathBuf::new());
    }

    #[test]
    fn test_from_config_reads_libraries_table() {
        let toml_src = r#"
[libraries]
rtl_lib = ["rtl/**/*.vhd"]
"#;
        let mut c: OxideConfig = toml::from_str(toml_src).unwrap();
        c.root = PathBuf::from("/ws");
        let m = LibraryMatcher::from_config(&c);
        assert_eq!(m.library_for(&PathBuf::from("/ws/rtl/cpu.vhd")), "rtl_lib");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test config:: 2>&1 | tail -20`

Expected: compile errors — `LibraryMatcher` not found, no field `libraries`, no field `root`.

- [ ] **Step 3: Add the config fields**

In `src/config.rs`, add `use std::collections::HashMap;` to the imports at the top, extend `use std::path::Path;` to `use std::path::{Path, PathBuf};`, and extend the globset import to include `GlobBuilder`:

```rust
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
```

`Glob` stays — `build_globset()` still uses it.

Add these two fields to `OxideConfig`, after `include_workspace`:

```rust
    /// Maps VHDL library names to the path globs whose files belong to that library.
    ///
    /// Globs are matched against the path relative to the workspace root and, failing
    /// that, against the absolute path — the latter is what lets vendor libraries
    /// outside the workspace be declared. Files matching nothing belong to `work`.
    ///
    /// ```toml
    /// [libraries]
    /// rtl_lib = ["rtl/**/*.vhd"]
    /// unisim    = ["/opt/Xilinx/**/unisims/**/*.vhd"]
    /// ```
    #[serde(default)]
    pub libraries: HashMap<String, Vec<String>>,

    /// Workspace root, captured by `load()`. Not part of the TOML schema.
    /// Used to resolve relative library globs. Empty for `default()`.
    #[serde(skip)]
    pub root: PathBuf,
```

Add both to the `OxideConfig::default()` literal:

```rust
            libraries: HashMap::new(),
            root: PathBuf::new(),
```

And in `load()`, set `root` on both branches so it is always populated:

```rust
    pub fn load(root_path: &Path) -> Self {
        let config_path = root_path.join("oxide.toml");

        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(config_path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_else(|_| Self::default())
        } else {
            Self::default()
        };
        config.root = root_path.to_path_buf();
        config
    }
```

- [ ] **Step 4: Implement `LibraryMatcher`**

Add at the end of `src/config.rs`, before the `#[cfg(test)] mod tests` block:

```rust
/// Resolves which VHDL library a source file belongs to, based on the
/// `[libraries]` globs from `oxide.toml`.
///
/// Entries are sorted by library name at construction so that a file matching
/// several libraries always resolves to the same one (the alphabetically first).
/// Files matching no library belong to `work`, which is also the behaviour when
/// no `[libraries]` section is present at all.
#[derive(Debug, Clone)]
pub struct LibraryMatcher {
    /// (lowercase library name, compiled globs), sorted by name.
    entries: Vec<(String, GlobSet)>,
    root: PathBuf,
}

impl LibraryMatcher {
    /// Builds a matcher from raw (library name, globs) pairs.
    ///
    /// Invalid glob patterns are skipped silently, consistent with `build_globset`.
    ///
    /// # Arguments
    /// * `entries` - Library name paired with its list of glob patterns.
    /// * `root` - Workspace root, used to relativize paths before matching.
    pub fn new(entries: Vec<(String, Vec<String>)>, root: PathBuf) -> Self {
        let mut compiled: Vec<(String, GlobSet)> = entries
            .into_iter()
            .filter_map(|(name, globs)| {
                let mut builder = GlobSetBuilder::new();
                for pattern in &globs {
                    // `literal_separator(true)` stops `*` from crossing `/`, which is
                    // globset's default. Without it `rtl/*.vhd` also matches
                    // `rtl/a/b/deep.vhd`, so two libraries with patterns the user
                    // believes are disjoint would both match and be silently resolved
                    // by the alphabetical tie-break. `**` still spans directories,
                    // including zero of them.
                    //
                    // NOTE: `build_globset()` above deliberately keeps the loose
                    // default. Tightening it would change which files existing users
                    // have indexed; `[libraries]` is new, so it has no such history.
                    if let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() {
                        builder.add(glob);
                    }
                }
                builder.build().ok().map(|set| (name.to_lowercase(), set))
            })
            .collect();
        compiled.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            entries: compiled,
            root,
        }
    }

    /// Builds a matcher from a loaded configuration.
    pub fn from_config(config: &OxideConfig) -> Self {
        let entries = config
            .libraries
            .iter()
            .map(|(name, globs)| (name.clone(), globs.clone()))
            .collect();
        Self::new(entries, config.root.clone())
    }

    /// Returns the lowercase library name owning `path`, or `"work"`.
    ///
    /// Each glob set is tried against the workspace-relative path first, then
    /// against the absolute path so that vendor libraries outside the workspace
    /// can be declared with absolute globs.
    pub fn library_for(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.root).ok();
        for (name, set) in &self.entries {
            if let Some(rel) = relative
                && set.is_match(rel)
            {
                return name.clone();
            }
            if set.is_match(path) {
                return name.clone();
            }
        }
        "work".to_string()
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test config:: 2>&1 | tail -20`

Expected: 11 tests pass.

- [ ] **Step 6: Run the full suite**

Run: `cargo test 2>&1 | tail -5`

Expected: `465 passed` plus the 11 new = `476 passed; 0 failed`.

> **Expected transient warnings.** This task only *produces* the `LibraryMatcher`
> API; its first production consumer arrives in Task 2. Until then the non-test build
> reports three `dead_code` warnings (`libraries` never read, `LibraryMatcher` never
> constructed, its methods never used). That is correct and expected — **do not silence
> them with `#[allow(dead_code)]`**, or the attribute will outlive its reason and hide
> genuinely dead code later. They clear themselves when Task 2 lands.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "feat: add [libraries] config section and LibraryMatcher path resolution"
```

---

## Task 2: Stamp Each `Analysis` With Its Library

**Files:**
- Modify: `src/analysis/mod.rs`
- Modify: `src/analysis/tests/mod.rs`
- Modify: `src/backend/workspace.rs`
- Modify: `src/backend/mod.rs` (one `ensure_fully_parsed` call site)

**Interfaces:**
- Consumes: `LibraryMatcher::from_config`, `LibraryMatcher::library_for` (Task 1).
- Produces:
  - `Analysis.library: String` — lowercase library name, `"work"` by default. Every `Analysis` in the `AnalysisMap` carries a correct value regardless of which code path created it.
  - `pub fn analysis_for_file(text: &str, path: &Path, matcher: &LibraryMatcher) -> Analysis` in `src/backend/workspace.rs` — shallow-scans `text` and stamps the library. Pure: no I/O, no locks, no `Client`.
  - `ensure_fully_parsed` gains a `matcher: &LibraryMatcher` parameter.

Three paths write an `Analysis` into the map. **All three compute the library the same way** — from the matcher, using the file's path:

| Path | Location |
|---|---|
| Shallow index | `index_workspace` (the `join_set.spawn` closure) |
| Deep parse on open/change | `parse_and_update_document` (Phase 3) |
| JIT upgrade | `ensure_fully_parsed` (final block) |

An earlier draft of this plan special-cased the third path — `ensure_fully_parsed` has no `config`, so it would have carried the library over from the existing map entry instead. That was a trap: it produces no compile error and no test failure, so an implementer who skipped it would ship silently-wrong resolution for exactly the files that got JIT-upgraded. Threading the matcher through instead costs four call-site updates and removes the failure mode entirely. Take the uniform version.

The `analysis_for_file` helper exists for the same reason. Without it, nothing tests that indexing actually *calls* the matcher — every test downstream sets `analysis.library` by hand, so the seam between Task 1 and Task 4 would be completely uncovered. `index_workspace` itself can't be unit-tested (it needs a live `Client` and a real directory tree), but the per-file step factored out of it can.

- [ ] **Step 1: Write the failing tests**

Add to `src/analysis/tests/mod.rs`, at the end of the file:

```rust
#[test]
fn test_analysis_defaults_to_work_library() {
    let analysis = crate::analysis::Analysis::new();
    assert_eq!(analysis.library, "work");
}

#[test]
fn test_analysis_library_survives_clone() {
    let mut analysis = crate::analysis::Analysis::new();
    analysis.library = "rtl_lib".to_string();
    let copy = analysis.clone();
    assert_eq!(copy.library, "rtl_lib");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -- test_analysis_defaults_to_work_library test_analysis_library_survives_clone 2>&1 | tail -20`

Expected: compile error — no field `library` on `Analysis`.

- [ ] **Step 3: Add the field**

In `src/analysis/mod.rs`, add to the `Analysis` struct after `parse_level`:

```rust
    /// VHDL library this file's design units belong to, lowercase.
    ///
    /// Resolved from the `[libraries]` globs in `oxide.toml` at index time.
    /// Defaults to `work`, which is also what every file gets when no libraries
    /// are configured — making library-aware resolution a no-op for such workspaces.
    pub library: String,
```

And to the `Self { .. }` literal in `Analysis::new()`:

```rust
            library: "work".to_string(),
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -- test_analysis_defaults_to_work_library test_analysis_library_survives_clone 2>&1 | tail -10`

Expected: 2 passed.

- [ ] **Step 5: Write the failing test for `analysis_for_file`**

`src/backend/workspace.rs` **already has** a `#[cfg(test)] mod tests` block (added by the 0.6.6 buffer-guard fix). Add these tests *inside* it — do not declare a second `mod tests`. Add `use crate::config::LibraryMatcher;` and `use std::path::PathBuf;` to that module's existing imports, then append:

```rust

    fn matcher() -> LibraryMatcher {
        LibraryMatcher::new(
            vec![("rtl_lib".to_string(), vec!["rtl/**/*.vhd".to_string()])],
            PathBuf::from("/ws"),
        )
    }

    #[test]
    fn test_analysis_for_file_stamps_matched_library() {
        let src = "entity uart_tx is\nend entity;\n";
        let a = super::analysis_for_file(src, &PathBuf::from("/ws/rtl/uart_tx.vhd"), &matcher());
        assert_eq!(a.library, "rtl_lib");
    }

    #[test]
    fn test_analysis_for_file_defaults_unmatched_to_work() {
        let src = "entity uart_tx is\nend entity;\n";
        let a = super::analysis_for_file(src, &PathBuf::from("/ws/tb/uart_tb.vhd"), &matcher());
        assert_eq!(a.library, "work");
    }

    #[test]
    fn test_analysis_for_file_still_populates_symbols() {
        // The library stamp must not disturb the shallow scan it wraps.
        let src = "entity uart_tx is\nend entity;\n";
        let a = super::analysis_for_file(src, &PathBuf::from("/ws/rtl/uart_tx.vhd"), &matcher());
        assert!(
            a.symbols.contains_key("uart_tx"),
            "shallow scan results lost, got keys: {:?}",
            a.symbols.keys().collect::<Vec<_>>()
        );
    }
```

Note the existing module already defines a helper named `analyze`; the `matcher()` helper added here does not collide with it.

Run: `cargo test workspace::tests 2>&1 | tail -20`

Expected: compile error — `analysis_for_file` not found.

- [ ] **Step 6: Implement `analysis_for_file` and use it in the indexer**

In `src/backend/workspace.rs`, extend the imports:

```rust
use crate::config::{LibraryMatcher, OxideConfig};
```

(replacing the existing `use crate::config::OxideConfig;`), and add `use std::path::Path;`.

Add the helper above `index_workspace`:

```rust
/// Shallow-scans one file's text and stamps the library it belongs to.
///
/// Factored out of `index_workspace` so the scan-and-stamp step is unit-testable:
/// `index_workspace` itself needs a live `Client` and a real directory tree, but
/// this does not. Pure — no I/O, no locks.
///
/// # Arguments
/// * `text` - File contents.
/// * `path` - Path on disk, used only to resolve the library.
/// * `matcher` - Compiled `[libraries]` globs.
pub fn analysis_for_file(text: &str, path: &Path, matcher: &LibraryMatcher) -> Analysis {
    let mut analysis = Analysis::new();
    analysis.library = matcher.library_for(path);
    for s in scanner::scan_fast(text) {
        analysis.symbols.insert(s.name.clone().to_lowercase(), s);
    }
    analysis
}
```

In `index_workspace`, immediately after `let matcher = config.build_globset();`, add:

```rust
    let lib_matcher = Arc::new(LibraryMatcher::from_config(&config));
```

Add a clone just above the spawn, next to `let sem_clone = semaphore.clone();`:

```rust
        let lib_clone = lib_matcher.clone();
```

Then replace the body of the `join_set.spawn` closure with:

```rust
        join_set.spawn(async move {
            let _permit = sem_clone.acquire_owned().await.unwrap();
            tokio::task::spawn_blocking(move || {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                (path_uri, analysis_for_file(&text, &path, &lib_clone))
            })
            .await
            .unwrap()
        });
```

Run: `cargo test workspace::tests 2>&1 | tail -10`

Expected: 3 passed.

- [ ] **Step 7: Stamp during deep parse**

> **Note:** the 0.6.6 bugfix replaced the old inline `map.insert` here with a call to
> `store_analysis`, which guards against clobbering a good analysis with an empty one
> while the buffer is mid-edit. Stamp the library *before* that call — do not reintroduce
> a raw `map.insert`, and do not put the stamping inside `store_analysis` (that function
> is deliberately free of config dependencies).

In `parse_and_update_document`, build the matcher once at the top of the function, just after the `let uri = uri.clone();` line (it is needed again in Step 8):

```rust
    let lib_matcher = LibraryMatcher::from_config(&config);
```

Then replace the Phase 3 block:

```rust
    // Phase 3: Store analysis in map (skipped if the buffer is mid-edit and
    // momentarily unparseable — see `store_analysis`).
    store_analysis(&analysis_map, &uri, analysis.clone()).await;
```

with:

```rust
    // Phase 3: Stamp the library, then store (the store is skipped if the buffer is
    // mid-edit and momentarily unparseable — see `store_analysis`). A fresh Analysis
    // defaults to "work" and would otherwise clobber what the indexer resolved.
    let mut analysis = analysis;
    if let Ok(path) = uri.to_file_path() {
        analysis.library = lib_matcher.library_for(&path);
    }
    store_analysis(&analysis_map, &uri, analysis.clone()).await;
```

Note the `let mut analysis = analysis;` rebinding — the existing binding earlier in the function is immutable and is moved into `analysis_for_diag` further down, so rebinding here leaves the later code untouched.

- [ ] **Step 8: Thread the matcher through `ensure_fully_parsed`**

Change the signature to take the matcher:

```rust
pub async fn ensure_fully_parsed(
    client: &Client,
    analysis_map: &Arc<RwLock<AnalysisMap>>,
    parser: &Arc<Mutex<crate::backend::Parser>>,
    uri: &Url,
    matcher: &LibraryMatcher,
) {
```

Then stamp in its final block, replacing:

```rust
        let mut map = analysis_map.write().await;
        map.insert(uri.clone(), analysis);
```

with:

```rust
        // A JIT upgrade replaces the shallow Analysis wholesale, so the library must
        // be recomputed — the fresh Analysis defaults to "work" and would otherwise
        // silently clobber what the indexer resolved for this file.
        let mut analysis = analysis;
        if let Ok(path) = uri.to_file_path() {
            analysis.library = matcher.library_for(&path);
        }
        let mut map = analysis_map.write().await;
        map.insert(uri.clone(), analysis);
```

The `if let Some(analysis) = result {` binding above is already a fresh owned value, so the rebinding compiles.

Now update all call sites. Find them with:

```bash
grep -rn "ensure_fully_parsed(" --include='*.rs' src | grep -v "pub async fn"
```

There are **four live sites** today (plus one inside a commented-out block — ignore it), and Task 5 adds a fifth. Anchor on the enclosing function, not the line number, since these drift:

| Enclosing function | File | How to get a matcher |
|---|---|---|
| `parse_and_update_document` — use-clause packages | `workspace.rs` | `&lib_matcher` from Step 7 |
| `parse_and_update_document` — inner-scope use clauses | `workspace.rs` | `&lib_matcher` from Step 7 |
| `Backend::hover` | `backend/mod.rs` | build one locally, see below |
| `Backend::completion` | `backend/mod.rs` | build one locally, see below |
| Task 5's new loop in `ensure_dependencies_loaded` | `backend/mod.rs` | build one locally, see below |

For the `backend/mod.rs` sites, build the matcher from the stored config. **Mind the nesting:** in `Backend::hover` it belongs inside the `if !needs_jit.is_empty() {` block, and in `Backend::completion` inside the `if let Some(def_uri) = def_uri {` block. Keep the inner braces shown below — they drop the config read-guard before the `ensure_fully_parsed` await, and removing them risks holding a lock across an await.

```rust
            let lib_matcher = {
                let config_guard = self.config.read().await;
                crate::config::LibraryMatcher::from_config(
                    &config_guard.clone().unwrap_or_else(OxideConfig::default),
                )
            };
```

then pass `&lib_matcher`.

- [ ] **Step 9: Verify the whole suite still passes**

Run: `cargo test 2>&1 | tail -5`

Expected: `481 passed; 0 failed`.

- [ ] **Step 10: Commit**

```bash
git add src/analysis/mod.rs src/analysis/tests/mod.rs src/backend/workspace.rs src/backend/mod.rs
git commit -m "feat: stamp each Analysis with its VHDL library at index time"
```

---

## Task 3: Capture Library, Architecture and Unit Kind on `Instance`

**Files:**
- Modify: `src/analysis/types.rs`
- Modify: `src/analysis/builders.rs` (`create_instance_from_node`)
- Modify: `src/analysis/tests/builders_tests.rs`

**Interfaces:**
- Consumes: nothing from prior tasks.
- Produces:
  - `pub enum InstantiatedUnitKind { Entity, Component, Configuration }` — derives `Debug, Clone, Copy, PartialEq, Eq`.
  - Three new `Instance` fields:
    - `pub library: Option<String>` — lowercase; `None` for the plain/`component` forms which carry no library.
    - `pub architecture: Option<String>` — lowercase; from `entity work.cpu(behavioral)`.
    - `pub unit_kind: InstantiatedUnitKind`.
  - `Instance.component` is **unchanged** — still the bare unit name. Existing readers at `src/backend/features/symbol/mod.rs:54` and `src/backend/features/lookup/mod.rs:151` keep working untouched.

Re-read "Verified Grammar Facts" above before writing the extraction. The `work` vs `mylib` asymmetry is the whole difficulty here.

- [ ] **Step 1: Write the failing tests**

Add to `src/analysis/tests/builders_tests.rs`, at the end of the file:

```rust
// =========================================================================
// Instantiated unit: library / architecture / kind extraction
// =========================================================================

/// Helper: parse an architecture body and return its single instantiation.
fn single_instantiation(code: &str) -> crate::analysis::Instance {
    let tree = parse_text(code);
    let analysis = extract_document_symbols(code, tree.root_node());
    assert_eq!(
        analysis.scope_trees[0].instantiations.len(),
        1,
        "expected exactly one instantiation in: {code}"
    );
    analysis.scope_trees[0].instantiations[0].clone()
}

#[test]
fn test_inst_unit_entity_work() {
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: entity work.cpu port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cpu");
    assert_eq!(inst.library.as_deref(), Some("work"));
    assert_eq!(inst.architecture, None);
    assert_eq!(inst.unit_kind, crate::analysis::InstantiatedUnitKind::Entity);
}

#[test]
fn test_inst_unit_entity_named_library() {
    // `mylib` is NOT a library_namespace node — it is the name's first identifier,
    // with `cpu` inside a selection. This is the asymmetry vs `work`.
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: entity mylib.cpu port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cpu");
    assert_eq!(inst.library.as_deref(), Some("mylib"));
    assert_eq!(inst.unit_kind, crate::analysis::InstantiatedUnitKind::Entity);
}

#[test]
fn test_inst_unit_entity_with_architecture() {
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: entity work.cpu(behavioral) port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cpu");
    assert_eq!(inst.library.as_deref(), Some("work"));
    assert_eq!(inst.architecture.as_deref(), Some("behavioral"));
}

#[test]
fn test_inst_unit_named_library_with_architecture() {
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: entity mylib.cpu(rtl) port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cpu");
    assert_eq!(inst.library.as_deref(), Some("mylib"));
    assert_eq!(inst.architecture.as_deref(), Some("rtl"));
}

#[test]
fn test_inst_unit_preserves_source_case() {
    // The codebase convention is: HashMap keys are lowercased, struct fields hold
    // the source text verbatim, and comparisons normalize at the comparison site
    // (23 uses of `eq_ignore_ascii_case` across 9 files). All three fields follow it.
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: entity MyLib.Cpu(Behavioral) port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "Cpu");
    assert_eq!(inst.library.as_deref(), Some("MyLib"));
    assert_eq!(inst.architecture.as_deref(), Some("Behavioral"));
}

#[test]
fn test_inst_unit_plain_component() {
    // No instantiated_unit node at all — `component:` hangs off the statement.
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: cpu port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cpu");
    assert_eq!(inst.library, None);
    assert_eq!(
        inst.unit_kind,
        crate::analysis::InstantiatedUnitKind::Component
    );
}

#[test]
fn test_inst_unit_explicit_component_keyword() {
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: component cpu port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cpu");
    assert_eq!(inst.library, None);
    assert_eq!(
        inst.unit_kind,
        crate::analysis::InstantiatedUnitKind::Component
    );
}

#[test]
fn test_inst_unit_configuration() {
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: configuration work.cfg port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "cfg");
    assert_eq!(inst.library.as_deref(), Some("work"));
    assert_eq!(
        inst.unit_kind,
        crate::analysis::InstantiatedUnitKind::Configuration
    );
}

#[test]
fn test_inst_unit_three_part_name_takes_last_segment() {
    // `a.b.c` yields a name with two selection children; the unit is the last.
    let inst = single_instantiation(
        "architecture rtl of t is begin u0: entity a.b.c port map (clk => clk); end architecture;",
    );
    assert_eq!(inst.component, "c");
    assert_eq!(inst.library.as_deref(), Some("a"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test test_inst_unit 2>&1 | tail -20`

Expected: compile errors — no `InstantiatedUnitKind`, no field `library` on `Instance`.

- [ ] **Step 3: Add the enum and the fields**

In `src/analysis/types.rs`, add above the `Instance` struct:

```rust
/// Which flavour of instantiation statement produced an [`Instance`].
///
/// VHDL allows three: a direct entity instantiation (`entity lib.name`), a
/// component instantiation (bare `name`, or the explicit `component name`
/// form), and a configuration instantiation (`configuration lib.name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantiatedUnitKind {
    /// `u0: entity work.cpu` — resolves against entity declarations.
    Entity,
    /// `u0: cpu` or `u0: component cpu` — resolves against component declarations.
    Component,
    /// `u0: configuration work.cfg` — not resolved by Oxide HDL yet.
    Configuration,
}
```

Add these fields to `Instance`, after `component`:

```rust
    /// Library prefix exactly as written in the source, case preserved.
    /// `None` for component instantiations, which carry no library.
    ///
    /// `work` here is a self-reference to the library of the file containing the
    /// instantiation, not a library literally named `work`. Compare it
    /// case-insensitively — see `backend::units::resolve_entity_uris`.
    pub library: Option<String>,
    /// Architecture named in `entity work.cpu(behavioral)`, case preserved.
    pub architecture: Option<String>,
    /// Which instantiation form this is.
    pub unit_kind: InstantiatedUnitKind,
```

- [ ] **Step 4: Rewrite the extraction**

In `src/analysis/builders.rs`, replace the whole of `create_instance_from_node` (find it by name; it moves as the file changes) with:

```rust
/// Create an instance struct from the component instantiation node
///
/// # Arguments
/// `node` - Component Instantiation Node
/// `text` - Fill source text
///
/// # Returns
/// A struct representing the instance
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
    if let Some(name) = name_node {
        let selections: Vec<Node> = name
            .children(&mut name.walk())
            .filter(|c| c.kind() == "selection")
            .collect();

        if let Some(last) = selections.last() {
            // Dotted name: unit is the final segment, library the leading identifier.
            if let Some(iden) = find_child(*last, "identifier") {
                component = text[iden.byte_range()].to_string();
            }
            if library.is_none()
                && let Some(iden) = find_child(name, "identifier")
            {
                library = Some(text[iden.byte_range()].to_string());
            }
        } else if let Some(iden) = find_child(name, "identifier") {
            component = text[iden.byte_range()].to_string();
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
    }
}
```

Add `InstantiatedUnitKind` to the `crate::analysis::{...}` import list at the top of `builders.rs` (line 8), alongside the existing `Instance`.

- [ ] **Step 5: Run the new tests**

Run: `cargo test test_inst_unit 2>&1 | tail -20`

Expected: 9 passed.

- [ ] **Step 6: Verify no regression in existing instantiation tests**

Run: `cargo test instantiation 2>&1 | tail -15`

Expected: the pre-existing `test_instantiation_entity_work`, `test_instantiation_entity_with_architecture`, `test_instantiation_in_generate` and friends all still pass — they assert only `label` and `component`, both unchanged.

- [ ] **Step 7: Run the full suite**

Run: `cargo test 2>&1 | tail -5`

Expected: `490 passed; 0 failed`.

> **Expected transient warning.** Like Task 1, this task only *produces* new API.
> Until Task 4 consumes them, the non-test build reports
> `fields library, architecture, and unit_kind are never read` at `src/analysis/types.rs`.
> That is correct — **do not silence it with `#[allow(dead_code)]`**. It clears when
> Task 4 lands.

- [ ] **Step 8: Commit**

```bash
git add src/analysis/types.rs src/analysis/builders.rs src/analysis/tests/builders_tests.rs
git commit -m "feat: capture library, architecture and unit kind on Instance"
```

---

## Task 4: Library-Aware Design-Unit Queries

**Files:**
- Create: `src/backend/units.rs`
- Modify: `src/backend/mod.rs` (module registration only)

**Interfaces:**
- Consumes: `Analysis.library` (Task 2); `Instance.library`, `Instance.unit_kind`, `InstantiatedUnitKind` (Task 3).
- Produces, all in `crate::backend::units`:
  - `pub fn file_declares_entity(analysis: &Analysis, name_lc: &str) -> bool`
  - `pub fn resolve_entity_uris(map: &AnalysisMap, inst: &Instance, current_library: &str) -> Vec<Url>`
  - `pub fn entities_in_library(map: &AnalysisMap, library: &str) -> Vec<(String, Url)>`
  - `pub fn known_libraries(map: &AnalysisMap) -> Vec<String>`

These are pure functions over an immutable map — no locks, no async, no filesystem. That is what makes them directly unit-testable and safe to call from inside completion.

Three rules encode the semantics:

1. **`work` is a self-reference.** `entity work.cpu` written in a file belonging to `rtl_lib` resolves against `rtl_lib`, not against some library named `work`.
2. **Unconfigured means unchanged.** If the library-scoped search finds nothing, fall back to a name-only search across the whole map. A workspace with no `[libraries]` therefore behaves exactly as it does today.
3. **Entities must be findable at both parse levels.** Shallow files expose entities via `analysis.symbols` (kind `Entity`); deep files via `analysis.entity_scope_trees`. Check both.

Results are sorted by URI string so callers get a stable order.

- [ ] **Step 1: Write the failing tests**

Create `src/backend/units.rs` containing **only** the test module for now:

```rust
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
```

Note the unused-import risk: if `ParseLevel` or `Range` end up unused after this, drop them from the test module's `use` list to keep the build warning-free.

- [ ] **Step 2: Register the module and run the tests to verify they fail**

In `src/backend/mod.rs`, add after `pub mod syntax;` (line 8):

```rust
pub mod units;
```

Run: `cargo test units:: 2>&1 | tail -20`

Expected: compile errors — `resolve_entity_uris`, `entities_in_library`, `known_libraries`, `file_declares_entity` not found.

- [ ] **Step 3: Implement the queries**

Prepend to `src/backend/units.rs`, above the test module:

```rust
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

use crate::analysis::{Analysis, InstantiatedUnitKind, Instance, OxideSymbolKind};
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
pub fn resolve_entity_uris(
    map: &AnalysisMap,
    inst: &Instance,
    current_library: &str,
) -> Vec<Url> {
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test units:: 2>&1 | tail -20`

Expected: 12 passed.

- [ ] **Step 5: Run the full suite**

Run: `cargo test 2>&1 | tail -5`

Expected: `502 passed; 0 failed`.

- [ ] **Step 6: Commit**

```bash
git add src/backend/units.rs src/backend/mod.rs
git commit -m "feat: add library-aware design-unit resolution queries"
```

---

## Task 5: Deep-Parse Instantiated Entities On Demand

**Files:**
- Modify: `src/analysis/scope_tree.rs`
- Modify: `src/backend/mod.rs:199-247` (`ensure_dependencies_loaded`)

**Interfaces:**
- Consumes: `resolve_entity_uris` (Task 4); `Analysis.library` (Task 2).
- Produces: `ScopeTree::collect_all_instantiations(&self) -> Vec<&Instance>` — every instantiation in this scope and all nested scopes (generate bodies, blocks).

`ensure_dependencies_loaded` already runs after every open and change, and already JIT-loads `use`d packages. Instantiated entities get the same treatment: resolve each one, then `ensure_fully_parsed` the file that declares it. After this, hovering an instance or completing its port map needs no separate on-demand parse.

Nested scopes matter — an instantiation inside a `for ... generate` lives in a child `ScopeTree`, and a top level that instantiates everything inside generate blocks is common.

- [ ] **Step 1: Write the failing test**

Add to `src/analysis/tests/scope_tree_tests.rs`, at the end of the file:

```rust
#[test]
fn test_collect_all_instantiations_includes_nested_scopes() {
    let code = r#"
architecture rtl of top is
begin
    u_direct: entity work.cpu port map (clk => clk);

    gen_loop: for i in 0 to 3 generate
    begin
        u_nested: entity work.cell port map (idx => i);
    end generate;

    blk: block
    begin
        u_block: entity work.ram port map (clk => clk);
    end block;
end architecture;
"#;
    let tree = crate::backend::test_utils::parse_text(code);
    let analysis =
        crate::backend::syntax::parser::extract_document_symbols(code, tree.root_node());

    let mut found: Vec<String> = analysis.scope_trees[0]
        .collect_all_instantiations()
        .iter()
        .map(|i| i.component.to_lowercase())
        .collect();
    found.sort();

    assert_eq!(
        found,
        vec!["cell".to_string(), "cpu".to_string(), "ram".to_string()],
        "instantiations inside generate and block scopes must be collected too"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test test_collect_all_instantiations 2>&1 | tail -20`

Expected: compile error — no method `collect_all_instantiations`.

- [ ] **Step 3: Implement the collector**

In `src/analysis/scope_tree.rs`, find `collect_all_use_clauses` and add directly beneath it, inside the same `impl ScopeTree` block:

```rust
    /// Collects every instantiation in this scope and all nested child scopes.
    ///
    /// Instantiations inside `generate` and `block` statements live in child scope
    /// trees, so a flat read of `self.instantiations` misses them. Used to decide
    /// which entity files to JIT-parse when a document is opened.
    pub fn collect_all_instantiations(&self) -> Vec<&Instance> {
        let mut out: Vec<&Instance> = self.instantiations.iter().collect();
        for child in &self.children {
            out.extend(child.collect_all_instantiations());
        }
        out
    }
```

If `Instance` is not already in scope in `scope_tree.rs`, it is — the file imports it at line 6 (`use crate::analysis::Instance;`).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test test_collect_all_instantiations 2>&1 | tail -10`

Expected: 1 passed.

- [ ] **Step 5: Wire instantiated entities into the JIT loader**

In `src/backend/mod.rs`, add to the imports near line 11:

```rust
use crate::backend::units;
```

Then in `ensure_dependencies_loaded`, extend the collection block. Replace lines 200-219 (from `// 1. Get the analysis` through the closing `}; // Lock dropped here`) with:

```rust
        // 1. Get the analysis of the current file to find what it "uses" and instantiates
        let (deps_to_load, entity_files) = {
            let map = self.analysis_map.read().await;
            let analysis = match map.get(uri) {
                Some(a) => a,
                None => return,
            };

            let mut missing_deps = Vec::new();

            for clause in &analysis.use_clauses {
                // CALL THE INTERCEPTOR!
                if let Some(dep_uri) = lookup::resolve_import_uri(&clause.library, &clause.name)
                    && !map.contains_key(&dep_uri)
                {
                    missing_deps.push(dep_uri);
                }
            }

            // Entities instantiated by this file need their interfaces available for
            // hover, goto and port-map completion. Resolve them now; the parse itself
            // happens below, outside the read lock.
            //
            // `seen` gives O(1) dedup — a top level instantiating the same entity 50
            // times must parse it once. The Vec preserves a deterministic order so JIT
            // parse log lines are reproducible across runs; HashSet iteration is not.
            let mut entity_files: Vec<Url> = Vec::new();
            let mut seen: std::collections::HashSet<Url> = std::collections::HashSet::new();
            for scope_tree in &analysis.scope_trees {
                for inst in scope_tree.collect_all_instantiations() {
                    for target in units::resolve_entity_uris(&map, inst, &analysis.library) {
                        if target != *uri && seen.insert(target.clone()) {
                            entity_files.push(target);
                        }
                    }
                }
            }

            (missing_deps, entity_files)
        }; // Lock dropped here
```

Then append, immediately before the closing brace of `ensure_dependencies_loaded` (after the existing `for dep_uri in deps_to_load { ... }` loop):

```rust
        // 3. Upgrade instantiated entities from shallow to deep so their ports are known.
        //    `ensure_fully_parsed` short-circuits on `parse_level == Deep` behind a read
        //    lock, so repeat calls cost a hashmap get each. The expensive pass happens
        //    once, at first open — which is when an LSP is expected to warm up.
        let lib_matcher = {
            let config_guard = self.config.read().await;
            crate::config::LibraryMatcher::from_config(
                &config_guard.clone().unwrap_or_else(OxideConfig::default),
            )
        };
        for entity_uri in entity_files {
            workspace::ensure_fully_parsed(
                &self.client,
                &self.analysis_map,
                &self.parser,
                &entity_uri,
                &lib_matcher,
            )
            .await;
        }
```

- [ ] **Step 6: Verify it compiles and nothing regressed**

Run: `cargo test 2>&1 | tail -5`

Expected: `503 passed; 0 failed`.

**Do not delete the `Backend::completion` / `Backend::hover` pre-parse calls.** Once Task 5 lands they look redundant — the entities an open file instantiates are already deep-parsed. They are not. Task 5 deliberately resolves nothing for `unit_kind == Component`, so *component* instantiations still depend entirely on those call sites to force-parse the entity file that supplies their ports. Removing them silently breaks port-map completion for every component-style instantiation, with no test failure. Leave a comment saying so at each site.

**Note on test coverage:** `ensure_dependencies_loaded` is an async method on `Backend`, which requires a live `tower_lsp::Client` to construct — there is no existing harness for that in this codebase, and building one is out of scope. The pure part (`collect_all_instantiations`) is tested; the wiring is verified by compilation and by the manual check in Step 7. Do not claim this step is covered by automated tests.

- [ ] **Step 7: Manual verification**

```bash
mkdir -p /tmp/oxide-jit-check/rtl
cat > /tmp/oxide-jit-check/rtl/uart_tx.vhd <<'VHDL'
entity uart_tx is
  generic (BAUD : integer := 115200);
  port (clk : in bit; tx : out bit);
end entity;
VHDL
cat > /tmp/oxide-jit-check/rtl/top.vhd <<'VHDL'
architecture rtl of top is
  signal c : bit;
  signal t : bit;
begin
  u0: entity work.uart_tx port map (clk => c, tx => t);
end architecture;
VHDL
cargo build --release 2>&1 | tail -3
```

Open `/tmp/oxide-jit-check` in an editor using the built server, open `top.vhd` only (never open `uart_tx.vhd`), and confirm the server log shows `JIT Parse completed for .../uart_tx.vhd`. Then hover `uart_tx` and confirm it resolves.

- [ ] **Step 8: Commit**

```bash
git add src/analysis/scope_tree.rs src/analysis/tests/scope_tree_tests.rs src/backend/mod.rs
git commit -m "feat: JIT deep-parse entities instantiated by an open document"
```

---

## Task 6: Entity Name Completion After a Library Prefix

**Files:**
- Modify: `src/backend/features/completion/mod.rs`
- Modify: `src/backend/features/completion/tests.rs`

**Interfaces:**
- Consumes: `entities_in_library`, `known_libraries` (Task 4); `Analysis.library` (Task 2).
- Produces:
  - Two `CompletionContext` variants: `InstantiationLibrary` and `LibraryUnits(String)` (payload is the lowercase library name as typed).
  - `fn detect_instantiation_unit_context(text: &str, pos: Position) -> Option<CompletionContext>`

This is the `rtl_lib.my_|` case. Detection has to be text-based: mid-type, `u0: entity rtl_lib.my_` does not parse into a clean `instantiated_unit`, and the existing `is_dot_access_context` fast path (line 1364) would otherwise swallow the dot and try to resolve record fields.

Completion items carry the bare entity name as the label. Where the target entity is **already deep-parsed**, the insert text is the full port-map snippet — free, since the interface is in memory. Where it is still shallow, the insert text is just the name; the user then types `port map (` and the existing `PortMapLhs` path (which force-parses on demand) takes over. This is what keeps a 500-entity library cheap.

- [ ] **Step 1: Write the failing tests**

Add to `src/backend/features/completion/tests.rs`, at the end of the file:

```rust
// =========================================================================
// Instantiation unit completion: `entity <lib>.<name>`
// =========================================================================

use crate::analysis::{OxideSymbolKind as OSK, ParseLevel, Symbol as Sym};

/// Builds a shallow Analysis in `library` declaring `entities`.
fn shallow_lib(library: &str, entities: &[&str]) -> crate::analysis::Analysis {
    let mut a = crate::analysis::Analysis::new();
    a.library = library.to_string();
    a.parse_level = ParseLevel::Shallow;
    for e in entities {
        a.symbols.insert(
            e.to_lowercase(),
            Sym {
                name: e.to_string(),
                kind: OSK::Entity,
                detail: Some("Entity".to_string()),
                range: tower_lsp::lsp_types::Range::default(),
                children: Vec::new(),
            },
        );
    }
    a
}

#[test]
fn test_detect_context_after_library_dot() {
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.my_\n";
    let pos = Position {
        line: 2,
        character: 26,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::LibraryUnits("rtl_lib".to_string()))
    );
}

#[test]
fn test_detect_context_after_library_dot_empty_prefix() {
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::LibraryUnits("rtl_lib".to_string()))
    );
}

#[test]
fn test_detect_context_after_entity_keyword() {
    let text = "architecture rtl of top is\nbegin\n  u0: entity \n";
    let pos = Position {
        line: 2,
        character: 13,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::InstantiationLibrary)
    );
}

#[test]
fn test_detect_context_is_case_insensitive() {
    let text = "architecture rtl of top is\nbegin\n  U0: ENTITY RTL_LIB.MY_\n";
    let pos = Position {
        line: 2,
        character: 26,
    };
    assert_eq!(
        super::detect_instantiation_unit_context(text, pos),
        Some(CompletionContext::LibraryUnits("rtl_lib".to_string()))
    );
}

#[test]
fn test_detect_context_ignores_unrelated_dotted_names() {
    // A record field access must NOT be mistaken for a library prefix.
    let text = "architecture rtl of top is\nbegin\n  x <= my_rec.fie\n";
    let pos = Position {
        line: 2,
        character: 18,
    };
    assert_eq!(super::detect_instantiation_unit_context(text, pos), None);
}

#[test]
fn test_detect_context_ignores_use_clause() {
    // `use ieee.std_logic_1164` is not an instantiation.
    let text = "use ieee.std_\n";
    let pos = Position {
        line: 0,
        character: 13,
    };
    assert_eq!(super::detect_instantiation_unit_context(text, pos), None);
}

#[test]
fn test_library_units_completion_lists_entities_of_that_library() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity rtl_lib.\n";
    let pos = Position {
        line: 2,
        character: 24,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        shallow_lib("rtl_lib", &["uart_tx", "cpu"]),
    );
    map.insert(
        Url::parse("file:///b.vhd").unwrap(),
        shallow_lib("other_lib", &["excluded"]),
    );
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();

    let ctx = get_completion_context(text, root, pos);
    assert_eq!(ctx, CompletionContext::LibraryUnits("rtl_lib".to_string()));

    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);
    let names = labels(&items);
    assert!(names.contains(&"cpu"), "expected cpu, got {names:?}");
    assert!(names.contains(&"uart_tx"), "expected uart_tx, got {names:?}");
    assert!(
        !names.contains(&"excluded"),
        "entity from another library leaked: {names:?}"
    );
}

#[test]
fn test_work_prefix_lists_current_files_library() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity work.\n";
    let pos = Position {
        line: 2,
        character: 19,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        shallow_lib("rtl_lib", &["uart_tx"]),
    );
    map.insert(
        Url::parse("file:///b.vhd").unwrap(),
        shallow_lib("other_lib", &["excluded"]),
    );
    // The file being edited lives in rtl_lib, so `work.` means rtl_lib.
    map.insert(top_uri.clone(), shallow_lib("rtl_lib", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();

    let ctx = get_completion_context(text, root, pos);
    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);
    let names = labels(&items);
    assert!(names.contains(&"uart_tx"), "expected uart_tx, got {names:?}");
    assert!(
        !names.contains(&"excluded"),
        "work must not reach other_lib: {names:?}"
    );
}

#[test]
fn test_instantiation_library_completion_lists_libraries() {
    use crate::backend::AnalysisMap;
    let text = "architecture rtl of top is\nbegin\n  u0: entity \n";
    let pos = Position {
        line: 2,
        character: 13,
    };
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        shallow_lib("rtl_lib", &["uart_tx"]),
    );
    map.insert(top_uri.clone(), shallow_lib("work", &[]));

    let tree = crate::backend::test_utils::parse_text(text);
    let root = tree.root_node();

    let ctx = get_completion_context(text, root, pos);
    assert_eq!(ctx, CompletionContext::InstantiationLibrary);

    let items = complete_scope(&map, &top_uri, &ctx, pos, text, root);
    let names = labels(&items);
    assert!(names.contains(&"work"), "expected work, got {names:?}");
    assert!(
        names.contains(&"rtl_lib"),
        "expected rtl_lib, got {names:?}"
    );
}
```

`labels` is already defined in `tests.rs` and is in scope.

> **Do not wrap `parse_text` in `SHARED_PARSER_LOCK`.** `crate::backend::test_utils::parse_text`
> acquires that mutex internally, and `std::sync::Mutex` is not reentrant — locking around a
> `parse_text` call deadlocks the test binary. It compiles cleanly and the RED step looks
> normal, so the failure surfaces only as a hung suite at GREEN. Existing tests in this file
> that *do* take the lock construct a `Parser` by hand instead of calling `parse_text`; either
> approach is fine, but never combine them.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -- completion::tests::test_detect_context completion::tests::test_library_units completion::tests::test_work_prefix completion::tests::test_instantiation_library 2>&1 | tail -20`

Expected: compile errors — no `detect_instantiation_unit_context`, no `LibraryUnits` / `InstantiationLibrary` variants.

- [ ] **Step 3: Add the context variants**

In `src/backend/features/completion/mod.rs`, add to the `CompletionContext` enum after `GenericMapRhs` (line 83):

```rust
    /// Cursor sits right after the `entity` keyword of an instantiation, before any
    /// library name. Suggests: library names present in the workspace.
    InstantiationLibrary,

    /// Cursor sits after `<library>.` in an instantiation. Payload: lowercase library
    /// name as typed (`work` is resolved against the current file's library at
    /// completion time, not here). Suggests: entities declared in that library.
    LibraryUnits(String),
```

- [ ] **Step 4: Implement detection**

`completion/mod.rs` has no `lazy_static!` block yet — add one directly below the `use` statements at the top of the file (both crates are already dependencies and used elsewhere in the codebase, e.g. `src/backend/syntax/scanner.rs`):

```rust
lazy_static::lazy_static! {
    /// Matches `entity <lib>.<partial>` at the end of the text before the cursor.
    /// Captures the library name.
    static ref RE_INST_LIB_DOT: regex::Regex =
        regex::Regex::new(r"(?i)\bentity\s+([A-Za-z]\w*)\s*\.\s*(\w*)$").unwrap();

    /// Matches `entity <partial>` at the end of the text before the cursor, with no dot.
    static ref RE_INST_ENTITY_KW: regex::Regex =
        regex::Regex::new(r"(?i)\bentity\s+(\w*)$").unwrap();
}
```

Then add the detection function, next to `find_component_via_text`:

```rust
/// Detects whether the cursor sits in the *unit name* position of a direct entity
/// instantiation — i.e. `u0: entity |`, or `u0: entity rtl_lib.my_|`.
///
/// This is text-based rather than AST-based on purpose: while the user is typing,
/// `entity rtl_lib.my_` does not parse into a clean `instantiated_unit`, and the
/// generic dot-access path would otherwise treat the dot as a record field access.
///
/// Anchored to the `entity` keyword, so `use ieee.std_logic_1164` and ordinary
/// record accesses like `my_rec.field` are correctly rejected.
///
/// # Arguments
/// * `text` - Full source text.
/// * `pos` - Cursor position.
///
/// # Returns
/// `Some(CompletionContext::LibraryUnits(lib))` after a library prefix,
/// `Some(CompletionContext::InstantiationLibrary)` right after `entity`,
/// or `None` when this is not an instantiation unit position.
fn detect_instantiation_unit_context(text: &str, pos: Position) -> Option<CompletionContext> {
    let line = text.lines().nth(pos.line as usize)?;
    let col = (pos.character as usize).min(line.len());
    let prefix = &line[..col];

    if let Some(caps) = RE_INST_LIB_DOT.captures(prefix) {
        let library = caps.get(1)?.as_str().to_lowercase();
        return Some(CompletionContext::LibraryUnits(library));
    }

    if RE_INST_ENTITY_KW.is_match(prefix) {
        return Some(CompletionContext::InstantiationLibrary);
    }

    None
}
```

Wire it into `get_completion_context` **before** the dot-access fast path. Replace lines 1362-1365:

```rust
    // 1. Handle Dot Access (fast path)
    if is_dot_access_context(&node, text, pos) {
        return CompletionContext::DotAccess;
    }
```

with:

```rust
    // 1. Instantiation unit position (`entity |`, `entity lib.|`) — must precede the
    //    dot-access fast path, which would otherwise treat the library dot as a
    //    record field access.
    if let Some(ctx) = detect_instantiation_unit_context(text, pos) {
        return ctx;
    }

    // 2. Handle Dot Access (fast path)
    if is_dot_access_context(&node, text, pos) {
        return CompletionContext::DotAccess;
    }
```

- [ ] **Step 5: Implement resolution**

In `complete_scope`, add two arms to the `match context { ... }` block, immediately before the `CompletionContext::DotAccess =>` arm (around line 2290):

```rust
            CompletionContext::InstantiationLibrary => {
                for library in crate::backend::units::known_libraries(analysis_map) {
                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::MODULE),
                        label: library.clone(),
                        detail: Some("VHDL library".to_string()),
                        insert_text: Some(format!("{}.", library)),
                        ..Default::default()
                    });
                }
                // `work` is always a valid prefix even if no file is stamped with it.
                if !items.iter().any(|i| i.label == "work") {
                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::MODULE),
                        label: "work".to_string(),
                        detail: Some("VHDL library (current)".to_string()),
                        insert_text: Some("work.".to_string()),
                        ..Default::default()
                    });
                }
                items.sort_by(|a, b| a.label.cmp(&b.label));
                return items;
            }

            CompletionContext::LibraryUnits(library) => {
                // `work` means the library of the file being edited. Compare
                // case-insensitively — VHDL identifiers are case-insensitive and this
                // payload comes from user-typed text.
                let target = if library.eq_ignore_ascii_case("work") {
                    current_analysis.library.clone()
                } else {
                    library.clone()
                };

                for (name, entity_uri) in
                    crate::backend::units::entities_in_library(analysis_map, &target)
                {
                    // Emit a full port-map snippet only when the entity's interface is
                    // already in memory. Deep-parsing a whole library to build this list
                    // would be unacceptable, so shallow entities get a plain name and the
                    // existing PortMapLhs path fills in ports once the user types further.
                    let deep_tree = analysis_map
                        .get(&entity_uri)
                        .and_then(|a| a.entity_scope_trees.get(&name));

                    let (insert_text, format) = match deep_tree {
                        Some(tree) => (
                            generate_instantiation_snippet(&name, tree),
                            InsertTextFormat::SNIPPET,
                        ),
                        None => (name.clone(), InsertTextFormat::PLAIN_TEXT),
                    };

                    items.push(CompletionItem {
                        kind: Some(CompletionItemKind::CLASS),
                        label: name.clone(),
                        detail: Some(format!("entity in {}", target)),
                        filter_text: Some(name.clone()),
                        insert_text: Some(insert_text),
                        insert_text_format: Some(format),
                        ..Default::default()
                    });
                }
                items.sort_by(|a, b| a.label.cmp(&b.label));
                return items;
            }
```

- [ ] **Step 6: Run the new tests**

Run: `cargo test -- completion::tests::test_detect_context completion::tests::test_library_units completion::tests::test_work_prefix completion::tests::test_instantiation_library 2>&1 | tail -20`

Expected: 9 passed.

- [ ] **Step 7: Verify no completion regressions**

Run: `cargo test completion 2>&1 | tail -10`

Expected: all completion tests pass. Pay particular attention to any existing `DotAccess` test — if one now fails, the `entity` anchor in `RE_INST_LIB_DOT` is matching too broadly; tighten it rather than reordering the checks.

- [ ] **Step 8: Run the full suite**

Run: `cargo test 2>&1 | tail -5`

Expected: `512 passed; 0 failed`.

- [ ] **Step 9: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: complete library names and entity names in direct instantiations"
```

---

## Task 7: Workspace-Wide Instantiation Snippets in Direct Form

**Files:**
- Modify: `src/backend/features/completion/mod.rs` (`generate_entity_completions`, line 1885)
- Modify: `src/backend/features/completion/tests.rs`

**Interfaces:**
- Consumes: `Analysis.library` (Task 2).
- Produces: `generate_entity_completions(analysis_map: &AnalysisMap, analysis: &Analysis) -> Vec<CompletionItem>` — signature unchanged, behaviour extended. It reads `analysis.library` for the current file rather than taking a new parameter.

Two defects are fixed here, both verified against the current build:

1. **Entities in other files are never offered.** `generate_entity_completions` iterates only `analysis.entity_scope_trees` — the current file. In a direct-instantiation codebase with no component declarations and no package to import, typing `u0: ` yields zero instantiation snippets.
2. **The emitted snippet is invalid VHDL.** It writes a bare `uart_tx\nport map (...)`. A bare name is only legal when a component declaration is in scope; for an entity it must be `entity <lib>.<name>`. Today's same-file snippet emits code that does not compile.

Component-sourced snippets (from `use`d packages) keep emitting a bare name — that is correct for components, and untouched.

`generate_instantiation_snippet` itself is **not** modified: its first parameter is already "the text to emit as the instantiated unit", so passing `"entity work.uart_tx"` produces the right output and every existing test of that function stays green.

- [ ] **Step 1: Write the failing tests**

Add to `src/backend/features/completion/tests.rs`, at the end of the file:

```rust
// =========================================================================
// Workspace-wide instantiation snippets, direct form
// =========================================================================

/// Builds a deep-parsed Analysis in `library` with one entity that has ports.
fn deep_entity_analysis(library: &str, entity: &str, src: &str) -> crate::analysis::Analysis {
    let tree = crate::backend::test_utils::parse_text(src);
    let mut a =
        crate::backend::syntax::parser::extract_document_symbols(src, tree.root_node());
    a.library = library.to_string();
    assert!(
        a.entity_scope_trees.contains_key(entity),
        "fixture did not produce an entity scope tree for {entity}"
    );
    a
}

#[test]
fn test_instantiation_snippet_offered_for_entity_in_another_file() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit; tx : out bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let sub_uri = Url::parse("file:///sub.vhd").unwrap();
    let top_uri = Url::parse("file:///top.vhd").unwrap();

    let mut map = AnalysisMap::new();
    map.insert(
        sub_uri,
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a = crate::backend::syntax::parser::extract_document_symbols(
            top_src,
            tree.root_node(),
        );
        a.library = "rtl_lib".to_string();
        a
    };
    map.insert(top_uri, top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let names = labels(&items);
    assert!(
        names.contains(&"uart_tx"),
        "cross-file entity must be offered, got {names:?}"
    );
}

#[test]
fn test_same_library_entity_uses_work_prefix() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///sub.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a = crate::backend::syntax::parser::extract_document_symbols(
            top_src,
            tree.root_node(),
        );
        a.library = "rtl_lib".to_string();
        a
    };
    map.insert(Url::parse("file:///top.vhd").unwrap(), top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("uart_tx must be offered");
    let text = item.insert_text.as_ref().expect("snippet must have text");
    assert!(
        text.starts_with("entity work.uart_tx"),
        "same-library entity should use the work prefix, got: {text}"
    );
}

#[test]
fn test_cross_library_entity_uses_explicit_library_prefix() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let mut map = AnalysisMap::new();
    map.insert(
        Url::parse("file:///sub.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a = crate::backend::syntax::parser::extract_document_symbols(
            top_src,
            tree.root_node(),
        );
        a.library = "top_lib".to_string();
        a
    };
    map.insert(Url::parse("file:///top.vhd").unwrap(), top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let item = items
        .iter()
        .find(|i| i.label == "uart_tx")
        .expect("uart_tx must be offered");
    let text = item.insert_text.as_ref().expect("snippet must have text");
    assert!(
        text.starts_with("entity rtl_lib.uart_tx"),
        "cross-library entity needs an explicit prefix, got: {text}"
    );
}

#[test]
fn test_entity_snippet_is_deduplicated_across_files() {
    use crate::backend::AnalysisMap;

    let sub_src = "entity uart_tx is\n  port (clk : in bit);\nend entity;\n";
    let top_src = "architecture rtl of top is\nbegin\nend architecture;\n";

    let mut map = AnalysisMap::new();
    // Same entity name declared twice in the same library.
    map.insert(
        Url::parse("file:///a.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    map.insert(
        Url::parse("file:///b.vhd").unwrap(),
        deep_entity_analysis("rtl_lib", "uart_tx", sub_src),
    );
    let top_analysis = {
        let tree = crate::backend::test_utils::parse_text(top_src);
        let mut a = crate::backend::syntax::parser::extract_document_symbols(
            top_src,
            tree.root_node(),
        );
        a.library = "rtl_lib".to_string();
        a
    };
    map.insert(Url::parse("file:///top.vhd").unwrap(), top_analysis.clone());

    let items = generate_entity_completions(&map, &top_analysis);
    let count = items.iter().filter(|i| i.label == "uart_tx").count();
    assert_eq!(count, 1, "duplicate entity must be offered once");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -- completion::tests::test_instantiation_snippet_offered completion::tests::test_same_library_entity completion::tests::test_cross_library_entity completion::tests::test_entity_snippet_is_dedup 2>&1 | tail -20`

Expected: 4 failures — the cross-file entity is not offered at all, so `uart_tx` is missing from every assertion.

- [ ] **Step 3: Rewrite the entity half of `generate_entity_completions`**

In `src/backend/features/completion/mod.rs`, replace the first loop of `generate_entity_completions` — the block beginning `for entity in analysis.entity_scope_trees.values() {` and ending just before `for clause in &analysis.use_clauses {` — with:

```rust
    let current_library = analysis.library.clone();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Every deep-parsed entity in the workspace, not just this file's. In a
    // direct-instantiation codebase there are no component declarations to fall
    // back on, so a current-file-only list means no completions at all.
    let mut sources: Vec<(&Analysis, &ScopeTree, String)> = Vec::new();
    for global in analysis_map.values() {
        for (key, tree) in &global.entity_scope_trees {
            sources.push((global, tree, key.clone()));
        }
    }
    sources.sort_by(|a, b| a.2.cmp(&b.2));

    for (owner, entity, key) in sources {
        if !seen.insert(key.clone()) {
            continue;
        }
        let name = entity.name.clone().unwrap_or_else(|| key.clone());

        // `work` is the correct prefix only when the target shares our library;
        // otherwise the library must be named explicitly.
        let unit_ref = if owner.library == current_library {
            format!("entity work.{}", name)
        } else {
            format!("entity {}.{}", owner.library, name)
        };

        let snippet = generate_instantiation_snippet(&unit_ref, entity);
        items.push(CompletionItem {
            kind: Some(CompletionItemKind::SNIPPET),
            label: name.clone(),
            detail: Some("Entity Instantiation".to_string()),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(format!("entity in {}", owner.library)),
            }),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "** Generate instantiation for `{}`**\n\n```vhdl\n{}\n```",
                    name, snippet
                ),
            })),
            sort_text: Some(format!("!{}", name)),
            filter_text: Some(name),
            insert_text: Some(snippet),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });
    }
```

`ScopeTree` and `Analysis` are both already in the `crate::analysis::{...}` import at the top of the file — no import change is needed.

- [ ] **Step 4: Run the new tests**

Run: `cargo test -- completion::tests::test_instantiation_snippet_offered completion::tests::test_same_library_entity completion::tests::test_cross_library_entity completion::tests::test_entity_snippet_is_dedup 2>&1 | tail -20`

Expected: 4 passed.

- [ ] **Step 5: Check the pre-existing snippet tests**

Run: `cargo test generate_instantiation_snippet 2>&1 | tail -10`

Expected: all pass unchanged — those tests call `generate_instantiation_snippet` directly with a bare name, and that function was not modified.

- [ ] **Step 6: Run the full suite**

Run: `cargo test 2>&1 | tail -5`

Expected: `516 passed; 0 failed`.

- [ ] **Step 7: Commit**

```bash
git add src/backend/features/completion/mod.rs src/backend/features/completion/tests.rs
git commit -m "feat: offer workspace-wide entity instantiation snippets in direct form"
```

---

## Task 8: Documentation and Final Verification

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: everything above.
- Produces: user-facing documentation of the `[libraries]` config section.

- [ ] **Step 1: Document `[libraries]` in the README**

Find the section of `README.md` that documents `oxide.toml` (search for `ignore` or `include_workspace`) and add:

````markdown
### Libraries

By default every file belongs to the `work` library, and `entity work.foo`
resolves against any entity named `foo` anywhere in the workspace. If your design
compiles into several VHDL libraries, declare them so that resolution, go-to
definition and entity completion become library-accurate:

```toml
[libraries]
rtl_lib = ["rtl/**/*.vhd", "common/**/*.vhd"]
unisim    = ["/opt/Xilinx/**/unisims/**/*.vhd"]
```

Globs are matched against the path relative to the workspace root, then against
the absolute path — the latter lets you declare vendor libraries living outside
your repository. Files matching no pattern stay in `work`.

`work` is treated as a self-reference, exactly as VHDL defines it: `entity work.cpu`
written in a file belonging to `rtl_lib` resolves to `rtl_lib.cpu`.

With libraries declared, typing `u0: entity rtl_lib.` offers every entity in that
library, drawn from the fast index without parsing them.
````

- [ ] **Step 2: Add a CHANGELOG entry**

Add to the top of `CHANGELOG.md`, following the existing format used for prior releases:

```markdown
## Unreleased

### Added

- `[libraries]` section in `oxide.toml` mapping path globs to VHDL library names.
- Entity name completion after a library prefix (`u0: entity rtl_lib.`), served
  from the fast index without deep-parsing the library.
- Library name completion after the `entity` keyword.
- Instantiation snippets now cover entities declared anywhere in the workspace,
  not only the current file.
- Entities instantiated by an open file are deep-parsed automatically, so hover
  and port-map completion work without opening them.

### Changed

- Entity instantiation snippets are emitted in direct form (`entity work.foo`)
  instead of a bare name, which was not valid VHDL without a component declaration.
- `Instance` now records the library, architecture and instantiation kind that the
  parser previously discarded.
```

- [ ] **Step 3: Full verification**

```bash
cargo test 2>&1 | tail -5
cargo clippy --all-targets 2>&1 | grep -E "^(warning|error)" | head -20
cargo build --release 2>&1 | tail -3
```

Expected: `516 passed; 0 failed`; no new clippy warnings; a clean release build.

- [ ] **Step 4: Review the commit series**

```bash
git log --oneline main..HEAD
```

Expected: 8 commits, one per task.

- [ ] **Step 5: Commit the documentation**

```bash
git add README.md CHANGELOG.md
git commit -m "docs: document [libraries] config and direct instantiation support"
```

---

## Self-Review Notes

Checked against the four requirements as stated:

| Requirement | Task |
|---|---|
| "be able to set libraries" | 1 (config + matcher), 2 (stamping) |
| "check entity from all we know when direct instantiation is done" | 3 (capture library), 4 (`resolve_entity_uris`) |
| "deep parse if an entity is being used somewhere" | 5 |
| "auto complete for entity name while we write `rtl_lib.my_`" | 6 |
| (implied, found during analysis) authoring flow produces invalid VHDL | 7 |

Points where an implementer is most likely to go wrong, all called out inline:

- **Task 3** — the grammar asymmetries. `work` after `entity` is a `library_namespace` field; `work` after `configuration` is not; a named library is the leading identifier of a dotted `name`; and the plain form has no `instantiated_unit` node at all. Reread "Verified Grammar Facts" before writing that function.

- **Task 3 / Task 4 — case handling.** `Instance.library` and `.architecture` preserve source case, matching the codebase rule (HashMap keys lowercased, struct fields verbatim, comparisons normalized at the comparison site). That makes `resolve_entity_uris` responsible for lowercasing *before* it tests for `work` — `test_uppercase_work_still_expands_to_current_library` pins it. Note `Analysis.library` is different and *is* stored lowercase: it comes from `oxide.toml` via `LibraryMatcher`, not from parsed VHDL.

- **Dead-code warnings lag one task behind.** Each of Tasks 1, 3 and 4 defines API whose first
  *caller* appears in a later task, and rustc's dead-code analysis is transitive — a function
  that reads a field does not count as using it unless that function itself is called. So
  `units.rs`'s four functions warn until Task 5 calls `resolve_entity_uris`, and
  `Instance.architecture` keeps warning even then, because nothing reads it (architecture
  resolution is explicitly out of scope). All of this is expected. Never add `#[allow(dead_code)]`.

- **Task 5** — do not delete the `Backend::completion` / `Backend::hover` pre-parse calls as redundant. Component instantiations still depend on them, and nothing will fail if you remove them.
- **Task 2, Step 8** — `ensure_fully_parsed` replaces the `Analysis` wholesale, so it must recompute `library` from the matcher. An earlier draft had it carry the value over from the existing map entry, which was a trap: no compile error, no test failure, just silently wrong resolution for JIT-upgraded files. The matcher parameter exists specifically so all three write paths share one rule. **Do not "simplify" it back to a carry-over.**

- **Task 1** — library globs use `literal_separator(true)`; `build_globset()` deliberately does not. That asymmetry is intentional, not an oversight: tightening the existing ignore globs would change which files current users have indexed, while `[libraries]` is new and has no such history. Two tests (`test_star_does_not_cross_directory_separator`, `test_double_star_matches_zero_directories`) pin the intended semantics.

Task 5 is the only task whose deliverable is not fully covered by automated tests, and Step 6 says so explicitly rather than implying otherwise.
