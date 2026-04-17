//! Smoke tests for the `system_cmd` module (FE-11 system management).
//!
//! TODO (T-001): expand with full health/usage/enable/disable/reload/config
//! coverage. Real verification requires a live apcore::Executor with system
//! modules registered.

use apcore_cli::SYSTEM_COMMANDS;

#[test]
fn system_commands_constant_is_nonempty() {
    assert!(!SYSTEM_COMMANDS.is_empty());
}

#[test]
fn system_commands_contains_health() {
    assert!(SYSTEM_COMMANDS.contains(&"health"));
}

#[test]
fn system_commands_contains_config() {
    assert!(SYSTEM_COMMANDS.contains(&"config"));
}
