---
layout: default
title: IDE Integration
---

# IDE Integration (LSP)

Jonesy includes a Language Server Protocol (LSP) server that shows panic point diagnostics directly in your IDE.

## Quick Start

```bash
jonesy lsp
```

The LSP server communicates via stdin/stdout and works with any LSP-compatible editor.

## Features

- **Inline diagnostics** - See panic points as warnings in your code
- **Quick fixes** - Add `// jonesy:allow` comments or config rules directly from the IDE
- **Auto-refresh** - Re-analyzes when binaries or config change
- **Workspace support** - Analyzes all binaries and libraries in Cargo workspaces

## Automatic Re-Analysis

The LSP server automatically re-analyzes when relevant files change. This uses native file watching for reliability.

### Binary/Library Changes (in `target/debug/`)

| File Type | Pattern | Trigger Reason |
|-----------|---------|----------------|
| Binary executable | `target/debug/<name>` | Main crate binary rebuilt |
| Rust library | `target/debug/lib<name>.rlib` | Library crate rebuilt |
| Dynamic library | `target/debug/lib<name>.dylib` (macOS) | cdylib/dylib rebuilt |
| Dynamic library | `target/debug/lib<name>.so` (Linux) | cdylib/dylib rebuilt |
| Static library | `target/debug/lib<name>.a` | staticlib rebuilt |

### Config File Changes

| File | Location | Trigger Reason |
|------|----------|----------------|
| `jonesy.toml` | Workspace root | Allow/deny rules changed |
| `Cargo.toml` | Workspace root | Workspace members changed |
| `Cargo.toml` | Each member crate | Dependencies or features changed |

### Manual Refresh

You can also trigger analysis manually:
- **VS Code**: Command Palette → "Run Jonesy Panic Analysis"
- **RustRover**: Actions → search "jonesy"
- **Neovim**: `:lua vim.lsp.buf.execute_command({command = "jonesy.analyze"})`

### Incremental Analysis Cache

The LSP caches analysis results in `target/jonesy/cache.json` to avoid re-analyzing unchanged targets:

- **Binary mtime tracking** - Skips targets whose modification time hasn't changed
- **Workspace state** - Detects member/target additions, removals, and path changes
- **Smart invalidation** - Only re-analyzes affected targets when `Cargo.toml` changes

For large workspaces, this can significantly reduce re-analysis time after incremental builds.

![Jonesy LSP showing panic diagnostics with quick fix options](./assets/images/lsp-quickfix.png)

## Editor Setup

### VS Code

Install a generic LSP client extension (e.g., [Generic LSP Client](https://marketplace.visualstudio.com/items?itemName=llllvvuu.llllvvuu-glspc)), then configure:

```json
{
  "glspc.serverCommand": "jonesy lsp",
  "glspc.languageId": "rust"
}
```

### RustRover / IntelliJ

1. Install the [LSP4IJ plugin](https://plugins.jetbrains.com/plugin/23257-lsp4ij)
2. Go to **Settings → Languages & Frameworks → Language Servers**
3. Add a new server:
   - **Name**: Jonesy
   - **Command**: `jonesy lsp`
   - **File patterns**: `*.rs`

### Neovim

```lua
require('lspconfig').jonesy = {
  default_config = {
    cmd = { 'jonesy', 'lsp' },
    filetypes = { 'rust' },
    root_dir = function(fname)
      return require('lspconfig').util.find_git_ancestor(fname)
    end,
  },
}
require('lspconfig').jonesy.setup{}
```

## Configuration

Create a `.jonesy.toml` in your project root to customize behavior:

```toml
# Allow specific panic types globally
allow = ["unwrap", "expect"]

# Scoped rules for specific paths
[[rules]]
path = "tests/*"
allow = ["*"]

[[rules]]
path = "src/main.rs"
function = "handle_*"
deny = ["unwrap"]
```

See the [full documentation](https://github.com/andrewdavidmackenzie/jonesy#ide-integration-lsp-server) for more details.
