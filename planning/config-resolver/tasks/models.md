# Task: models

**Feature**: config-resolver (FE-07)
**Status**: pending
**Estimated Time**: ~30 minutes
**Depends On**: (none — foundation task)
**Required By**: `resolver`, `tests`

---

## Goal

Audit and finalise the `ConfigResolver` struct definition, its `DEFAULTS` constant, and its public API surface in `src/config.rs` so that downstream tasks (`resolver` and `tests`) have a stable, correct foundation to build on. No logic is implemented here — this task only ensures types, field visibility, and default values are correct per the feature spec before `todo!()` bodies are filled.

---

## Files Involved

| File | Action |
|---|---|
| `src/config.rs` | Modify — add missing default, verify field types and visibility |
| `tests/test_config.rs` | Read only — verify test scaffolding matches the struct surface |

---

## Steps

### 1. Run existing passing tests to establish a baseline

```bash
cd /Users/tercel/WorkSpace/aiperceivable/apcore-cli-rust
cargo test config -- --include-ignored 2>&1 | grep -E "^(test |FAILED|ok |error)"
```

Expected: the two instantiation tests in `src/config.rs` (inline `#[cfg(test)]` block) pass; all `todo!()` tests panic-fail with "not implemented". The integration tests in `tests/test_config.rs` that have `assert!(false)` will also fail — this is expected and will be addressed in the `tests` task.

### 2. Add `cli.auto_approve` to `DEFAULTS`

The feature spec Section 5 lists `cli.auto_approve` as a known configuration key with default `False`. The Python `config.py` includes it on line 25. The current Rust `DEFAULTS` constant (line 36-41 of `src/config.rs`) is missing this entry.

Edit `src/config.rs`, locate `DEFAULTS`:

```rust
// Before:
pub const DEFAULTS: &'static [(&'static str, &'static str)] = &[
    ("extensions.root", "./extensions"),
    ("logging.level", "WARNING"),
    ("sandbox.enabled", "false"),
    ("cli.stdin_buffer_limit", "10485760"),
];

// After:
pub const DEFAULTS: &'static [(&'static str, &'static str)] = &[
    ("extensions.root", "./extensions"),
    ("logging.level", "INFO"),
    ("sandbox.enabled", "false"),
    ("cli.stdin_buffer_limit", "10485760"),
    ("cli.auto_approve", "false"),
];
```

Note: also correct `"WARNING"` → `"INFO"` to match the Python spec (`DEFAULTS["logging.level"] = "INFO"` in `config.py` line 21).

### 3. Verify field types and visibility are appropriate

Check each field in `ConfigResolver`:

- `_cli_flags: HashMap<String, Option<String>>` — correct; `Option<String>` represents "flag present but value not provided" (None) vs "flag provided with value" (Some).
- `_config_file: Option<HashMap<String, String>>` — correct; `None` means file not found or unparseable.
- `config_path: Option<PathBuf>` — private, correct; only needed during construction.
- `defaults: HashMap<&'static str, &'static str>` — correct; lifetime is `'static` since DEFAULTS is a `const`.

No changes needed to field types.

### 4. Verify `flatten_dict` public method signature

The current public signature is:
```rust
pub fn flatten_dict(&self, map: serde_json::Value) -> HashMap<String, String>
```

This takes `serde_json::Value`. Document (as a code comment) that this is the public-facing method for JSON callers, and that the internal YAML flatten path will use a private helper `flatten_yaml_value`. This avoids a public API break while supporting YAML internally.

Add this comment directly above the method in `src/config.rs`:

```rust
// Public flatten method for JSON callers (e.g. tests, external consumers).
// Internal YAML loading uses the private `flatten_yaml_value` helper instead.
pub fn flatten_dict(&self, map: serde_json::Value) -> HashMap<String, String> {
```

No signature change required.

### 5. Confirm `test_defaults_contains_expected_keys` still passes after adding `cli.auto_approve`

The inline unit test only checks four keys — it will still pass. No test changes needed in this task.

```bash
cargo test test_defaults_contains_expected_keys 2>&1 | tail -5
```

Expected: `test tests::test_defaults_contains_expected_keys ... ok`

### 6. Run full test suite and record baseline

```bash
cargo test 2>&1 | tail -20
```

Record the count of passing vs failing tests. The number of passing tests must not decrease from the pre-task baseline.

---

## Acceptance Criteria

- [ ] `DEFAULTS` contains exactly five entries: `extensions.root`, `logging.level`, `sandbox.enabled`, `cli.stdin_buffer_limit`, `cli.auto_approve`
- [ ] `logging.level` default value is `"INFO"` (not `"WARNING"`)
- [ ] `cli.auto_approve` default value is `"false"`
- [ ] `cargo test test_defaults_contains_expected_keys` passes
- [ ] `cargo test test_config_resolver_instantiation` passes
- [ ] No previously-passing tests are broken by this task
- [ ] A comment above `flatten_dict` documents the JSON vs YAML split

---

## Dependencies

- **Depends on**: (none)
- **Required by**: `resolver` (fills in `todo!()` bodies), `tests` (completes integration assertions)
