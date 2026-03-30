# Project

Jonesy resides at https://github.com/andrewdavidmackenzie/jonesy, and its purpose
is to analyse a debug rust binary and detect all the points in the user's code where a 
panic could occur.

There are descriptions of features in README.md, as well as user-facing docs in docs folder
and more technical details in description.md, SCENARIOS.md, features_and_tests.md, notes.md,
BENCHMARK.md, benchmark_flow_workspace.md.

## General Considerations

1. Allow Claude to say "I don't know" if it can't find information to confirm a
conclusion or answer, or can't quote sources for a statement when needed. I
prefer no answer than one that may mislead us.

2. Verify with Citations. Make sure you can explain any conclusions you have reached
by being able to cite the source information and then explain the logic used.

3. Use direct quotes for factual grounding.

## Workflow Rules

- Never commit to master/main branch, always use a feature branch and create a PR.
- Always wait for code reviews to terminate, or be repeated if they failed due to
  rate limiting, and then address all comments from the review.
- Always wait for the human user to approve before you merge a PR.
- Don't close GitHub issues without the user's explicit approval.
- Don't change Rust versions or install or uninstall anything using rustup without the user's explicit approval.
- Don't add new crate dependencies without the user's explicit approval.
- Always run `make test` (not just `cargo test`) before pushing,
since the Makefile builds nested workspaces (like `examples/workspace_test`) that aren't part of the
  main workspace.
- Always run `make clippy` and `cargo fmt` before committing or pushing changes.
- Explain your analysis of the problem, and proposed implementation plan before starting to 
implement changes. Describe what files will be modified what functions added/deleted/modifes

## Coding Rules

- **macOS aarch64 only** — jonesy only supports macOS aarch64. Don't add cross-platform
  concerns or conditional compilation for other targets.
- **Heuristics belong in `heuristics.rs`** — detection logic based on stdlib function
  names or file paths (e.g., `detect_panic_cause`, `is_panic_triggering_function`)
  belongs in the heuristics module, not scattered across other files.
- **Reuse existing analysis functions** — LSP and CLI should use the same
  `analyze_macho`/`analyze_archive` functions, not duplicate analysis logic.
- **When adding a new `PanicCause` variant**, also add: error code (JPxxx), `docs_slug`,
  `docs_url`, suggestion (direct + indirect), all output format support
  (text/json/html/lsp), and a documentation page in `docs/panics/`.
- **Update `examples/panic/` when adding new panic detection** — add a test case
  function that exercises the new detection, with a `// jonesy: expect panic` marker.
- attempt a reasonable level of code re-use, detect functions that are similar and combine them
with parameters if they can be
- Use rust canonical code where possible. Implement `From` traits for conversion, create structs
with methods, use traits when multiple implementations may be needed, etc.

## Testing Rules

- Don't assume that any test failure is independent of your change. We usually start
  a new feature branch from master where tests were working.
- Use `make test` not `cargo test` — the Makefile builds nested workspaces that aren't
  part of the main workspace.
- When changing line numbers in example files, update all hardcoded line references
  in tests.
- Integration tests use `// jonesy: expect panic` markers — the marker-based test
  system validates detection. New panic types need markers in the example source files.