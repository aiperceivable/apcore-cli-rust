//! Smoke tests for the `strategy` module (FE-11 describe-pipeline + --strategy).
//!
//! TODO (T-001): expand with full describe-pipeline / strategy-selection coverage.
//! Real verification requires a live apcore::Executor with strategy support —
//! see code-forge:build for the dedicated test-writing pass.

#[test]
fn strategy_module_describe_pipeline_command_constructible() {
    // describe_pipeline_command() is a clap::Command builder. Verify the
    // constructor returns a valid command without panicking. The function
    // is not at the crate root (per audit D9-005 lib.rs trim), so import
    // via the full module path.
    let cmd = apcore_cli::strategy::describe_pipeline_command();
    assert_eq!(cmd.get_name(), "describe-pipeline");
}
