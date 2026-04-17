//! Smoke tests for the `validate` module (FE-11 --dry-run / preflight).
//!
//! TODO (T-001): expand with full preflight / --dry-run coverage. Real
//! verification requires a live apcore::Executor wired to a Registry.

#[test]
fn validate_command_constructible() {
    let cmd = apcore_cli::validate::validate_command();
    assert_eq!(cmd.get_name(), "validate");
}
