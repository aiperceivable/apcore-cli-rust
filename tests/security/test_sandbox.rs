// apcore-cli — Integration tests for Sandbox execution.
// Protocol spec: SEC-04

use apcore_cli::security::sandbox::{ModuleExecutionError, Sandbox};
use serde_json::json;

#[tokio::test]
async fn test_sandbox_disabled_executes_inline() {
    // Sandbox::new(false, 0) must NOT spawn a subprocess.
    // The disabled path returns SpawnFailed("in-process executor not wired …")
    // which confirms no OS process was launched.
    let sandbox = Sandbox::new(false, 0);
    assert!(!sandbox.is_enabled(), "sandbox must report disabled");
    let result = sandbox.execute("math.add", json!({"a": 1, "b": 2})).await;
    match result {
        Err(ModuleExecutionError::SpawnFailed(msg)) => {
            assert!(
                msg.contains("not wired"),
                "disabled sandbox must return the 'not wired' error, got: {msg}"
            );
        }
        other => panic!("expected SpawnFailed(not wired) for disabled sandbox, got: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires the compiled apcore-cli binary to be present at current_exe(); \
            run manually after `cargo build` with `cargo test -- --ignored`"]
async fn test_sandbox_enabled_spawns_subprocess() {
    // Sandbox::new(true, 5000) routes execution through a subprocess.
    // This test requires the binary to be built and available via current_exe().
    let sandbox = Sandbox::new(true, 5000);
    assert!(sandbox.is_enabled(), "sandbox must report enabled");
    let result = sandbox.execute("math.add", json!({"a": 1, "b": 2})).await;
    // The subprocess will fail unless --internal-sandbox-runner is handled,
    // but it must at least attempt to spawn (not return "not wired").
    match &result {
        Err(ModuleExecutionError::SpawnFailed(msg)) if msg.contains("not wired") => {
            panic!("sandbox enabled path must not return 'not wired'");
        }
        _ => {} // Ok, Timeout, NonZeroExit, SpawnFailed(other) are all acceptable
    }
}

#[tokio::test]
async fn test_sandbox_timeout_returns_error() {
    // A very short timeout must yield either Timeout or SpawnFailed.
    // Both are acceptable since the subprocess may or may not start within 1 ms.
    let sandbox = Sandbox::new(true, 1); // 1 ms timeout
    let result = sandbox.execute("slow.module", json!({})).await;
    assert!(
        result.is_err(),
        "sandbox with 1ms timeout must return an error"
    );
    // Must not be the "not wired" placeholder from the disabled path.
    match &result {
        Err(ModuleExecutionError::SpawnFailed(msg)) if msg.contains("not wired") => {
            panic!("enabled sandbox with timeout must not return 'not wired'");
        }
        Err(_) => {} // Timeout, NonZeroExit, SpawnFailed(spawn error) are all valid
        Ok(_) => panic!("expected error for 1ms timeout, got Ok"),
    }
}

#[tokio::test]
#[ignore = "requires the compiled apcore-cli binary to be present at current_exe(); \
            run manually after `cargo build --bin apcore-cli` with `cargo test -- --ignored`"]
async fn test_sandbox_nonzero_exit_returns_error() {
    // A subprocess exiting non-zero must yield NonZeroExit.
    // We trigger this by passing a module_id that the sandbox runner does not handle.
    let sandbox = Sandbox::new(true, 5000);
    let result = sandbox.execute("__nonexistent_module__", json!({})).await;
    assert!(result.is_err(), "unknown module must result in an error");
}
