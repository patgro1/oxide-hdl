# Oxide HDL

A VHDL Language Server Protocol (LSP) implementation written in Rust, focused on large codebases and real-world usability.

## Status: v0.4 (Alpha)

Oxide HDL is functional but actively evolving. Basic LSP features work well, diagnostics are solid, but type system and package support are still in development.

## Why Oxide HDL?

Most VHDL tools try to be full compilers, which means slow startup and heavy memory usage on large projects. Oxide HDL takes a different approach:

- **Fast indexing** using Tree-sitter instead of a full compiler frontend
- **Incremental parsing** - only analyze files you're actively editing
- **Practical diagnostics** - catches real bugs without requiring perfect compile-ability
- **Built for monorepos** - designed to handle thousands of VHDL files without grinding to a halt

If you're working on a large FPGA project and your current tools take minutes to index or constantly crash, Oxide HDL might help.

## Installation

### From Source
```bash
git clone https://github.com/patgro1/oxide-hdl.git
cd oxide-hdl
cargo build --release
```

The binary will be at `./target/release/oxide-hdl`.

### Editor Setup

**Neovim** (with native LSP):
```lua
vim.lsp.start_client({
  name = "oxide_hdl",
  cmd = { "/path/to/oxide-hdl" },
  root_dir = vim.fs.dirname(
    vim.fs.find({'oxide.toml', '.git'}, { upward = true })[1]
  ),
})
```

**Emacs**:
```lisp
(use-package eglot
    :demand t
    :config
    ;; Add custom VHDL language server (with path validation)
    (let ((vhdl-lsp-path "~/Workspace/oxide-hdl/target/release/oxide-hdl"))
      (when (file-executable-p vhdl-lsp-path)
        (add-to-list 'eglot-server-programs
                     `(vhdl-ts-mode . (,vhdl-lsp-path "--stdio")))
        (message "VHDL language server configured: %s" vhdl-lsp-path))))
```

**VS Code**: Use a generic LSP extension and point it to the oxide-hdl binary.

Other editors should work with standard LSP client configurations.

## Features

### Working Well
- **Go to definition** for signals, variables, entities, components
- **Hover** for type information and documentation
- **Document symbols** (outline view with proper nesting)
- **Diagnostics**:
  - Syntax errors from Tree-sitter
  - Unused signals, variables, constants (HINT severity)
  - Incomplete sensitivity lists (WARNING for missing, HINT for unnecessary)
  - Edge detection (rising_edge, falling_edge, clk'event)

### In Development (v0.5)
- Package support (use clauses, IEEE libraries)
- Undeclared identifier detection
- Duplicate declaration detection
- Better cross-file entity resolution

### Planned (v0.6+)
- Full type system with validation
- Type mismatches in assignments
- Port map type checking
- Background indexing and disk cache

## Known Limitations

**No package resolution yet.** Constants and types from `ieee.std_logic_1164` and friends won't be recognized. This means:
- False positives for "undeclared" on standard types
- Incomplete sensitivity list validation for package constants

**Per-file analysis only.** Entities and architectures in separate files won't be fully linked. Most features work, but cross-file validation is limited.

**Conservative error handling.** When in doubt, Oxide HDL stays quiet rather than spamming false positives. This means some real issues might be missed.

**Tree-sitter grammar limitations.** VHDL is complex, and the grammar occasionally produces ERROR nodes for valid code. We're working with upstream to fix these.

## Configuration

Create `oxide.toml` in your project root:
```toml
# File extensions to index
extensions = ["vhd", "vhdl"]

# Paths to ignore (speeds up indexing)
ignore = [
    "**/build/**",
    "**/simulation/**",
    "**/work/**",
    "**/.git/**",
]
```

## Contributing

Bug reports and PRs welcome. The codebase is being actively refactored (v0.5 development), so check existing issues before starting major work.

Main development happens in feature branches, merged incrementally to keep main stable.

## Architecture

- **Tree-sitter** for parsing (fast, incremental, error-tolerant)
- **Scope trees** for tracking declarations and visibility
- **tower-lsp** for LSP protocol handling
- **Modular diagnostics** - each lint is independent

See `/src/analysis` for the core semantic analysis code.

## License

MIT
