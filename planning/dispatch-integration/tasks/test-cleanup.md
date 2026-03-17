# Task: Gate MockRegistry behind cfg(test) and remove dead test code

## Goal

1. Gate `MockRegistry` and `mock_module` behind `#[cfg(test)]` in `discovery.rs`.
2. Remove unused test helpers in `tests/common/mod.rs`.
3. Remove unused `SchemaArgs` import in `tests/test_schema_parser.rs`.

## Files Involved

- `src/discovery.rs` -- `MockRegistry` struct, impl, and `mock_module` function
- `src/lib.rs` -- re-export of `MockRegistry` and `mock_module`
- `tests/common/mod.rs` -- unused helpers
- `tests/test_schema_parser.rs` -- unused import

## Steps (TDD)

### Part 1: Gate MockRegistry

1. In `src/discovery.rs`, wrap `MockRegistry` and `mock_module` in `#[cfg(test)]`:
   ```rust
   #[cfg(test)]
   pub struct MockRegistry { ... }

   #[cfg(test)]
   impl MockRegistry { ... }

   #[cfg(test)]
   impl RegistryProvider for MockRegistry { ... }

   #[cfg(test)]
   pub fn mock_module(...) -> Value { ... }
   ```

2. In `src/lib.rs`, gate the re-exports:
   ```rust
   #[cfg(test)]
   #[doc(hidden)]
   pub use discovery::{mock_module, MockRegistry};
   ```

3. Check if any integration tests in `tests/` use `MockRegistry` from the crate root. If so, those tests need to either:
   - Import from `apcore_cli::discovery::MockRegistry` (which is `pub` within the crate for unit tests), OR
   - Maintain their own mock, OR
   - The items remain available via `#[cfg(test)]` since integration tests compile with the test cfg.

   **Note**: `#[cfg(test)]` in `lib.rs` does NOT apply to integration tests (they are separate crates). If integration tests need `MockRegistry`, keep it behind `#[doc(hidden)]` without `#[cfg(test)]`, or use a `test-support` feature flag. Based on the grep, no integration tests currently use `MockRegistry`, so `#[cfg(test)]` gating is safe.

4. **Run `cargo test`** to verify all unit tests in `discovery.rs` still compile and pass.

5. **Run `cargo build`** (release) to verify `MockRegistry` is excluded from production builds.

### Part 2: Remove unused helpers in tests/common/mod.rs

1. Verify no test file calls `sample_module_descriptor`, `sample_module_with_schema`, `sample_exec_result`, or `strip_apcore_env_vars`. (Confirmed by grep: zero usages outside the definition file.)

2. Remove all four functions from `tests/common/mod.rs`.

3. Remove the unused imports (`HashMap`, `json`, `Value`) if they become unused.

4. If `tests/common/mod.rs` becomes empty, consider whether to delete the file entirely or leave a comment placeholder. Check if any test file has `mod common;` -- if so, the file must exist (even if empty) or the `mod common;` declarations must be removed.

5. **Run `cargo test`** to verify nothing breaks.

### Part 3: Remove unused SchemaArgs import

1. In `tests/test_schema_parser.rs`, change line 8 from:
   ```rust
   use apcore_cli::schema_parser::{reconvert_enum_values, schema_to_clap_args, SchemaArgs};
   ```
   to:
   ```rust
   use apcore_cli::schema_parser::{reconvert_enum_values, schema_to_clap_args};
   ```

2. **Run `cargo test`** and **`cargo clippy`** to confirm clean.

## Acceptance Criteria

- `MockRegistry` and `mock_module` are not present in `cargo build --release` output (gated behind `#[cfg(test)]`).
- All discovery unit tests still pass (they run under `#[cfg(test)]`).
- `tests/common/mod.rs` contains no unused helper functions.
- `tests/test_schema_parser.rs` has no unused imports.
- `cargo clippy -- -D warnings` is clean.

## Dependencies

None. This is an independent cleanup task.

## Estimated Time

~20 minutes
