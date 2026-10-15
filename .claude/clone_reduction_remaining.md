# Remaining Clone Reduction Opportunities

Saved from the clone scan analysis for future work.

## Point 3: CrateCodePoint.name and node.name.clone() (call_tree.rs lines 387, 458)

Building `CrateCodePoint` from `CodePointInfo` borrowed from the `DashMap`. The `DashMap` 
ref-guard prevents moving the data out, so cloning is required. Each clone involves:
- `name: String` -- heap alloc (~50 bytes avg function name)
- `causes: HashSet<PanicCause>` -- heap alloc (hash table)

Called once per unique code point. Fix: intern names as indices (same as point 1).
If names are interned, causes clone remains but is bounded to ~2K per binary.

## Point 4: causes.clone() (call_tree.rs line 391)

`causes: HashSet<PanicCause>` is cloned from CodePointInfo into CrateCodePoint.
Could use `Arc<HashSet<PanicCause>>` for shared ownership without cloning, but
adds indirection. ~2K per binary. Low priority.

## Point 5: sym_name.to_string() (sym.rs lines 158, 211, 281)

Symbol name allocation in find_symbol_containing / find_all_symbols_with_addresses.
Called per matching symbol (~100 matches). Negligible impact.

## Point 6: DWARF string extraction (function_index.rs, 8 sites)

`.to_string_lossy()?.into_owned()` -- NECESSARY. gimli borrowed slices have lifetimes
tied to unit/header. No alternative exists.

## Point 7: gimli program.clone().rows() (full_line_table.rs, crate_line_table.rs)

NECESSARY per gimli's API -- consuming iteration requires cloning the program.

## Point 8: StringTables bidirectional map clones (string_tables.rs lines 41, 57, etc.)

Inherent cost of maintaining both Vec and HashMap for the same strings.
Each unique string cloned exactly once. Bounded cost.

## Point 9: Workspace output cloning (main.rs lines 357-360)

member_code_points.clone(), member_file_paths.clone() in text output mode.
Fix: build WorkspaceMemberResult first, borrow for text output.
Only affects text mode workspace output. Low priority.

## Tier 2 structural ideas (larger changes)

- Intern `CallTreeNode.name` as u32 index -- covered by point 1 implementation
- Use `HashSet<u16>` in AnalysisSummary internal collection instead of resolving
  to String during traversal
