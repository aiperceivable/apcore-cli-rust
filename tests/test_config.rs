// apcore-cli — Integration tests for ConfigResolver.
// Protocol spec: FE-07

mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use apcore_cli::config::ConfigResolver;
use tempfile::tempdir;

#[test]
fn test_config_resolver_instantiation() {
    let resolver = ConfigResolver::new(None, None);
    assert!(!resolver.defaults.is_empty());
}

#[test]
fn test_config_resolver_with_cli_flags() {
    let mut flags = HashMap::new();
    flags.insert("--extensions-dir".to_string(), Some("/cli".to_string()));
    let resolver = ConfigResolver::new(Some(flags.clone()), None);
    assert_eq!(resolver.cli_flags, flags);
}

#[test]
fn test_defaults_contains_expected_keys() {
    // Audit D9 (v0.6.x): sandbox.enabled and cli.stdin_buffer_limit were
    // removed because they were never read by resolve() at runtime. The
    // canonical set of resolvable keys is verified by the unit tests in
    // src/config.rs (test_defaults_contains_expected_keys + test_deleted_keys_absent).
    let resolver = ConfigResolver::new(None, None);
    for key in [
        "extensions.root",
        "logging.level",
        "cli.help_text_max_length",
        "cli.approval_timeout",
        "cli.strategy",
        "cli.group_depth",
    ] {
        assert!(
            resolver.defaults.contains_key(key),
            "missing default: {key}"
        );
    }
}

// T-CFG-01: CLI flag must beat env var, config file, and default.
// Uses a non-APCORE_ prefixed env var so strip_apcore_env_vars() in other
// parallel tests cannot interfere.
#[test]
fn test_resolve_tier1_cli_flag_wins() {
    unsafe { std::env::set_var("TCFG01_EXT_ROOT", "/env-path") };

    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap(); // safe: tempdir is writable

    let mut flags = HashMap::new();
    flags.insert(
        "--extensions-dir".to_string(),
        Some("/cli-path".to_string()),
    );
    let resolver = ConfigResolver::new(Some(flags), Some(config_path));

    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("TCFG01_EXT_ROOT"),
    );
    assert_eq!(result, Some("/cli-path".to_string()), "CLI flag must win");

    unsafe { std::env::remove_var("TCFG01_EXT_ROOT") };
}

// T-CFG-02: Env var must beat config file and default (no CLI flag).
// Uses a non-APCORE_ prefixed env var so strip_apcore_env_vars() in other
// parallel tests cannot interfere.
#[test]
fn test_resolve_tier2_env_var_wins() {
    unsafe { std::env::set_var("TCFG02_EXT_ROOT", "/env-path") };

    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap(); // safe: tempdir is writable

    // No CLI flags provided.
    let resolver = ConfigResolver::new(None, Some(config_path));
    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("TCFG02_EXT_ROOT"),
    );
    assert_eq!(
        result,
        Some("/env-path".to_string()),
        "env var must win over config file"
    );

    unsafe { std::env::remove_var("TCFG02_EXT_ROOT") };
}

// T-CFG-03: Config file must beat default when no CLI flag or env var is set.
#[test]
fn test_resolve_tier3_config_file_wins() {
    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap(); // safe: tempdir is writable

    let resolver = ConfigResolver::new(None, Some(config_path));
    // Pass env_var=None so no env lookup is performed; only config file and defaults apply.
    let result = resolver.resolve("extensions.root", None, None);
    assert_eq!(
        result,
        Some("/config-path".to_string()),
        "config file must win over default"
    );
}

// T-CFG-04: Built-in default must be returned when no other tier provides a value.
#[test]
fn test_resolve_tier4_default_wins() {
    let resolver = ConfigResolver::new(None, Some(PathBuf::from("/nonexistent/apcore.yaml")));
    // Pass no env_var so no env lookup is performed.
    let result = resolver.resolve("extensions.root", None, None);
    assert_eq!(
        result,
        Some("./extensions".to_string()),
        "built-in default must be returned"
    );
}

// T-CFG-09: A CLI flag entry with value None must be skipped (fall through to tier 2).
// Uses a non-APCORE_ prefixed env var so strip_apcore_env_vars() in other
// parallel tests cannot interfere.
#[test]
fn test_resolve_cli_flag_none_skips_tier1() {
    unsafe { std::env::set_var("TCFG09_EXT_ROOT", "/env-path") };

    // CLI flag is registered with value None — must be skipped.
    let mut flags = HashMap::new();
    flags.insert("--extensions-dir".to_string(), None);
    let resolver = ConfigResolver::new(Some(flags), None);
    let result = resolver.resolve(
        "extensions.root",
        Some("--extensions-dir"),
        Some("TCFG09_EXT_ROOT"),
    );
    assert_eq!(
        result,
        Some("/env-path".to_string()),
        "None CLI flag must fall through to env var"
    );

    unsafe { std::env::remove_var("TCFG09_EXT_ROOT") };
}

// T-CFG-08: An empty-string env var must be treated as unset (fall through to tier 3).
// Uses a non-APCORE_ prefixed env var so strip_apcore_env_vars() in other
// parallel tests cannot interfere.
#[test]
fn test_resolve_env_var_empty_string_skips_tier2() {
    unsafe { std::env::set_var("TCFG08_EXT_ROOT", "") };

    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "extensions:\n  root: /config-path\n").unwrap(); // safe: tempdir is writable

    let resolver = ConfigResolver::new(None, Some(config_path));
    let result = resolver.resolve("extensions.root", None, Some("TCFG08_EXT_ROOT"));
    assert_eq!(
        result,
        Some("/config-path".to_string()),
        "empty-string env var must fall through to config file"
    );

    unsafe { std::env::remove_var("TCFG08_EXT_ROOT") };
}

#[test]
fn test_resolve_unknown_key_returns_none() {
    let resolver = ConfigResolver::new(None, None);
    // No env var lookup and no config file; only defaults apply.
    // "nonexistent.key" has no default, so None is expected.
    let result = resolver.resolve("nonexistent.key", None, None);
    assert!(result.is_none());
}

// T-CFG-07: Valid YAML file must be loaded and flattened to dot-notation keys.
#[test]
fn test_load_config_file_valid_yaml() {
    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(
        &config_path,
        "extensions:\n  root: /custom/path\nlogging:\n  level: DEBUG\n",
    )
    .unwrap(); // safe: tempdir is writable

    let resolver = ConfigResolver::new(None, Some(config_path));
    let file_map = resolver
        .config_file
        .as_ref()
        .expect("config file must be loaded");
    assert_eq!(
        file_map.get("extensions.root"),
        Some(&"/custom/path".to_string())
    );
    assert_eq!(file_map.get("logging.level"), Some(&"DEBUG".to_string()));
}

#[test]
fn test_load_config_file_not_found() {
    let resolver = ConfigResolver::new(None, Some(PathBuf::from("/nonexistent/apcore.yaml")));
    assert!(resolver.config_file.is_none());
}

// T-CFG-05: Missing config file — no panic, _config_file is None.
#[test]
fn test_config_file_not_found_silent() {
    let resolver = ConfigResolver::new(None, Some(PathBuf::from("/this/does/not/exist.yaml")));
    assert!(resolver.config_file.is_none());
    // Must still resolve to default without panicking; no env var lookup.
    let result = resolver.resolve("extensions.root", None, None);
    assert_eq!(result, Some("./extensions".to_string()));
}

// T-CFG-06: Malformed YAML — no panic, _config_file is None.
#[test]
fn test_config_file_malformed_yaml() {
    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    // Invalid YAML: unbalanced braces.
    std::fs::write(&config_path, "{ invalid yaml: [unclosed").unwrap(); // safe: tempdir is writable

    let resolver = ConfigResolver::new(None, Some(config_path));
    assert!(
        resolver.config_file.is_none(),
        "malformed YAML must result in None config_file"
    );
}

// YAML root is a list, not a dict — treated as malformed.
#[test]
fn test_config_file_root_not_dict() {
    let dir = tempdir().unwrap(); // safe: tempdir creation is infallible in practice
    let config_path = dir.path().join("apcore.yaml");
    std::fs::write(&config_path, "- item1\n- item2\n").unwrap(); // safe: tempdir is writable

    let resolver = ConfigResolver::new(None, Some(config_path));
    assert!(
        resolver.config_file.is_none(),
        "list root must result in None config_file"
    );
}

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
