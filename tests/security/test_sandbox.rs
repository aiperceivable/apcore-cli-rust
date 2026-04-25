// apcore-cli — Integration tests for Sandbox execution.
// Protocol spec: SEC-04
//
// Audit A-003 (v0.6.x): Sandbox.execute() now takes an `&apcore::Executor`
// parameter for the disabled-path passthrough. The disabled branch delegates
// to executor.call() instead of returning a stub error.

use apcore::{Config, Executor, Registry};
use apcore_cli::security::sandbox::{ModuleExecutionError, Sandbox};
use serde_json::json;

/// Build a minimal apcore::Executor for tests. The executor wraps an empty
/// Registry — module lookups will fail, which is acceptable for testing the
/// passthrough plumbing (we only verify the executor *receives* the call).
fn make_test_executor() -> Executor {
    Executor::new(Registry::new(), Config::default())
}

#[tokio::test]
async fn test_sandbox_disabled_passes_through_to_executor() {
    // Sandbox::new(false, 0) must NOT spawn a subprocess. The disabled
    // branch delegates to the injected executor.call() and preserves the
    // apcore ModuleError variant so callers can map to protocol exit codes.
    // The empty test registry yields ModuleError(MODULE_NOT_FOUND).
    let sandbox = Sandbox::new(false, 0);
    assert!(!sandbox.is_enabled(), "sandbox must report disabled");
    let executor = make_test_executor();
    let result = sandbox
        .execute("math.add", json!({"a": 1, "b": 2}), &executor)
        .await;
    match &result {
        Err(ModuleExecutionError::SpawnFailed(msg)) if msg.contains("not wired") => {
            panic!("disabled sandbox must passthrough to executor, not return 'not wired'");
        }
        Err(ModuleExecutionError::ModuleError(_)) => {
            // Expected: executor returned MODULE_NOT_FOUND (empty registry).
            // Variant is preserved so cli.rs can map the protocol exit code.
        }
        Err(other) => panic!(
            "disabled sandbox must surface ModuleError variant for executor \
             failures so exit-code mapping stays consistent with direct exec; \
             got: {other:?}"
        ),
        Ok(_) => {
            // If the executor somehow succeeded (unlikely with empty registry),
            // the passthrough still worked.
        }
    }
}

#[tokio::test]
#[ignore = "requires the compiled apcore-cli binary to be present at current_exe(); \
            run manually after `cargo build` with `cargo test -- --ignored`"]
async fn test_sandbox_enabled_spawns_subprocess() {
    // Sandbox::new(true, 5) routes execution through a subprocess (5 second timeout).
    // The enabled branch ignores the executor argument — the subprocess
    // loads its own apcore environment from inherited APCORE_* env vars.
    let sandbox = Sandbox::new(true, 5);
    assert!(sandbox.is_enabled(), "sandbox must report enabled");
    let executor = make_test_executor();
    let result = sandbox
        .execute("math.add", json!({"a": 1, "b": 2}), &executor)
        .await;
    // Subprocess will likely fail without --internal-sandbox-runner wiring,
    // but it must at least attempt to spawn (not return the executor error).
    match &result {
        Err(ModuleExecutionError::SpawnFailed(msg)) if msg.contains("not found") => {
            panic!(
                "sandbox enabled path must not delegate to executor, \
                 must spawn subprocess instead. Got: {msg}"
            );
        }
        _ => {} // Ok, Timeout, NonZeroExit, SpawnFailed(spawn) all acceptable
    }
}

#[tokio::test]
async fn test_sandbox_enabled_timeout_returns_error() {
    // A very short timeout with enabled=true must yield Timeout or SpawnFailed.
    let sandbox = Sandbox::new(true, 1); // 1 second timeout — short enough to trigger quickly
    let executor = make_test_executor();
    let result = sandbox.execute("slow.module", json!({}), &executor).await;
    assert!(
        result.is_err(),
        "sandbox with 1s timeout must return an error"
    );
}

#[tokio::test]
async fn test_nonzero_exit_carries_stderr() {
    // Construct a NonZeroExit manually to verify the stderr field is
    // preserved in the Display output. This closes the review finding that
    // the sandbox discarded captured stderr before returning the error.
    let err = ModuleExecutionError::NonZeroExit {
        module_id: "some.module".to_string(),
        exit_code: 2,
        stderr: "simulated panic: invalid state".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("simulated panic: invalid state"),
        "NonZeroExit Display must include captured stderr, got: {msg}"
    );
    assert!(msg.contains("exited with code 2"));
}

#[tokio::test]
#[ignore = "requires the compiled apcore-cli binary to be present at current_exe(); \
            run manually after `cargo build --bin apcore-cli` with `cargo test -- --ignored`"]
async fn test_sandbox_nonzero_exit_returns_error() {
    // A subprocess exiting non-zero must yield NonZeroExit.
    let sandbox = Sandbox::new(true, 5); // 5 second timeout
    let executor = make_test_executor();
    let result = sandbox
        .execute("__nonexistent_module__", json!({}), &executor)
        .await;
    assert!(result.is_err(), "unknown module must result in an error");
}
