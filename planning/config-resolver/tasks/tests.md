# Task: tests

**Feature**: config-resolver (FE-07)
**Status**: pending
**Estimated Time**: ~1 hour
**Depends On**: `resolver`
**Required By**: (none — final task)

---

## Goal

Replace every `assert!(false, "not implemented")` stub in `tests/test_config.rs` with real assertions that fully exercise the 4-tier precedence logic, YAML loading, flattening, and edge cases defined in the feature spec (T-CFG-01 through T-CFG-09). All nine spec test cases must be covered. After this task, `cargo test` passes with zero failures.

---

## Files Involved

| File | Action |
|---|---|
| `tests/test_config.rs` | Modify — replace all `assert!(false)` stubs with real test bodies |
| `tests/common/mod.rs` | Read only — uses `strip_apcore_env_vars()` helper |

---

## Steps

### 1. Confirm resolver task is complete (prerequisite check)

```bash
cargo test --lib 2>&1 | grep -E "FAILED|todo"
```

Must return no lines. If any `todo!()` panics remain, complete the `resolver` task first.

### 2. Implement `test_resolve_tier1_cli_flag_wins` (T-CFG-01)

```rust
#[test]
fn test_resolve_tier1_cli_flag_wins() {
    common::strip_apcore_env_vars();
    // Also set the env var so we can prove CLI flag beats it.
    unsafe { std::env::set_var("APCORE_EXTENSIONS_ROOT", "/env-path") };

    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap();

    let mut flags = HashMap::new();
    flags.insert("--extensions-dir".to_string(), Some("/cli-path".to_string()));
    let resolver = ConfigResolver::new(Some(flags), Some(config_path));

    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("APCORE_EXTENSIONS_ROOT"),
    );
    assert_eq!(result, Some("/cli-path".to_string()), "CLI flag must win");

    unsafe { std::env::remove_var("APCORE_EXTENSIONS_ROOT") };
}
```

### 3. Implement `test_resolve_tier2_env_var_wins` (T-CFG-02)

```rust
#[test]
fn test_resolve_tier2_env_var_wins() {
    common::strip_apcore_env_vars();
    unsafe { std::env::set_var("APCORE_EXTENSIONS_ROOT", "/env-path") };

    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap();

    // No CLI flags provided.
    let resolver = ConfigResolver::new(None, Some(config_path));
    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("APCORE_EXTENSIONS_ROOT"),
    );
    assert_eq!(result, Some("/env-path".to_string()), "env var must win over config file");

    unsafe { std::env::remove_var("APCORE_EXTENSIONS_ROOT") };
}
```

### 4. Implement `test_resolve_tier3_config_file_wins` (T-CFG-03)

Replace the existing partial stub:

```rust
#[test]
fn test_resolve_tier3_config_file_wins() {
    common::strip_apcore_env_vars();

    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap();

    let resolver = ConfigResolver::new(None, Some(config_path));
    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("APCORE_EXTENSIONS_ROOT"),
    );
    assert_eq!(
        result,
        Some("/config-path".to_string()),
        "config file must win over default"
    );
}
```

### 5. Implement `test_resolve_tier4_default_wins` (T-CFG-04)

Replace the existing partial stub:

```rust
#[test]
fn test_resolve_tier4_default_wins() {
    common::strip_apcore_env_vars();
    let resolver = ConfigResolver::new(None, Some(PathBuf::from("/nonexistent/apcore.yaml")));
    let result = resolver.resolve("extensions.root", None, None);
    assert_eq!(
        result,
        Some("./extensions".to_string()),
        "built-in default must be returned"
    );
}
```

### 6. Implement `test_resolve_cli_flag_none_skips_tier1` (T-CFG-09)

```rust
#[test]
fn test_resolve_cli_flag_none_skips_tier1() {
    common::strip_apcore_env_vars();
    unsafe { std::env::set_var("APCORE_EXTENSIONS_ROOT", "/env-path") };

    // CLI flag is registered with value None — must be skipped.
    let mut flags = HashMap::new();
    flags.insert("--extensions-dir".to_string(), None);
    let resolver = ConfigResolver::new(Some(flags), None);
    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("APCORE_EXTENSIONS_ROOT"),
    );
    assert_eq!(result, Some("/env-path".to_string()), "None CLI flag must fall through to env var");

    unsafe { std::env::remove_var("APCORE_EXTENSIONS_ROOT") };
}
```

### 7. Implement `test_resolve_env_var_empty_string_skips_tier2` (T-CFG-08)

```rust
#[test]
fn test_resolve_env_var_empty_string_skips_tier2() {
    common::strip_apcore_env_vars();
    unsafe { std::env::set_var("APCORE_EXTENSIONS_ROOT", "") };

    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap();

    let resolver = ConfigResolver::new(None, Some(config_path));
    let result = resolver.resolve(
        "extensions.root",
        None,
        Some("APCORE_EXTENSIONS_ROOT"),
    );
    assert_eq!(
        result,
        Some("/config-path".to_string()),
        "empty-string env var must fall through to config file"
    );

    unsafe { std::env::remove_var("APCORE_EXTENSIONS_ROOT") };
}
```

### 8. Implement `test_load_config_file_valid_yaml` (T-CFG-07)

Replace the existing partial stub:

```rust
#[test]
fn test_load_config_file_valid_yaml() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(
        &config_path,
        "extensions:\n  root: /custom/path\nlogging:\n  level: DEBUG\n",
    )
    .unwrap();

    let resolver = ConfigResolver::new(None, Some(config_path));
    let file_map = resolver._config_file.as_ref().expect("config file must be loaded");
    assert_eq!(file_map.get("extensions.root"), Some(&"/custom/path".to_string()));
    assert_eq!(file_map.get("logging.level"), Some(&"DEBUG".to_string()));
}
```

### 9. Implement `test_flatten_dict_nested` and `test_flatten_dict_deeply_nested`

```rust
#[test]
fn test_flatten_dict_nested() {
    let resolver = ConfigResolver::new(None, None);
    let map = serde_json::json!({"extensions": {"root": "/path"}});
    let result = resolver.flatten_dict(map);
    assert_eq!(result.get("extensions.root"), Some(&"/path".to_string()));
    assert_eq!(result.len(), 1);
}

#[test]
fn test_flatten_dict_deeply_nested() {
    let resolver = ConfigResolver::new(None, None);
    let map = serde_json::json!({"a": {"b": {"c": "deep_value"}}});
    let result = resolver.flatten_dict(map);
    assert_eq!(result.get("a.b.c"), Some(&"deep_value".to_string()));
    assert_eq!(result.len(), 1);
}
```

### 10. Add missing edge-case tests not in the current scaffold

Add these at the bottom of `tests/test_config.rs` to cover spec rows not currently stubbed:

```rust
// T-CFG-05: config file does not exist — no panic, defaults used.
#[test]
fn test_config_file_not_found_silent() {
    common::strip_apcore_env_vars();
    let resolver = ConfigResolver::new(None, Some(PathBuf::from("/this/does/not/exist.yaml")));
    assert!(resolver._config_file.is_none());
    // Must still resolve to default without panicking.
    let result = resolver.resolve("extensions.root", None, None);
    assert_eq!(result, Some("./extensions".to_string()));
}

// T-CFG-06: config file is invalid YAML — WARNING emitted, defaults used.
#[test]
fn test_config_file_malformed_yaml() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    // Invalid YAML: unbalanced braces.
    std::fs::write(&config_path, "{ invalid yaml: [unclosed").unwrap();

    let resolver = ConfigResolver::new(None, Some(config_path));
    assert!(
        resolver._config_file.is_none(),
        "malformed YAML must result in None config_file"
    );
}

// YAML root is a list, not a dict — treated as malformed.
#[test]
fn test_config_file_root_not_dict() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "- item1\n- item2\n").unwrap();

    let resolver = ConfigResolver::new(None, Some(config_path));
    assert!(
        resolver._config_file.is_none(),
        "list root must result in None config_file"
    );
}

// Unknown key returns None.
// (Already scaffolded as test_resolve_unknown_key_returns_none — verify it passes.)
```

### 11. Run full test suite (GREEN)

```bash
cargo test 2>&1 | tail -20
```

All tests must pass. If any `assert!(false)` or `todo!()` errors remain, fix them before declaring complete.

```bash
cargo test 2>&1 | grep -c "^test .* ok"
cargo test 2>&1 | grep "FAILED" || echo "no failures"
```

### 12. Final check: no stubs remain

```bash
grep -n "assert!(false" /Users/tercel/WorkSpace/aiperceivable/apcore-cli-rust/tests/test_config.rs
grep -n "todo!" /Users/tercel/WorkSpace/aiperceivable/apcore-cli-rust/src/config.rs
```

Both commands must return no output.

---

## Acceptance Criteria

- [ ] All `assert!(false, "not implemented")` lines in `tests/test_config.rs` are replaced
- [ ] `cargo test` reports zero test failures
- [ ] T-CFG-01: CLI flag beats env var, config file, and default
- [ ] T-CFG-02: Env var beats config file and default (no CLI flag)
- [ ] T-CFG-03: Config file beats default (no CLI flag, no env var)
- [ ] T-CFG-04: Built-in default is returned when no other tier matches
- [ ] T-CFG-05: Missing config file — no panic, `_config_file` is `None`
- [ ] T-CFG-06: Malformed YAML — no panic, `_config_file` is `None`
- [ ] T-CFG-07: Nested YAML flattened to dot-notation correctly
- [ ] T-CFG-08: Empty-string env var falls through to config file
- [ ] T-CFG-09: CLI flag value of `None` falls through to env var
- [ ] YAML root that is a list (not a dict) is treated as malformed
- [ ] `test_resolve_unknown_key_returns_none` passes
- [ ] `test_load_config_file_not_found` passes (already had partial implementation)
- [ ] `test_config_resolver_with_cli_flags` passes

---

## Dependencies

- **Depends on**: `resolver` (logic must be implemented before assertions can verify it)
- **Required by**: (none — this is the final task for this feature)
