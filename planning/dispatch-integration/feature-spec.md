# Dispatch Integration & Code Cleanup

> Final integration feature: wire dynamic module dispatch, register exec subcommand,
> and address remaining suggestion-level review items.
> Source: review consistency gaps + suggestion-level issues
> Created: 2026-03-17

## Purpose

Complete the remaining integration gaps and code quality items identified in the
project review:

### Priority 1 — Dispatch Integration (plan consistency gaps)
1. **Wire dynamic module dispatch** — connect the external subcommand match arm in
   `main.rs` to `dispatch_module` in `cli.rs`, so `apcore-cli math.add --a 5 --b 10`
   actually executes the module instead of exiting 44.
2. **Register `exec` subcommand** — the plan specifies `exec` as a first-class
   subcommand but it is not registered in main.rs.

### Priority 2 — Suggestion-level cleanup
3. Gate `MockRegistry` behind `#[cfg(test)]` feature flag
4. Change `resolve_refs` signature from `&mut Value` to `&Value`
5. Remove unused test helpers in `tests/common/mod.rs`
6. Remove unused import `SchemaArgs` in `tests/test_schema_parser.rs`
7. `validate_extensions_dir` should return `Result` instead of calling `process::exit`

## Constraints

- All existing 444 tests must continue to pass
- No breaking changes to the public API surface
- `serde_yaml` migration deferred to a separate feature
