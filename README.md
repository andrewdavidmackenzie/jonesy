# Jonesy: "Don't Panic!"

Jonesy analyzes Rust binaries to find all code paths that can lead to a panic, helping developers understand where panics
can originate in their code.

**[Documentation](https://jonesy.mackenzie-serres.net/)** | **[Panic Reference](https://jonesy.mackenzie-serres.net/panics/)**

Focus is currently on getting something useful working. I work on macOS and ARM64, so that's what implemented, but I
definitely want to make it
cross-platform and multi-architecture in the future, but will probably need help from others on Linux and Mac.

## Installation

### Via cargo-binstall (recommended, fastest)

```bash
cargo binstall jonesy
```

This downloads a pre-built binary from GitHub releases.

### Via cargo install (from crates.io)

```bash
cargo install jonesy
```

### From source

```bash
git clone https://github.com/andrewdavidmackenzie/jonesy
cd jonesy
cargo install --path jonesy
```

### Direct download

Download the latest release from [GitHub Releases](https://github.com/andrewdavidmackenzie/jonesy/releases):

```bash
curl -LO https://github.com/andrewdavidmackenzie/jonesy/releases/latest/download/jonesy-macos-arm64
chmod +x jonesy-macos-arm64
mv jonesy-macos-arm64 /usr/local/bin/jonesy
```

> **Note:** Currently only macOS ARM64 (Apple Silicon) is supported.

## Usage

### From a Crate Directory

Run jonesy from the root of any Rust crate (where `Cargo.toml` is located):

```bash
cd my-crate
cargo build
jonesy
```

Jonesy will parse `Cargo.toml` to find the package name and binary targets, then analyze all binaries found in
`target/debug/`.

### From a Workspace Root

When run from a workspace root, jonesy analyzes all workspace member binaries:

```bash
cd my-workspace
cargo build
jonesy
```

### Analyzing a Specific Binary

Use `--bin` to analyze a specific binary file:

```bash
jonesy --bin target/debug/my-binary
```

### Analyzing Libraries

Jonesy can analyze Rust libraries built as dynamic libraries (`.dylib`):

```bash
jonesy --lib target/debug/libmy_lib.dylib
```

**Library Setup Requirements:**

For jonesy to analyze a library, it must be built as a `cdylib` with exported symbols:

1. Add `cdylib` to your crate types in `Cargo.toml`:
   ```toml
   [lib]
   crate-type = ["rlib", "cdylib"]
   ```

2. Mark functions to export with `#[no_mangle]`:
   ```rust
   #[unsafe(no_mangle)]
   pub fn my_library_function() {
       // ...
   }
   ```

3. Build and create dSYM:
   ```bash
   cargo build
   dsymutil target/debug/libmy_lib.dylib -o target/debug/libmy_lib.dSYM
   ```

**Why `cdylib` + `#[no_mangle]`?**

There are two ways to build a Rust dynamic library:

| Type     | Size   | `pub fn` exported?        | Analysis speed |
|----------|--------|---------------------------|----------------|
| `cdylib` | ~16KB  | No (needs `#[no_mangle]`) | Fast           |
| `dylib`  | ~1.4MB | Yes (automatic)           | Very slow      |

- **`cdylib`** creates a minimal C-compatible library. Only explicitly marked functions are exported; others are removed
  by dead code elimination. Analysis is fast because only your code is included.

- **`dylib`** creates a full Rust dynamic library including the standard library runtime. All `pub fn` are exported
  automatically, but the ~90x larger binary makes analysis impractical (minutes vs seconds).

**Other notes:**

- `.rlib` files (Rust library archives) are fully supported for panic detection
- `.a` (staticlib) files work but only detect panics in `#[no_mangle]` exported functions (see [Limitations](#static-libraries-a-and-dce))
- The dSYM bundle provides debug symbols for source location information

## Command Line Options

```
Usage:
  jonesy [OPTIONS]
  jonesy [OPTIONS] --bin <path_to_binary>
  jonesy [OPTIONS] --lib <path_to_lib_object>
  jonesy lsp

Subcommands:
  lsp                Start the LSP server for IDE integration

Options:
  --tree             Show full call tree instead of just crate code points
  --summary-only     Only show summary, not detailed panic points
  --format <fmt>     Output format: text (default), json, or html
  --config <path>    Path to a TOML config file for allow/deny rules
  --max-threads N    Maximum threads for parallel analysis (default: CPU count)
  --no-hyperlinks    Disable terminal hyperlinks (use plain relative paths for CI)
  --quiet            Suppress progress messages
  --bin              Analyze a specific binary file
  --lib              Analyze a specific library object file
  --version, -V      Print version and exit
```

### `--tree`

By default, jonesy shows only the panic code points in your crate's source code. Use `--tree` to see the full call tree
from `rust_panic` up to your code:

```bash
jonesy --tree
```

Example output with `--tree`:

```text
Full call tree:
__rustc::rust_panic
Called from: 'panic_with_hook' (source: library/std/src/panicking.rs:796)
    Called from: '{closure#0}' (source: library/std/src/panicking.rs:698)
        ...
            Called from: 'panic_fmt' (source: library/core/src/panicking.rs:55)
                Called from: 'main' (source: src/main.rs:8)
```

### `--summary-only`

Show only the summary without detailed panic point locations. Useful for CI pipelines or quick checks:

```bash
jonesy --summary-only
```

Example output:

```text
Summary:
  Project: my-app
  Root: /path/to/project
  Panic points: 5 in 2 file(s)
```

### `--no-hyperlinks`

When stdout is a terminal, jonesy outputs source file locations
as [OSC 8 terminal hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda), making paths
clickable in supported terminals (iTerm2, Kitty, WezTerm, VS Code terminal, and others). The link points to the full
file path while displaying a shorter relative path.

When output is piped or redirected (e.g., `jonesy > file.txt`), plain relative paths are used automatically to avoid
escape sequences in logs or files. This is also the recommended mode for CI pipelines.

If your terminal doesn't support OSC 8 hyperlinks (e.g. macOS Terminal.app), the escape sequences will be invisible and
the output will still be readable. However, if you prefer plain relative paths even in an interactive terminal, use this
flag:

```bash
jonesy --no-hyperlinks
```

This outputs relative paths like `src/main.rs:42:1` instead of clickable hyperlinks, which is compatible with
GitHub Actions problem matchers for inline PR annotations. See [CI Integration](#ci-integration-github-actions).

### `--config`

Specify a custom TOML configuration file for allow/deny rules:

```bash
jonesy --config my-config.toml
```

See the [Configuration](#configuration) section for details on the config file format.

### `--format json`

Output results as machine-readable JSON instead of human-readable text:

```bash
jonesy --format json
```

The JSON output includes a versioned schema for compatibility:

```json
{
  "version": "1.2",
  "jonesy_version": "0.5.0",
  "project": {
    "name": "my-crate",
    "root": "/path/to/project"
  },
  "summary": {
    "panic_points": 5,
    "files_affected": 2
  },
  "panic_points": [
    {
      "file": "src/main.rs",
      "line": 10,
      "column": 5,
      "function": "main",
      "causes": [
        {
          "code": "JP006",
          "type": "unwrap",
          "description": "unwrap() on None",
          "docs_url": "https://jonesy.mackenzie-serres.net/panics/JP006-unwrap-none",
          "suggestion": "Use if let, match, unwrap_or, or ? operator instead"
        }
      ]
    }
  ]
}
```

Each panic point may have multiple causes when different panic paths converge at the same location.

The `--tree` and `--summary-only` flags work with JSON output:

- `--format json` — Flat list of panic points (no call tree)
- `--format json --tree` — Full hierarchical tree with `children` arrays
- `--format json --summary-only` — Summary only, empty `panic_points` array

#### Workspace JSON Output

For workspaces, JSON output uses schema version `1.2` with a hierarchical structure:

```json
{
  "version": "1.2",
  "jonesy_version": "0.5.0",
  "workspace": {
    "root": "/path/to/workspace",
    "members": [
      {
        "name": "crate-a",
        "path": "crate-a",
        "summary": { "panic_points": 5, "files_affected": 2 },
        "panic_points": [...]
      }
    ]
  },
  "summary": {
    "total_panic_points": 8,
    "total_files_affected": 3,
    "members_analyzed": 2
  }
}
```

### `--format html`

Generate a self-contained HTML report that can be viewed in any browser:

```bash
jonesy --format html > report.html
```

The HTML report includes:
- Project summary with panic point counts
- Clickable `file://` links to source locations
- Visual hierarchy for panic call chains (with `--tree`)
- Panic cause descriptions and suggestions
- Dark theme with responsive design

The `--tree` and `--summary-only` flags work with HTML output:

- `--format html` — Flat list of panic points
- `--format html --tree` — Full hierarchical tree with nested children
- `--format html --summary-only` — Summary statistics only, no panic point list

For workspaces, HTML output includes collapsible sections for each workspace member with individual summaries and panic points.

## Configuration

Jonesy supports configuring which panic causes to report (deny) or suppress (allow). This is useful for:

- Suppressing known-acceptable panics in your codebase
- Enforcing stricter rules (e.g. reporting drop panics)
- Per-project customization

### Configuration Cascade

Configuration is loaded in order of precedence (later overrides earlier):

1. **Code defaults** - `drop` and `unwind` panics are allowed; all others are denied
2. **Cargo.toml** - `[package.metadata.jonesy]` section
3. **jonesy.toml** - Project root config file
4. **`--config`** - Command-line override

### Panic Cause Identifiers

| ID              | Description                               | Default     | Clippy Lint |
|-----------------|-------------------------------------------|-------------|-------------|
| `panic`         | Explicit `panic!()` calls                 | denied      | `clippy::panic` |
| `bounds`        | Array/slice index out of bounds           | denied      | `clippy::indexing_slicing` |
| `overflow`      | All arithmetic/shift overflow (matches `div_overflow`, `rem_overflow`, `shift_overflow`) | denied | `clippy::arithmetic_side_effects` |
| `div_overflow`  | Division overflow specifically            | denied      | `clippy::arithmetic_side_effects` |
| `rem_overflow`  | Remainder overflow specifically           | denied      | `clippy::arithmetic_side_effects` |
| `shift_overflow`| Shift overflow (shl/shr)                  | denied      | `clippy::arithmetic_side_effects` |
| `div_zero`      | Division by zero                          | denied      | `clippy::arithmetic_side_effects` |
| `unwrap`        | `unwrap()` on `None` or `Err`             | denied      | `clippy::unwrap_used` |
| `expect`        | `expect()` on `None` or `Err`             | denied      | `clippy::expect_used` |
| `assert`        | `assert!()` failures                      | denied      | — |
| `debug_assert`  | `debug_assert!()` failures                | denied      | — |
| `unreachable`   | `unreachable!()` reached                  | denied      | `clippy::unreachable` |
| `unimplemented` | `unimplemented!()` reached                | denied      | `clippy::unimplemented` |
| `todo`          | `todo!()` reached                         | denied      | `clippy::todo` |
| `drop`          | Panic during drop/cleanup                 | **allowed** | — |
| `unwind`        | Panic in no-unwind context                | **allowed** | — |
| `format`        | Formatting error (Display/Debug panic)    | denied      | — |
| `capacity`      | Capacity overflow (collection too large)  | denied      | — |
| `oom`           | Out of memory (allocation failed)         | denied      | — |
| `str_slice`     | String/slice encoding or bounds error     | denied      | — |
| `invalid_enum`  | Invalid enum discriminant (unsafe code)   | denied      | — |
| `misaligned_ptr`| Misaligned pointer dereference            | denied      | — |
| `unknown`       | Unknown panic cause                       | denied      | — |

Clippy lints are "restriction" lints (off by default). Enable in `Cargo.toml`:

```toml
[lints.clippy]
unwrap_used = "warn"
expect_used = "warn"
indexing_slicing = "warn"
panic = "warn"
```

Clippy's static analysis may produce false positives, while jonesy only reports actual panic paths in the compiled binary.

### jones.toml Format

Create a `jones.toml` file in your project root:

```toml
# Allow specific panic causes (suppress from output)
allow = ["drop", "unwind", "debug_assert"]

# Deny specific panic causes (report in output)
deny = ["todo", "unimplemented"]
```

### Cargo.toml Format

Add configuration to your `Cargo.toml` under `[package.metadata.jonesy]`:

```toml
[package]
name = "my-crate"
version = "0.1.0"

[package.metadata.jonesy]
allow = ["drop", "unwind"]
deny = ["todo"]
```

### Example: Strict Mode

To report all panic causes including drops:

```toml
# jonesy.toml
deny = ["drop", "unwind"]
```

### Example: Lenient Development Mode

To allow common development panics:

```toml
# jonesy.toml
allow = ["todo", "unimplemented", "debug_assert"]
```

### Scoped Rules

Scoped rules let you allow or deny panic causes in specific files or functions using glob patterns:

```toml
# jonesy.toml

# Allow all panics in test files
[[rules]]
path = "**/tests/**"
allow = ["*"]

# Allow explicit panics only in main.rs
[[rules]]
path = "**/main.rs"
allow = ["panic"]

# Allow unwrap in a specific function
[[rules]]
function = "*::parse_config"
allow = ["unwrap", "expect"]
```

#### Pattern Matching

- **Path patterns** match against the full source file path (e.g., `/path/to/src/main.rs`)
- **Function patterns** match against function names (e.g., `main`, `my_mod::helper`)
- Use `*` for single component matching, `**` for directory wildcard
- Use `"*"` in allow/deny to match all panic causes

#### Rule Precedence

When multiple rules match, more specific rules take precedence:

1. **Function patterns** are more specific than path patterns
2. **Longer patterns** (more literal characters) are more specific
3. Rules with both path and function patterns are most specific

Within equal specificity, later rules in the config file override earlier ones.

### Inline Allow Comments

For fine-grained control, you can add `// jonesy:allow(cause)` comments directly in your source code:

```rust
fn setup_config() {
    // Allow unwrap on a value we know is valid
    let config = load_config().unwrap(); // jonesy:allow(unwrap)

    // Allow multiple causes
    let value = data.unwrap(); // jonesy:allow(unwrap, bounds)

    // Allow all panic causes at this line
    risky_operation(); // jonesy:allow(*)
}
```

The comment applies to the line it's on. Due to DWARF debug info sometimes being slightly off, jonesy checks a small range around the reported line number (±2 lines).

**Available cause IDs:** `panic`, `bounds`, `overflow`, `div_overflow`, `rem_overflow`, `shift_overflow`, `div_zero`, `unwrap`, `expect`, `assert`, `debug_assert`, `unreachable`, `unimplemented`, `todo`, `format`, `capacity`, `oom`, `str_slice`, `invalid_enum`, `misaligned_ptr`, `drop`, `unwind`, `unknown`

> **Tip for constant divisors:** If you have divisions with compile-time constant non-zero divisors (e.g., `x / 60`), you can suppress false positive warnings with `allow = ["div_overflow", "div_zero"]` in a scoped rule.

Use `*` to allow all causes at that location.

### Phantom Async Filtering

By default, jonesy filters out "phantom" panic points from empty async functions. These are false positives caused by Rust's generated async state machine code having drop handlers with panic paths that can never actually be triggered by user code.

For example, `async fn empty() {}` compiles to a state machine with `drop_in_place` handlers that technically have panic paths (like misaligned pointer dereference), but these cannot be reached from user code.

**Criteria for filtering:**
- Function name ends with `{async_fn#N}` (generated async state machine)
- Only cause is Unknown (no specific panic identified)
- No children (no real panic-inducing code in the call chain)

To disable this filtering and see all potential panic points including phantoms:

```toml
# jonesy.toml
filter_phantom_async = false
```

#### Configuring Linters and Code Review Tools

Code review tools like CodeRabbit may flag `// jonesy:allow(...)` comments as "spurious" or "undocumented annotations" because they don't recognize jonesy directives. To prevent this, configure your tools to ignore these comments.

**CodeRabbit** (`.coderabbit.yaml` in your repository root):

```yaml
reviews:
  path_filters:
    # Don't flag jonesy inline allow comments
    - "!**/*.rs"  # Or use path_instructions instead

  path_instructions:
    - path: "**/*.rs"
      instructions: |
        Ignore comments matching the pattern `// jonesy:allow(...)` - these are
        valid directives for the jonesy panic analysis tool, not spurious comments.
```

Alternatively, add to your PR template or repository guidelines that `jonesy:allow` comments are intentional tool directives.

**Clippy**: Jonesy comments don't affect Clippy. If you use `#[allow(...)]` attributes for Clippy, those are separate from jonesy's inline comments.

**Other tools**: Most linters can be configured via ignore patterns or inline disable comments. The key is to document that `jonesy:allow` is a recognized directive in your project.

#### Example: Test-Friendly Configuration

```toml
# jonesy.toml

# Default: deny all (no global allow)

# Allow everything in tests
[[rules]]
path = "**/tests/**"
allow = ["*"]

# Allow debug_assert everywhere
[[rules]]
path = "**/*"
allow = ["debug_assert"]
```

## Exit Status

Jonesy exits with the number of panic code points found:

- `0` - No panics found (code "passed")
- `N` - N panic code points found

This makes it easy to use jonesy in CI pipelines:

```bash
jonesy || echo "Found potential panics!"
```

## CI Integration (GitHub Actions)

Jonesy provides a GitHub Action that shows panic points as inline annotations on PR diffs.

### Quick Setup

Add a workflow file (`.github/workflows/jonesy.yml`):

```yaml
name: Jonesy Analysis

on: [pull_request, push]

jobs:
  analyze:
    runs-on: macos-latest  # jonesy currently requires macOS
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable

      - name: Build project
        run: cargo build

      - name: Run jonesy
        uses: andrewdavidmackenzie/jonesy@v1
```

### Action Inputs

| Input | Description | Default |
|-------|-------------|---------|
| `fail-on-panic` | Fail the workflow if panic points are found | `false` |
| `working-directory` | Directory to run analysis in | `.` |
| `binary` | Specific binary to analyze | auto-detect |
| `extra-args` | Additional arguments to pass to jonesy | |
| `comment-on-pr` | Post a summary comment on pull requests | `true` |

### Permissions

To post PR comments, your workflow needs `pull-requests: write` permission:

```yaml
jobs:
  analyze:
    runs-on: macos-latest
    permissions:
      pull-requests: write
    steps:
      # ...
```

### Action Outputs

| Output | Description |
|--------|-------------|
| `panic-count` | Number of panic points found |

### Examples

**Fail on any panics:**

```yaml
- uses: andrewdavidmackenzie/jonesy@v1
  with:
    fail-on-panic: true
```

**Analyze a specific binary:**

```yaml
- uses: andrewdavidmackenzie/jonesy@v1
  with:
    binary: target/debug/my-app
```

**Use panic count in subsequent steps:**

```yaml
- name: Run jonesy
  id: jonesy
  uses: andrewdavidmackenzie/jonesy@v1

- name: Check threshold
  if: steps.jonesy.outputs.panic-count > 10
  run: |
    echo "::error::Too many panics: ${{ steps.jonesy.outputs.panic-count }}"
    exit 1
```

### How It Works

- The action registers a problem matcher that parses jonesy output
- Panic points appear as warnings directly on the affected lines in PR diffs
- Output uses relative paths for proper annotation linking

## Example Output

For a crate with multiple panic paths:

```text
Processing /path/to/target/debug/my-app
Using .dSYM bundle for debug info

Panic code points in crate:
 --> /path/to/src/main.rs:9:1 [explicit panic!() call]
     = help: Review if panic is intentional or add error handling
 --> /path/to/src/main.rs:13:1
     └──  --> /path/to/src/module/mod.rs:3:1
 --> /path/to/src/main.rs:16:1
     └──  --> /path/to/src/module/mod.rs:7:1 [unwrap() on None]
          = help: Use if let, match, unwrap_or, or ? operator instead

Summary:
  Project: my-app
  Root: /path/to
  Panic points: 5 in 2 file(s)
```

For a panic-free crate:

```text
Processing /path/to/target/debug/perfect
Using .dSYM bundle for debug info

No panics in crate

Summary:
  Project: perfect
  Root: /path/to
  Panic points: 0 in 0 file(s)
```

### Direct vs Indirect Panics

Jonesy distinguishes between **direct** and **indirect** panics, providing different suggestions for each:

**Direct panic** — Your code directly calls a panic-triggering function:

```rust
let value = some_option.unwrap();  // Direct call to unwrap()
```

```
--> src/main.rs:42:1 [JP006: unwrap() on None]
    = help: Use if let, match, unwrap_or, or ? operator instead
```

**Indirect panic** — Your code calls a function that may panic internally:

```rust
let mut builder = Builder::from_default_env();
builder.filter_level(level).init();  // init() internally calls unwrap()
```

```
--> src/main.rs:70:1 [JP007: unwrap() on Err]
    = help: This calls a function that may call unwrap(). Consider a fallible alternative (e.g., try_*)
```

There's no visible `unwrap()` on line 70, but this is correct! The `env_logger::Builder::init()` method internally calls `try_init().unwrap()`. Jonesy identifies that calling `init()` can lead to a panic, even though the `unwrap()` is inside another function.

For indirect panics, the suggestion recommends using a fallible alternative when available:

```rust
builder.filter_level(level).try_init().ok();  // Won't panic
```

Common fallible alternatives:
- `Mutex::lock()` → `Mutex::try_lock()`
- `Vec::reserve()` → `Vec::try_reserve()`
- `env_logger::init()` → `env_logger::try_init()`
- `thread::spawn().join().unwrap()` → handle the `Result`

Jonesy helps you find these hidden panic paths so you can decide whether to use fallible alternatives or accept the panic risk.

## Requirements

- macOS with ARM64 (Apple Silicon)—currently the only supported platform
- Debug symbols (build with `cargo build`, not release mode without debug info)

## Using on macOS

Jonesy needs DWARF debug information to map code addresses to source file locations. On macOS, Jonesy automatically
handles this for you:

### Automatic dSYM Generation

When no `.dSYM` bundle exists, Jonesy automatically runs `dsymutil` (if it is present) to generate one, if not it will
attempt (on macOS) to fall back to the "Debug Map" method.

in your project run:

```bash
cargo build
jonesy
```

Jonesy will output "Generated .dSYM bundle for debug info" when it creates one.

### Why is this needed?

By default, macOS Rust builds use Apple's "lazy" DWARF scheme:

- Debug info stays in object files (`target/debug/deps/*.o`)
- The final binary only contains a "debug map" pointing to those files
- `dsymutil` combines everything into a `.dSYM` bundle

Jonesy automatically runs `dsymutil` when needed, so you don't have to.

### Optional: Pre-generate dSYM in Cargo

If you want Cargo to create dSYM bundles during build (avoiding Jonesy's auto-generation), add to `Cargo.toml`:

```toml
[profile.dev]
split-debuginfo = "packed"
```

**Trade-off:** This slightly slows incremental builds because `dsymutil` runs on every build.

See [description.md](description.md) for detailed technical documentation.

## IDE Integration (LSP Server)

Jonesy includes a Language Server Protocol (LSP) server that integrates with IDEs and code editors to show panic point diagnostics inline.

### Starting the LSP Server

```bash
jonesy lsp
```

The LSP server communicates via stdin/stdout using the standard LSP protocol.

### Features

- **Diagnostics**: Panic points appear as warnings in your editor
- **Quick fixes**: Click on a diagnostic to see code actions for silencing panic points
- **Auto-refresh**: Analysis runs on initialization and when files are saved
- **Manual refresh**: Trigger re-analysis with the `jonesy.analyze` command

### Quick Fix Actions

When you click on a jonesy diagnostic, you'll see quick fix options:

- **"Allow '{cause}' on this line"** - Inserts `// jonesy:allow({cause})` comment
- **"Allow '{cause}' in {file}"** - Adds a scoped rule to `jonesy.toml`
- **"Allow '{cause}' in function '{name}'"** - Adds a function-scoped rule to `jonesy.toml`
- **"Allow all panics on this line"** - Inserts `// jonesy:allow(*)` for multiple causes

These actions integrate with the [scoped rules](#scoped-rules) and [inline allow comments](#inline-allow-comments) features.

### VS Code Setup

VS Code requires a language server client extension to connect to `jonesy lsp`. You can use the [Generic LSP Client](https://marketplace.visualstudio.com/items?itemName=llllvvuu.llllvvuu-glspc) extension or similar.

After installing an LSP client extension, configure it in `.vscode/settings.json`:

```json
{
  "glspc.serverCommand": "jonesy lsp",
  "glspc.languageId": "rust"
}
```

The exact configuration varies by extension. The key is to run `jonesy lsp` and associate it with Rust files.

### RustRover / IntelliJ Setup

RustRover and IntelliJ IDEA require the [LSP4IJ plugin](https://plugins.jetbrains.com/plugin/23257-lsp4ij) to add custom language servers.

**Install the plugin:**

1. Go to **Settings** → **Plugins** → **Marketplace**
2. Search for "LSP4IJ"
3. Click **Install** and restart the IDE

**Configure jonesy:**

1. Go to **Settings** → **Languages & Frameworks** → **Language Servers**
2. Click **+** to add a new server
3. In the **Server** tab:
   - **Name**: `jonesy`
   - **Command**: `jonesy lsp`
4. In the **Mappings** tab, add a file name pattern:
   - Click **+** in the file name patterns section
   - Add `*.rs`
5. Click **OK** to save

The configuration is stored at the application level, so jonesy will be available in all your Rust projects. The IDE will automatically start `jonesy lsp` when you open Rust files. Jonesy diagnostics will appear alongside rust-analyzer's analysis.

**Note:** The configuration is stored in `~/Library/Application Support/JetBrains/<version>/options/UserDefinedLanguageServerSettings.xml` on macOS. Project-level `.idea/lsp.json` files are not supported by LSP4IJ.

### Other Editors

The LSP server works with any editor that supports the Language Server Protocol:

- **Neovim**: Configure with `nvim-lspconfig`
- **Emacs**: Use `lsp-mode` or `eglot`
- **Sublime Text**: Use the LSP package
- **Helix**: Add to `languages.toml`

Example Neovim configuration:

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

configs.jonesy = {
  default_config = {
    cmd = { 'jonesy', 'lsp' },
    filetypes = { 'rust' },
    root_dir = lspconfig.util.root_pattern('Cargo.toml'),
  },
}

lspconfig.jonesy.setup({})
```

### How It Works

The LSP server:
1. Finds workspace binaries in `target/debug/`
2. Runs jonesy analysis on each binary
3. Publishes diagnostics to the editor with file locations and panic causes
4. Watches `target/debug/` for binary changes and re-analyzes automatically
5. Shows analysis progress in the IDE status bar (for IDEs that support LSP progress)

The server watches for binary changes rather than re-analyzing on every file save. This means analysis only runs when you build your project, avoiding redundant work.

The progress indicator shows which target is being analyzed (e.g., "Analyzing flowc (2/5)") and displays the final result ("Found 293 panic points in 42 files").

Note: The LSP server runs alongside rust-analyzer—it doesn't replace it. You'll see both rust-analyzer's diagnostics and jonesy's panic point warnings.

## Limitations

1. **ARM64 only**: Currently only supports ARM64 binaries (uses `bl` instruction detection)
2. **Direct calls only**: Only detects direct function calls, not indirect calls through function pointers
3. **macOS/Mach-O**: Currently only supports Mach-O binaries with dSYM or embedded DWARF
4. **Debug builds recommended**: Optimized builds may inline functions, affecting accuracy

### Library-Only Analysis Limitations

When analyzing library-only crates (rlib/staticlib) without binary entry points using `--lib`:

1. **Relocation-based detection**: Uses ARM64 branch relocations to find panic callers, which works differently from binary call tree analysis

2. **Line number precision**: For calls to standard library functions (like `Option::unwrap`), the reported line number is the function definition rather than the exact call site within the function

3. <a id="static-libraries-a-and-dce"></a>**Static libraries (`.a`) and DCE**: Static libraries are designed for C FFI. Only functions exported with `#[no_mangle]` are preserved - other functions are eliminated by dead code elimination (DCE) since C code cannot call mangled Rust symbols. This is correct behavior: jonesy reports only reachable panic points.

   ```rust
   // This panic WILL be detected (function preserved for C FFI)
   #[no_mangle]
   pub extern "C" fn exported_function() {
       panic!("reachable from C");
   }

   // This panic will NOT be detected (DCE removes unreachable code)
   pub fn internal_function() {
       panic!("unreachable from C");
   }
   ```

### Detected Panic Types in Library Mode

The following panic patterns are detected in library-only analysis:
- `panic!()`, `assert!()`, `assert_eq!()`, `assert_ne!()`
- `debug_assert!()`, `debug_assert_eq!()`, `debug_assert_ne!()`
- `unreachable!()`, `unimplemented!()`
- `Option::unwrap()`, `Option::expect()`
- `Result::unwrap()`, `Result::expect()`, `Result::unwrap_err()`, `Result::expect_err()`
- Division by zero, arithmetic overflow, shift overflow
- Slice index out of bounds

See the **[Panic Reference](https://jonesy.mackenzie-serres.net/panics/)** for detailed documentation of each panic type (JP001-JP022), including examples and how to avoid them.

See [SCENARIOS.md](SCENARIOS.md) for detailed documentation of all analysis scenarios, supported panic types, and implementation status.
