//! Integration tests for the `builtin_group` module (FE-13 — `apcli` group
//! visibility resolution). Audit D5-002 — adds a dedicated `tests/` peer to
//! complement the in-file unit tests; exercises the four-tier resolution
//! contract through the public surface that downstream embedders consume.

use apcore_cli::{
    ApcliConfig, ApcliGroup, ApcliGroupError, ApcliMode, APCLI_SUBCOMMAND_NAMES,
    RESERVED_GROUP_NAMES,
};

#[test]
fn apcli_subcommand_names_constant_is_nonempty_and_includes_core_names() {
    // Spec §4.2: the canonical subcommand list seeds the include/exclude
    // unknown-name warning. Treat it as an exported contract — empty would
    // silently break the warning path.
    assert!(!APCLI_SUBCOMMAND_NAMES.is_empty());
    for required in ["list", "describe", "exec", "config"] {
        assert!(
            APCLI_SUBCOMMAND_NAMES.contains(&required),
            "APCLI_SUBCOMMAND_NAMES must contain '{required}', got {APCLI_SUBCOMMAND_NAMES:?}"
        );
    }
}

#[test]
fn reserved_group_names_includes_apcli() {
    assert!(
        RESERVED_GROUP_NAMES.contains(&"apcli"),
        "spec §4.2 — 'apcli' must be reserved, got {RESERVED_GROUP_NAMES:?}"
    );
}

#[test]
fn from_cli_config_all_overrides_auto_detect() {
    // Tier 1 wins outright when non-Auto, regardless of registry_injected.
    let g = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::All,
            disable_env: true,
        }),
        true, // would normally auto-detect to "none"
    );
    assert_eq!(g.resolve_visibility(), "all");
    assert!(g.is_group_visible());
}

#[test]
fn from_cli_config_none_hides_group() {
    let g = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::None,
            disable_env: true,
        }),
        false,
    );
    assert_eq!(g.resolve_visibility(), "none");
    assert!(!g.is_group_visible());
}

#[test]
fn from_cli_config_include_filters_subcommands() {
    let g = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Include(vec!["list".into(), "describe".into()]),
            disable_env: true,
        }),
        false,
    );
    assert_eq!(g.resolve_visibility(), "include");
    assert!(g.is_subcommand_included("list"));
    assert!(g.is_subcommand_included("describe"));
    assert!(!g.is_subcommand_included("exec"));
}

#[test]
fn from_cli_config_exclude_filters_subcommands() {
    let g = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Exclude(vec!["exec".into()]),
            disable_env: true,
        }),
        false,
    );
    assert_eq!(g.resolve_visibility(), "exclude");
    assert!(g.is_subcommand_included("list"));
    assert!(!g.is_subcommand_included("exec"));
}

#[test]
fn auto_detect_falls_back_to_all_when_registry_not_injected() {
    // Tier 4 — when nothing else applies and registry is not injected the
    // group must surface (standalone CLI mode). disable_env=true seals
    // Tier 2 to keep this test independent of the host environment.
    let g = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Auto,
            disable_env: true,
        }),
        false,
    );
    assert_eq!(g.resolve_visibility(), "all");
}

#[test]
fn auto_detect_falls_back_to_none_when_registry_injected() {
    // Tier 4 — registry-injected (embedded) hides apcli by default.
    let g = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Auto,
            disable_env: true,
        }),
        true,
    );
    assert_eq!(g.resolve_visibility(), "none");
}

#[test]
fn try_from_yaml_rejects_invalid_mode_value() {
    // Spec §7 error table — validation surface used by the YAML loader.
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str("mode: not-a-mode").unwrap();
    let result = ApcliGroup::try_from_yaml(Some(yaml), false);
    assert!(
        matches!(result, Err(ApcliGroupError::ModeInvalid(_))),
        "unknown apcli.mode must be ModeInvalid, got {result:?}"
    );
}

#[test]
fn try_from_yaml_rejects_non_string_mode() {
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str("mode: 42").unwrap();
    let result = ApcliGroup::try_from_yaml(Some(yaml), false);
    assert!(
        matches!(result, Err(ApcliGroupError::ModeNotString(_))),
        "non-string apcli.mode must be ModeNotString, got {result:?}"
    );
}

// -----------------------------------------------------------------------------
// FE-13: built-in group rename (D11-001)
// -----------------------------------------------------------------------------

use apcore_cli::{
    effective_reserved_group_names, is_reserved_group_name, set_reserved_group_names,
    validate_builtin_group_name, DEFAULT_BUILTIN_GROUP_NAME,
};

#[test]
fn default_builtin_group_name_constant() {
    assert_eq!(DEFAULT_BUILTIN_GROUP_NAME, "apcli");
}

#[test]
fn from_cli_config_with_name_accepts_custom_name() {
    let group = ApcliGroup::from_cli_config_with_name(
        Some(ApcliConfig {
            mode: ApcliMode::All,
            disable_env: true,
        }),
        /*registry_injected*/ false,
        Some("tools".to_string()),
    )
    .expect("custom name 'tools' should validate");
    assert_eq!(group.name(), "tools");
    assert_eq!(group.resolve_visibility(), "all");
}

#[test]
fn from_cli_config_with_name_default_falls_back_to_apcli() {
    let group = ApcliGroup::from_cli_config_with_name(
        Some(ApcliConfig {
            mode: ApcliMode::All,
            disable_env: true,
        }),
        false,
        None,
    )
    .expect("default name should validate");
    assert_eq!(group.name(), "apcli");
}

#[test]
fn validate_builtin_group_name_accepts_valid_names() {
    assert!(validate_builtin_group_name("apcli").is_ok());
    assert!(validate_builtin_group_name("admin").is_ok());
    assert!(validate_builtin_group_name("a").is_ok());
    assert!(validate_builtin_group_name("a1_b-c").is_ok());
}

#[test]
fn validate_builtin_group_name_rejects_invalid_names() {
    for bad in ["", "Apcli", "1abc", "-abc", "_abc", "a b", "ab!", "ABC"] {
        let result = validate_builtin_group_name(bad);
        assert!(
            matches!(result, Err(ApcliGroupError::InvalidName(_))),
            "name '{bad}' should be rejected, got {result:?}"
        );
    }
}

#[test]
fn from_cli_config_with_name_rejects_invalid_names() {
    let result = ApcliGroup::from_cli_config_with_name(None, false, Some("Bad-Name".to_string()));
    assert!(matches!(result, Err(ApcliGroupError::InvalidName(_))));
}

#[test]
fn try_from_yaml_with_name_rejects_invalid_names() {
    let result = ApcliGroup::try_from_yaml_with_name(None, false, Some("BAD".to_string()));
    assert!(matches!(result, Err(ApcliGroupError::InvalidName(_))));
}

#[test]
fn set_reserved_group_names_updates_live_set() {
    // Snapshot to restore at end so we don't poison sibling tests in this
    // process. NOTE: this test is deliberately last in the rename block —
    // tests in the same binary can run concurrently, but the assertions
    // below only depend on this thread's view of the OnceLock<RwLock>.
    let initial = effective_reserved_group_names();

    use std::collections::HashSet;
    let mut custom: HashSet<String> = HashSet::new();
    custom.insert("tools".to_string());
    set_reserved_group_names(custom);

    assert!(is_reserved_group_name("tools"));
    let live = effective_reserved_group_names();
    assert!(live.contains("tools"));
    assert!(!live.contains("apcli"));

    // Restore the prior snapshot so other tests see the default state.
    set_reserved_group_names(initial);
}
