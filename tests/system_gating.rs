// apcore-cli — D10-004 system-module gating.
//
// Spec parity: ../apcore-cli/docs/features/usability-enhancements.md
// §register_system_commands — "either all six subcommands appear (executor
// returns success on `system.health.summary` probe) or none do. Partial
// registration is a parity bug." Mirrors apcore-cli-python factory.py
// `system_available` gating and the apcore-cli-typescript registration probe.
//
// This test lives in its own test binary so that toggling the process-global
// `set_system_modules_available(false)` cannot race the default-true
// expectations in `apcli_integration.rs` (each tests/*.rs is a separate process).

use apcore_cli::{register_apcli_subcommands, set_system_modules_available, ApcliGroup};
use clap::Command;

const SYSTEM_COMMANDS: [&str; 6] = ["health", "usage", "enable", "disable", "reload", "config"];

fn registered_names(cfg: &ApcliGroup) -> Vec<String> {
    let group = Command::new("apcli")
        .about("Built-in apcore-cli commands.")
        .hide(!cfg.is_group_visible());
    register_apcli_subcommands(group, cfg, "apcore-cli")
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect()
}

#[test]
fn system_subcommands_gated_by_availability() {
    let cfg = ApcliGroup::from_cli_config(None, /*registry_injected*/ false);

    // Unavailable: the six system subcommands must NOT be registered.
    set_system_modules_available(false);
    let names = registered_names(&cfg);
    for sys in SYSTEM_COMMANDS {
        assert!(
            !names.contains(&sys.to_string()),
            "{sys} must be hidden when system modules are unavailable; got {names:?}"
        );
    }
    // Non-system subcommands remain reachable.
    assert!(
        names.contains(&"list".to_string()),
        "list must remain; got {names:?}"
    );
    assert!(
        names.contains(&"exec".to_string()),
        "exec must remain; got {names:?}"
    );

    // Available (default): all six system subcommands are registered again.
    set_system_modules_available(true);
    let names = registered_names(&cfg);
    for sys in SYSTEM_COMMANDS {
        assert!(
            names.contains(&sys.to_string()),
            "{sys} must appear when system modules are available; got {names:?}"
        );
    }
}
