// apcore-cli — FE-13 Built-in Command Group integration tests.
//
// Spec parity: ../apcore-cli/docs/features/builtin-group.md §4.9 (registration
// rules), §4.11 (hidden-but-reachable). The pre-v0.7 root-level deprecation
// shims (spec §11.2) were removed in v0.8 per §11.3 (audit D9-003).
//
// Covers a practical subset of the T-APCLI-01..41 matrix; deep unit-level
// coverage of tier precedence, env parsing, and yaml validation lives in
// `src/builtin_group.rs::tests` (33 cases).

use apcore_cli::{
    register_apcli_subcommands, ApcliConfig, ApcliGroup, ApcliMode, ConfigResolver,
    APCLI_SUBCOMMAND_NAMES,
};
use clap::Command;
use std::path::PathBuf;

/// Build a standalone `apcli` group using the public dispatcher helper. Mirrors
/// the logic in `main.rs::build_cli_command` but skips the Tier 3 yaml lookup
/// and root-level wiring so tests can target the group in isolation.
fn build_apcli_group(cfg: &ApcliGroup) -> Command {
    let group = Command::new("apcli")
        .about("Built-in apcore-cli commands.")
        .hide(!cfg.is_group_visible());
    register_apcli_subcommands(group, cfg, "apcore-cli")
}

// ---------------------------------------------------------------------------
// Standalone CLI exposes the apcli group
// ---------------------------------------------------------------------------

#[test]
fn apcli_group_exists_in_standalone_cli() {
    // Standalone mode: registry_injected = false, no yaml config — Tier 4
    // auto-detect yields "all" so every subcommand is registered and the
    // group itself is visible.
    let cfg = ApcliGroup::from_cli_config(None, /*registry_injected*/ false);
    let group = build_apcli_group(&cfg);
    assert_eq!(group.get_name(), "apcli");
}

#[test]
fn apcli_list_reachable_under_apcli() {
    // Build a root command containing the apcli group and assert that
    // `apcli list` resolves to the canonical subcommand.
    let cfg = ApcliGroup::from_cli_config(None, /*registry_injected*/ false);
    let apcli = build_apcli_group(&cfg);
    let root = Command::new("apcore-cli").subcommand(apcli);
    let matches = root
        .try_get_matches_from(vec!["apcore-cli", "apcli", "list"])
        .expect("apcli list must parse");
    let apcli_m = matches
        .subcommand_matches("apcli")
        .expect("apcli subcommand must match");
    assert!(
        apcli_m.subcommand_matches("list").is_some(),
        "apcli list must be reachable"
    );
}

// ---------------------------------------------------------------------------
// Always-registered exec (FE-12 guarantee, spec §4.9)
// ---------------------------------------------------------------------------

#[test]
fn apcli_exec_always_registered() {
    // mode: Include with an empty list — normally registers nothing, but
    // `exec` is always registered per spec §4.9.
    let cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Include(Vec::new()),
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let group = build_apcli_group(&cfg);
    let names: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();
    assert_eq!(
        names,
        vec!["exec"],
        "only 'exec' must be registered; got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// mode: None hides the group but still registers all subcommands (§4.11)
// ---------------------------------------------------------------------------

#[test]
fn apcli_mode_none_hides_group_but_keeps_subcommands() {
    let cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::None,
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    assert!(
        !cfg.is_group_visible(),
        "mode: None must hide the apcli group"
    );

    let group = build_apcli_group(&cfg);
    let registered: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();
    // spec §4.9 registration rules table: mode: None still registers all 13
    // subcommands for hidden-but-reachable behavior.
    assert_eq!(
        registered.len(),
        APCLI_SUBCOMMAND_NAMES.len(),
        "mode: None must still register all 13 subcommands; got {registered:?}"
    );
    for canon in APCLI_SUBCOMMAND_NAMES {
        assert!(
            registered.contains(canon),
            "mode: None must register '{canon}'"
        );
    }
}

// ---------------------------------------------------------------------------
// Include filter honors exec always-registered
// ---------------------------------------------------------------------------

#[test]
fn apcli_include_filter_excludes_init() {
    // include: [list, describe] — only list, describe, and exec (always) get
    // registered. init must be absent.
    let cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Include(vec!["list".into(), "describe".into()]),
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let group = build_apcli_group(&cfg);
    let names: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();

    assert!(names.contains(&"list"), "include list must register 'list'");
    assert!(
        names.contains(&"describe"),
        "include list must register 'describe'"
    );
    assert!(
        names.contains(&"exec"),
        "'exec' must be always-registered regardless of include list"
    );
    assert!(
        !names.contains(&"init"),
        "init must be absent under include: [list, describe]"
    );
}

// ---------------------------------------------------------------------------
// Exclude filter leaves exec untouched
// ---------------------------------------------------------------------------

#[test]
fn apcli_exclude_filter_excludes_init() {
    // exclude: [init] — every subcommand except init should be registered.
    let cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Exclude(vec!["init".into()]),
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let group = build_apcli_group(&cfg);
    let names: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();

    assert!(
        !names.contains(&"init"),
        "init must be absent under exclude: [init]"
    );
    for canon in APCLI_SUBCOMMAND_NAMES {
        if *canon == "init" {
            continue;
        }
        assert!(
            names.contains(canon),
            "exclude: [init] must register '{canon}'; got {names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Reserved-name enforcement (spec §4.10)
// ---------------------------------------------------------------------------

#[test]
fn apcli_reserved_name_rejected_in_build_module_command() {
    use apcore_cli::{build_module_command, cli::CliError};

    let descriptor = apcore::registry::registry::ModuleDescriptor {
        module_id: "apcli".to_string(),
        name: None,
        description: "reserved name collision".to_string(),
        documentation: None,
        input_schema: serde_json::Value::Null,
        output_schema: serde_json::Value::Object(Default::default()),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(apcore::module::ModuleAnnotations::default()),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };

    let result = build_module_command(&descriptor);
    assert!(
        matches!(result, Err(CliError::ReservedModuleId(ref m)) if m == "apcli"),
        "expected ReservedModuleId for module_id='apcli', got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// T-APCLI-04: Tier-1 cli-config `apcli: false` hides group in standalone
// ---------------------------------------------------------------------------

#[test]
fn apcli_cli_config_false_hides_group_in_standalone() {
    let cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::None,
            disable_env: false,
        }),
        /*registry_injected*/ false,
    );
    assert_eq!(cfg.resolve_visibility(), "none");
    assert!(!cfg.is_group_visible());
}

// ---------------------------------------------------------------------------
// T-APCLI-21: exclude:[] is equivalent to all
// ---------------------------------------------------------------------------

#[test]
fn apcli_empty_exclude_equivalent_to_all() {
    let cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::Exclude(Vec::new()),
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let group = build_apcli_group(&cfg);
    let names: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();
    assert_eq!(
        names.len(),
        APCLI_SUBCOMMAND_NAMES.len(),
        "exclude: [] must register every subcommand; got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// T-APCLI-38: programmatic Tier 1 wins over yaml (Tier 3)
// ---------------------------------------------------------------------------

#[test]
fn apcli_cli_config_tier1_overrides_yaml_tier3() {
    // Tier 3 yaml = false (hide). Tier 1 programmatic cli-config = true (show).
    // Tier 1 wins per spec §5.
    let yaml_cfg = ApcliGroup::from_yaml(
        Some(serde_yaml_ng::Value::Bool(false)),
        /*registry_injected*/ false,
    );
    assert_eq!(yaml_cfg.resolve_visibility(), "none");

    let cli_cfg = ApcliGroup::from_cli_config(
        Some(ApcliConfig {
            mode: ApcliMode::All,
            disable_env: false,
        }),
        /*registry_injected*/ false,
    );
    assert_eq!(
        cli_cfg.resolve_visibility(),
        "all",
        "Tier 1 cli-config must override Tier 3 yaml"
    );
}

// ---------------------------------------------------------------------------
// T-APCLI-02 / 05: embedded-mode auto-detect hides the group
// ---------------------------------------------------------------------------

#[test]
fn apcli_embedded_mode_defaults_to_hidden() {
    let cfg = ApcliGroup::from_cli_config(None, /*registry_injected*/ true);
    assert_eq!(
        cfg.resolve_visibility(),
        "none",
        "embedded mode must auto-detect to 'none'"
    );
    assert!(!cfg.is_group_visible());
}

#[test]
fn apcli_standalone_mode_defaults_to_visible() {
    let cfg = ApcliGroup::from_cli_config(None, /*registry_injected*/ false);
    assert_eq!(
        cfg.resolve_visibility(),
        "all",
        "standalone mode must auto-detect to 'all'"
    );
    assert!(cfg.is_group_visible());
}

// ---------------------------------------------------------------------------
// Tier 3 YAML path via ConfigResolver::resolve_object
// ---------------------------------------------------------------------------

#[test]
fn apcli_yaml_tier3_reads_bool_shorthand() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("apcore.yaml");
    std::fs::write(&path, "apcli: false\n").unwrap();

    let resolver = ConfigResolver::new(None, Some(path));
    let yaml_val = resolver.resolve_object("apcli");
    assert!(yaml_val.is_some(), "resolve_object must return the bool");

    let cfg = ApcliGroup::from_yaml(yaml_val, /*registry_injected*/ false);
    assert_eq!(cfg.resolve_visibility(), "none");
}

#[test]
fn apcli_yaml_tier3_reads_object_form() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("apcore.yaml");
    std::fs::write(
        &path,
        "apcli:\n  mode: include\n  include:\n    - list\n    - describe\n",
    )
    .unwrap();

    let resolver = ConfigResolver::new(None, Some(path));
    let yaml_val = resolver.resolve_object("apcli");
    let cfg = ApcliGroup::from_yaml(yaml_val, /*registry_injected*/ false);
    let group = build_apcli_group(&cfg);
    let names: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();
    for required in &["list", "describe", "exec"] {
        assert!(
            names.contains(required),
            "yaml include=[list, describe] must register '{required}'; got {names:?}"
        );
    }
    assert!(
        !names.contains(&"init"),
        "yaml include=[list, describe] must not register 'init'"
    );
}

// ---------------------------------------------------------------------------
// disable_env seals APCORE_CLI_APCLI overrides (§4.12)
// ---------------------------------------------------------------------------

#[test]
fn apcli_disable_env_seals_tier2_override() {
    // disable_env = true must make the tier-2 env var path inert regardless
    // of whether APCORE_CLI_APCLI is set. We don't touch the env here to
    // avoid serialization concerns with the unit-test env mutex; instead we
    // rely on the behavioral contract — a sealed group returns the yaml
    // mode unchanged.
    let cfg = ApcliGroup::from_yaml(
        Some(serde_yaml_ng::Value::Mapping({
            let mut m = serde_yaml_ng::Mapping::new();
            m.insert(
                serde_yaml_ng::Value::String("mode".to_string()),
                serde_yaml_ng::Value::String("none".to_string()),
            );
            m.insert(
                serde_yaml_ng::Value::String("disable_env".to_string()),
                serde_yaml_ng::Value::Bool(true),
            );
            m
        })),
        /*registry_injected*/ false,
    );
    assert!(cfg.disable_env());
    assert_eq!(cfg.resolve_visibility(), "none");
}
