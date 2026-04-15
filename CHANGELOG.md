# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Fixed

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
