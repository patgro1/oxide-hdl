# Oxide HDL

**A blazingly fast, crash-proof VHDL Language Server Protocol (LSP) implementation written in Rust.**

> **Status:** v0.2 (Alpha)
> **Focus:** Large Monorepos, Instant Startup, Stability.

Oxide HDL is designed for hardware engineers working with massive VHDL codebases (3,000+ files) who are tired of waiting minutes for their editor to index. It prioritizes **Navigation Speed** and **Editor Responsiveness** over strict compiler-level validation.

## 🚀 Why Oxide HDL?

Existing VHDL tools often try to compile the entire world on startup, leading to massive RAM usage and long delays. Oxide HDL takes a **Hybrid Approach**:

1.  **Instant Startup (~100ms):** Uses a multi-threaded **Regex Scanner** to map the global namespace (Entities, Packages) without parsing syntax.
2.  **Deep Parsing on Demand:** Uses **Tree-sitter** only when you open a file to provide rich syntax highlighting, structure, and local navigation.
3.  **Just-In-Time (JIT) Resolution:** When you hover over a dependency (e.g., `u_inst : entity work.uart`), Oxide HDL parses the target file in the background (ms latency) to show you ports and generics instantly.

## ✨ Features

* **⚡ Blazing Fast Indexing:** Indexes thousands of files in sub-seconds.
* **🔍 Go to Definition:**
    * Jump to Signals/Variables (Local scope).
    * Jump to Entities/Components (Global scope).
    * Jump to Functions/Procedures (even inside Packages).
* **📦 Rich Hover:**
    * Hovering an **Instantiation** shows the target Entity's **Ports and Generics**.
    * Hovering a **Function** shows the full **Signature** (Arguments & Return type).
* **📑 Document Symbols:** Full support for "Outline View" and "Breadcrumbs" with correct nesting (Architecture -> Process -> Variable).
* **🛡️ Crash Proof:** Built with robust mutex protection to handle the non-thread-safe nature of VHDL C-grammars safely.

## ⚙️ Configuration (`oxide.toml`)

Oxide HDL looks for an `oxide.toml` file in the root of your workspace. You can use this to filter out build artifacts and simulation logs.

**Example `oxide.toml`:**

```toml
# List of file extensions to index
extensions = ["vhd", "vhdl"]

# Glob patterns to ignore (gitignore style)
# Critical for performance in large repos with generated IP or sim logs
ignore = [
    "**/build/**",
    "**/simulation/**",
    "**/work/**",
    "**/incremental_db/**",
    "**/.git/**",
    "**/*.bak"
]
```

### 4. Installation & Build

## 📦 Installation

*(Instructions for building from source)*

```bash
git clone [https://github.com/yourname/oxide-hdl.git](https://github.com/yourname/oxide-hdl.git)
cd oxide-hdl
cargo build --release
The binary will be located at ./target/release/oxide-hdl.
```


### 5. Editor Setup (Neovim)

## EDITOR Setup

### Neovim (Native LSP)

Add this to your `init.lua` or LSP configuration:

```lua
local client = vim.lsp.start_client({
  name = "oxide_hdl",
  cmd = { "/path/to/oxide-hdl/target/release/oxide-hdl" },
  root_dir = vim.fs.dirname(vim.fs.find({'oxide.toml', '.git'}, { upward = true })[1]),
  on_attach = function(client, bufnr)
    -- Enable completion, hover, definition, etc.
    vim.api.nvim_buf_set_option(bufnr, 'omnifunc', 'v:lua.vim.lsp.omnifunc')
  end,
})

if not client then
  vim.notify("Failed to start Oxide HDL")
else
  vim.api.nvim_create_autocmd("FileType", {
    pattern = "vhdl",
    callback = function()
      vim.lsp.buf_attach_client(0, client)
    end,
  })
end
```
VS Code

Currently requires a generic LSP extension (like "Glspc") configured to point to the oxide-hdl binary.


### 6. Architecture, Roadmap, License

## 🏗️ Architecture

Oxide HDL uses a **Controller-Service** architecture to manage concurrency:

* **`backend/mod.rs` (Controller):** Handles JSON-RPC communication and manages the Global Mutex for the parser.
* **`backend/workspace.rs` (Indexer):** Manages the file system, JIT parsing, and the Regex engine.
* **`backend/syntax/parser.rs` (Visitor):** A recursive AST walker that converts Tree-sitter nodes into a hierarchical symbol tree.
* **`backend/features/`:** Contains pure logic for Hover formatting, Fuzzy Lookup, and Completion.

## 📝 Roadmap

* [x] **v0.1:** Hybrid Indexing, Go-to-Def, Rich Hover, Outline.
* [x] **v0.2:** Basic Auto-Completion (Local signals and context detection in entities).
* [ ] **v0.3:** Diagnostics (Linter for undefined signals).
* [ ] **v0.4:** Smart Auto-Import (Add `use` clauses automatically).

## 📜 License

MIT
