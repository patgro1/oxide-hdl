# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Fixed

## [0.7.0] — 2026-07-27

### Added

- **VHDL library support** — a new `[libraries]` section in `oxide.toml` maps path globs to library names. `work` behaves as VHDL defines it: a self-reference to the library of the file containing the reference, not a library of its own. With no `[libraries]` section every file belongs to `work` and behaviour is unchanged.

  Library awareness currently applies to entity-name completion and to choosing which
  file to deep-parse for an instantiated entity. Go-to-definition, hover and port-map
  completion still resolve by bare name across the whole workspace, so two entities
  sharing a name in different libraries remain ambiguous for those features. **Extending
  them is planned for 0.7.1** — see the Libraries section of `roadmap.md`. Workspaces
  without colliding entity names are unaffected either way.
- **Entity name completion after a library prefix** — typing `u0: entity rtl_lib.` offers every entity in that library, served from the fast index without parsing them. Typing `u0: entity ` offers the library names themselves.
- **Instantiation snippets now cover deep-parsed entities anywhere in the workspace** — previously only entities declared in the current file, plus components from imported packages, were offered.

  **Known limitation:** a file is deep-parsed only once it has been opened, or once something you opened instantiates it. Entities still sitting in the shallow index are not offered in the architecture-body completion list at all, so the entity you are about to instantiate for the first time may not appear until you have opened its file. Typing `entity <lib>.` does list shallow entities by name and is the reliable route today. Closing this gap needs shallow entities offered by name plus `completionItem/resolve` to fill the snippet on selection — tracked for 0.7.1.
- **Automatic deep-parse of instantiated entities** — opening a file resolves every `entity <lib>.<name>` it instantiates and upgrades those files from the shallow index, so hover, go-to-definition and port-map completion work against the real interface without opening them.

### Changed

- **Entity instantiation snippets are emitted in direct form** (`entity work.foo`) rather than as a bare name. A bare name is only legal VHDL when a component declaration is in scope, so the previous output did not compile for an entity. Component snippets sourced from packages are unaffected and still emit a bare name.
- Instantiations now retain the library, architecture and instantiation kind that the parser previously discarded.
- **Document outline names the bound architecture** — an instance written `u0: entity work.cpu(behavioral)` now shows `Instance of cpu(behavioral)` rather than `Instance of cpu`, which matters precisely when an entity has several architectures.

### Fixed

- `cargo clippy` is now warning-free across all targets. Six of the eight warnings predated this work: three map iterations that should use `.values()`, a hand-rolled `Default` impl that can be derived, a `sort_by` that should be `sort_by_key`, and two `tests.rs` files nesting a redundant `mod tests` (the rest of the codebase does not). Note CI runs only `cargo build` and `cargo test`, so nothing was catching this drift.

## [0.6.6] — 2026-07-27

### Fixed

- **Completion, hover and go-to-definition no longer disappear while typing** — any unclosed construct (an `if` awaiting its `end if;`, an unclosed `process`, `generate` or `block`, or a file not yet terminated with `end architecture;`) stops tree-sitter from building the architecture, yielding an analysis with no scope trees at all. That empty result was overwriting the previous good one on every keystroke, blanking out every language feature for the file until the construct was closed. The last good analysis is now retained until the buffer parses again, so suggestions stay available mid-edit — at most a few lines stale, and refreshed as soon as the file parses.

## [0.6.5] — 2026-05-05

### Fixed

- **Sensitivity list false positive for signal attributes** — using a signal's static attribute (e.g., `a'length`, `a'range`) as a function argument no longer causes the signal to be incorrectly flagged as missing from the process sensitivity list.

## [0.6.4] — 2026-04-16

### Fixed

- **Snippet completion after labeled statements** — process, generate, and instantiation snippets are now accessible after label prefixes (e.g., `my_label:p|`). Subprogram call detection no longer interferes with labeled statement context.

## [0.6.3] — 2026-04-14

### Added

- **Smart argument completion for functions and procedures** — inside a subprogram call argument list, the LSP now suggests parameter names filtered by what's already been supplied. Named association mode (`=>`) and positional mode are detected automatically.

### Changed

- **Cross-file parameter resolution** — function and procedure parameters are now resolved even when the subprogram is declared in an imported package (`use work.my_pkg.all`).
- **Mid-list completion** — triggering completion while the cursor is between two already-bound arguments correctly excludes all bound parameters, not just those before the cursor.
- **Partial name typing** — typing the beginning of a parameter name no longer drops it from the suggestion list on editor re-trigger.
- **Incomplete code resilience** — completion continues to work correctly inside an argument list before the closing `)` has been typed.

### Fixed

- **Port/generic map filter** — already-connected ports and generics are now excluded from suggestions even when inserting in the middle of a `port map` or `generic map`.
- **False-positive undeclared diagnostics** — field names in record element constraints no longer trigger spurious "undeclared identifier" warnings.

## [0.6.2]

### Fixed

- Various LSP completion and diagnostics improvements.

## [0.6.0]

Initial stable release.
