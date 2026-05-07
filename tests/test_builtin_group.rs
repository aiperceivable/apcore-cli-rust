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
    let yaml: serde_yaml::Value = serde_yaml::from_str("mode: not-a-mode").unwrap();
    let result = ApcliGroup::try_from_yaml(Some(yaml), false);
    assert!(
        matches!(result, Err(ApcliGroupError::ModeInvalid(_))),
        "unknown apcli.mode must be ModeInvalid, got {result:?}"
    );
}

#[test]
fn try_from_yaml_rejects_non_string_mode() {
    let yaml: serde_yaml::Value = serde_yaml::from_str("mode: 42").unwrap();
    let result = ApcliGroup::try_from_yaml(Some(yaml), false);
    assert!(
        matches!(result, Err(ApcliGroupError::ModeNotString(_))),
        "non-string apcli.mode must be ModeNotString, got {result:?}"
    );
}
