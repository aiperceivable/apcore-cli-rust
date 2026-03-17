# Task: Change resolve_refs signature from &mut Value to &Value

## Goal

Tighten the `resolve_refs` signature to take `&Value` instead of `&mut Value`. The function already clones the input on the first line, so it never mutates the caller's value. This change makes the contract explicit and removes unnecessary mutability requirements at call sites.

## Files Involved

- `src/ref_resolver.rs` -- function signature change
- `src/lib.rs` -- re-export (no change needed, re-exports the function name)
- `src/cli.rs` -- any internal call sites (search for `resolve_refs`)
- `tests/` -- all test call sites that pass `&mut schema`

## Steps (TDD)

1. **Update signature** in `src/ref_resolver.rs`:
   ```rust
   pub fn resolve_refs(
       schema: &Value,       // was: &mut Value
       max_depth: usize,
       module_id: &str,
   ) -> Result<Value, RefResolverError> {
   ```

2. **Update all call sites** -- remove `mut` from variable bindings where no longer needed:
   - `src/ref_resolver.rs` unit tests: change `let mut schema = json!({...})` to `let schema = json!({...})` and `&mut schema` to `&schema`.
   - Any call sites in `src/cli.rs` (search for `resolve_refs`).
   - Any integration tests in `tests/`.

3. **Run tests**: `cargo test` -- all ref_resolver tests must pass unchanged (behavior is identical).

4. **Run clippy**: `cargo clippy -- -D warnings` -- no `unnecessary_mut` warnings.

## Acceptance Criteria

- `resolve_refs` accepts `&Value` (shared reference).
- All existing tests pass without behavioral changes.
- No `mut` bindings remain solely for the purpose of calling `resolve_refs`.
- Clippy clean.

## Dependencies

None. This is an independent refactoring task.

## Estimated Time

~20 minutes
