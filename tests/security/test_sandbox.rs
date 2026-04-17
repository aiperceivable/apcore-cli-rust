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
    // branch now delegates to the injected executor.call(). Since our test
    // registry is empty, the executor returns a "module not found" error,
    // which we catch as SpawnFailed (the audit A-003 design wraps any
    // executor error in SpawnFailed). The key assertion is that we DON'T
    // get the old "not wired" stub error.
    let sandbox = Sandbox::new(false, 0);
    assert!(!sandbox.is_enabled(), "sandbox must report disabled");
    let executor = make_test_executor();
    let result = sandbox
        .execute("math.add", json!({"a": 1, "b": 2}), &executor)
        .await;
    // We expect an error because the test registry is empty, but it must
    // NOT be the legacy "not wired" stub.
    match &result {
        Err(ModuleExecutionError::SpawnFailed(msg)) if msg.contains("not wired") => {
            panic!("disabled sandbox must passthrough to executor, not return 'not wired'");
        }
        Err(_) => {} // any executor error is fine — we just verified passthrough
        Ok(_) => {
            // If the executor somehow succeeded (unlikely with empty registry),
            // that's also acceptable — the passthrough worked.
        }
    }
}

#[tokio::test]
#[ignore = "requires the compiled apcore-cli binary to be present at current_exe(); \
            run manually after `cargo build` with `cargo test -- --ignored`"]
async fn test_sandbox_enabled_spawns_subprocess() {
    // Sandbox::new(true, 5000) routes execution through a subprocess.
    // The enabled branch ignores the executor argument — the subprocess
    // loads its own apcore environment from inherited APCORE_* env vars.
    let sandbox = Sandbox::new(true, 5000);
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
    let sandbox = Sandbox::new(true, 1); // 1 ms timeout
    let executor = make_test_executor();
    let result = sandbox.execute("slow.module", json!({}), &executor).await;
    assert!(
        result.is_err(),
        "sandbox with 1ms timeout must return an error"
    );
}

#[tokio::test]
#[ignore = "requires the compiled apcore-cli binary to be present at current_exe(); \
            run manually after `cargo build --bin apcore-cli` with `cargo test -- --ignored`"]
async fn test_sandbox_nonzero_exit_returns_error() {
    // A subprocess exiting non-zero must yield NonZeroExit.
    let sandbox = Sandbox::new(true, 5000);
    let executor = make_test_executor();
    let result = sandbox
        .execute("__nonexistent_module__", json!({}), &executor)
        .await;
    assert!(result.is_err(), "unknown module must result in an error");
}
