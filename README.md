# Oxide HDL

A VHDL Language Server Protocol (LSP) implementation written in Rust, built for large codebases and real-world FPGA development workflows.

## Why Oxide HDL?

Most VHDL tools are compiler-first — they require a fully correct, compilable design before offering any intelligence. This makes them slow to start, heavy on memory, and frustrating to use in the early stages of design.

Oxide HDL takes a different approach:

- **Tree-sitter based parsing** — fast, incremental, and error-tolerant. The server stays useful even when your file has syntax errors.
- **Two-pass analysis** — a lightweight regex scanner indexes your entire workspace on startup, then a full parse is triggered on-demand for files you actually open.
- **Built for monorepos** — designed to handle thousands of VHDL files without grinding to a halt.
- **Conservative diagnostics** — when in doubt, Oxide HDL stays quiet rather than flooding you with false positives.

## Installation

### Pre-built Binaries

Every release includes standalone binaries for all supported platforms, downloadable directly from the [Releases](https://github.com/patgro1/oxide-hdl/releases) page — no Rust toolchain required.

| Platform | Binary |
|----------|--------|
| Linux x86-64 | `oxide-hdl-linux-x64` |
| Linux ARM64 | `oxide-hdl-linux-arm64` |
| Linux x86-64 (musl/Alpine) | `oxide-hdl-alpine-x64` |
| Linux ARM64 (musl/Alpine) | `oxide-hdl-alpine-arm64` |
| macOS x86-64 | `oxide-hdl-darwin-x64` |
| macOS Apple Silicon | `oxide-hdl-darwin-arm64` |
| Windows x86-64 | `oxide-hdl-win32-x64.exe` |
| Windows ARM64 | `oxide-hdl-win32-arm64.exe` |

A [nightly build](https://github.com/patgro1/oxide-hdl/releases/tag/nightly) is published automatically every night from `main` with the same set of binaries and VSIXs.

**VS Code users:** platform-specific VSIXs are also available on the releases page — each bundles the server binary so no separate download is needed.

### From Source

> **Note:** Oxide HDL requires Rust with `edition2024` support (Rust 1.85+). The `cargo`/`rustc` packages shipped by most Linux distributions are too old and will produce an error like `feature 'edition2024' is required`. Install Rust via [rustup](https://rustup.rs/) instead:
>
> ```bash
> curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
> ```
>
> If you previously installed Rust via `apt`, remove it first:
>
> ```bash
> sudo apt remove rustc cargo
> ```
>
> After the installer finishes, restart your shell (or run `source ~/.cargo/env`) so that `~/.cargo/bin` is on your `PATH`.

```bash
git clone https://github.com/patgro1/oxide-hdl.git
cd oxide-hdl
cargo build --release
```

The binary will be at `./target/release/oxide-hdl`.

## Editor Setup

### VS Code

Install the platform-specific VSIX from the [Releases](https://github.com/patgro1/oxide-hdl/releases) page:

```
Extensions → ⋯ → Install from VSIX…
```

### Neovim

```lua
vim.lsp.start_client({
  name = "oxide_hdl",
  cmd = { "/path/to/oxide-hdl" },
  root_dir = vim.fs.dirname(
    vim.fs.find({ "oxide.toml", ".git" }, { upward = true })[1]
  ),
})
```

### Emacs (eglot)

```lisp
(add-to-list 'eglot-server-programs
             '(vhdl-ts-mode . ("/path/to/oxide-hdl" "--stdio")))
```

### Sublime Text

Install the [LSP](https://packagecontrol.io/packages/LSP) package via Package Control, then add the following to your LSP settings (`Preferences → Package Settings → LSP → Settings`):

```json
{
  "clients": {
    "oxide-hdl": {
      "enabled": true,
      "command": ["/path/to/oxide-hdl", "--stdio"],
      "selector": "source.vhdl | source.vhd",
      "initializationOptions": {}
    }
  }
}
```

You will also need a VHDL syntax definition. [VHDL](https://packagecontrol.io/packages/VHDL) from Package Control works well and provides the `source.vhdl` scope.

Any editor with standard LSP client support should work with the oxide-hdl binary and `--stdio` transport.

## Features

### Go to Definition

Resolves identifiers to their declaration site across files. Supported targets:

- Signals, variables, constants, generics
- Entity and component declarations
- Types, subtypes, and type aliases
- Package symbols imported via `use` clauses
- Record fields (via dot-notation navigation)
- Subprogram parameters

**Caveat:** Resolution depends on the workspace having been indexed. On very large codebases, go-to-definition may not work immediately on first open while indexing is still running.

### Find All References

Finds every usage of a declared identifier within its visible scope. Works across files when the symbol is declared in a package or entity.

### Rename

Renames an identifier and all its references within the current file. Scope-aware — renaming a local variable inside a process will not affect a signal with the same name in the architecture.

**Caveat:** Rename is file-scoped only. Cross-file rename is not yet implemented — renaming an entity port or a package symbol will not update references in other files.

### Hover

Displays type information and documentation comments above a hovered identifier. Documentation is extracted from `--`-prefixed comments immediately preceding the declaration.

### Document Symbols

Provides a hierarchical outline of the current file: entities, architectures, processes, subprograms, signals, constants, and types. Useful for editors that display a symbol tree or breadcrumb bar.

### Completion

Suggests identifiers visible at the cursor position:

- Local signals, variables, constants, and types
- Entity ports and generics
- Subprogram parameters
- Symbols from imported packages (`use work.my_pkg.all`)
- Process and generate statement snippets
- Component instantiation snippets with port/generic maps pre-filled

**Caveat:** Completion for IEEE standard library symbols (e.g., `std_logic`, `rising_edge`) requires the internal library cache to be extracted on first run. If suggestions are missing on a fresh install, restart the server once.

### Diagnostics

#### Syntax Errors
Reported directly from the Tree-sitter parse. Any construct the grammar cannot parse will produce an error. This catches most typos and structural mistakes immediately.

**Caveat:** The VHDL grammar occasionally produces spurious ERROR nodes for valid but unusual constructs. These are relatively rare and being addressed upstream.

#### Undeclared Identifiers
Reports identifiers that cannot be resolved to any declaration in scope or in imported packages. Checks both the local scope hierarchy and all active `use` clauses.

The following are intentionally not flagged:
- IEEE standard library identifiers (`std_logic`, `rising_edge`, `unsigned`, etc.) — the tree-sitter VHDL grammar parses these as dedicated node kinds (`library_type`, `library_function`, `library_constant`) rather than plain identifiers, so they are never collected as usages in the first place. The IEEE standard library is also bundled with the server as a fallback.
- Port map formal names (`clk => my_clk` — `clk` is a port of the instantiated unit)
- Record aggregate field names (`(field_a => 42)`)
- Record dot-access suffixes (`rec.field`)
- For-loop variables within their loop body
- Identifiers matching patterns in `ignored_identifiers` (see [Configuration](#configuration))

**Caveat:** Cross-file resolution requires the referenced file to be part of the indexed workspace. Files outside the project root or excluded by `ignore` patterns will not be found.

#### Unused Declarations
Reports signals, variables, and constants that are declared but never referenced. Reported at `HINT` severity so they are visible but not intrusive.

**Caveat:** "Used" is determined by presence in a usage expression. Signals that are only written to (never read) are still considered unused.

#### Sensitivity List
Analyzes `process` statements for sensitivity list correctness:

- **Missing signals** (WARNING) — a signal is read in the process body but absent from the sensitivity list
- **Unnecessary signals** (HINT) — a signal is in the sensitivity list but never read in the process body

Goes beyond simple presence checking: understands `if`/`case`/`loop` nesting, function calls, and record field access.

**Caveat:** For processes that call subprograms, the analysis uses parameter direction to determine whether a signal argument is being read. Two situations are treated conservatively to avoid false positives:
- **Overloaded subprograms** — when multiple declarations with the same name exist (overloaded functions/procedures), direction cannot be resolved without full type inference. All signal arguments are treated as reads, which may cause unnecessary-signal false negatives.
- **Unresolved subprograms** — if a subprogram is not found in the workspace, its signal arguments are skipped entirely.

### Code Actions

Quick-fix actions are offered alongside sensitivity list diagnostics:

| Action | Description |
|--------|-------------|
| **Fix sensitivity list** | Adds all missing and removes all unnecessary signals in one edit (shown when both kinds of issues are present) |
| **Add all missing signals** | Adds every missing signal to the list |
| **Remove all unnecessary signals** | Removes every unnecessary signal from the list |
| **Add '\<signal\>'** | Adds a single missing signal |
| **Remove '\<signal\>'** | Removes a single unnecessary signal |

The sensitivity list is rewritten in an opinionated format: signals are filled onto a line up to 120 characters, then wrapped with consistent indentation.

## Configuration

Place an `oxide.toml` file in your project root to configure the server. All fields are optional — if no configuration file is found, the server starts with the defaults shown below.

**Defaults (no `oxide.toml`):**
- Extensions: `vhd`, `vhdl`
- Ignore: `**/build/**`, `**/sim/**`, `**/target/**`, `**/.git/**`, `**/work/**`
- Diagnostics: `on_save`
- Ignored identifiers: none
- Included workspaces: none

```toml
# File extensions recognized as VHDL source files.
# Default: ["vhd", "vhdl"]
extensions = ["vhd", "vhdl"]

# Glob patterns for paths to exclude from workspace indexing.
# Speeds up startup and prevents false positives from generated files.
# Default: ["**/build/**", "**/sim/**", "**/target/**", "**/.git/**", "**/work/**"]
ignore = [
    "**/build/**",
    "**/sim/**",
    "**/target/**",
    "**/.git/**",
    "**/work/**",
]

# When to run diagnostics.
# "on_save"   — only after the file is saved (default, recommended for large projects)
# "on_change" — 300 ms after the last keystroke (more responsive; diagnostics are
#               debounced so partial edits do not produce spurious errors)
diagnostics = "on_save"

# Regex patterns for identifiers to suppress in undeclared-identifier diagnostics.
# Useful for auto-generated constants, tool-specific macros, or synthesis attributes
# that are injected outside the normal VHDL scope system.
# Patterns are case-insensitive.
# Default: [] (nothing suppressed)
ignored_identifiers = [
    "^REG_.*",       # auto-generated register map constants
    "^BUILD_ID$",    # injected by build system
]

# List of external workspace directories to include for indexing.
# This is useful when a repository depends on another repository.
# Default: []
include_workspace = []
```

### Configuration Reference

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `extensions` | `[string]` | `["vhd", "vhdl"]` | File extensions to index as VHDL |
| `ignore` | `[string]` | see above | Glob patterns for paths to exclude |
| `diagnostics` | `string` | `"on_save"` | When to publish diagnostics: `"on_save"` runs after every save; `"on_change"` runs 300 ms after the last keystroke (debounced). |
| `ignored_identifiers` | `[string]` | `[]` | Regex patterns for identifiers to ignore in undeclared checks |
| `include_workspace` | `[string]` | `[]` | List of external workspace directories to include for indexing |

## Known Limitations

**No type checking.** Oxide HDL understands declarations and scopes but does not validate types. Assigning a `std_logic` to an `integer` signal will not produce a diagnostic.

**No port map validation.** Component and entity instantiations are parsed and used for completion/go-to-definition, but port direction and type mismatches are not checked.

**Workspace-scoped only.** Files outside the project root are not indexed. If your design references IP cores or libraries stored elsewhere, you may see false "undeclared" diagnostics for their symbols.

**Single-workspace.** Multi-root workspaces are not supported. If your editor opens multiple folders simultaneously, only the first root will be indexed.

**No unnecessary-signal detection in synchronous processes.** The sensitivity list checker can detect missing signals but does not flag signals that are present in the list but logically unnecessary. In a clocked process gated on `rising_edge(clk)`, only `clk` (and an async reset, if used) is needed — extra signals in the list are redundant but will not be reported.

## Architecture

Oxide HDL uses a two-pass analysis pipeline:

1. **Shallow pass** (`scan_fast`) — a regex-based scanner that runs on every file at startup. Extracts top-level symbol names only. Fast enough to index thousands of files in seconds.
2. **Deep pass** (`extract_document_symbols`) — a full Tree-sitter parse triggered when a file is opened. Builds a hierarchical scope tree with complete declaration and usage tracking.

Key components:

- [tree-sitter](https://tree-sitter.github.io/) — incremental, error-tolerant parsing
- [tower-lsp](https://github.com/ebkalderon/tower-lsp) — async LSP protocol layer
- [tokio](https://tokio.rs/) — async runtime
- Scope trees — hierarchical declaration/usage tracking with cross-file visibility

## Contributing

Bug reports and pull requests are welcome. Check existing issues before starting major work, as some areas are actively being developed.

## License

MIT
