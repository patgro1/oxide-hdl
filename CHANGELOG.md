# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Fixed

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
