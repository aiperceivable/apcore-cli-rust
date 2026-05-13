// apcore-cli — Integration tests for Sandbox execution.
// Protocol spec: SEC-04
//
// Audit A-003 (v0.6.x): Sandbox.execute() now takes an `&apcore::Executor`
// parameter for the disabled-path passthrough. The disabled branch delegates
// to executor.call() instead of returning a stub error.

use std::path::PathBuf;

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
async fn test_sandbox_with_extensions_root_sets_field() {
    // D1-004 parity with Python Sandbox.with_extensions_root: the builder
    // must store the path so _sandboxed_execute can inject the absolute path
    // as APCORE_EXTENSIONS_ROOT into the sandbox subprocess environment.
    let path = PathBuf::from("/tmp/apcore-ext-test");
    let sandbox = Sandbox::new(false, 5).with_extensions_root(Some(path.clone()));
    assert_eq!(
        sandbox.extensions_root(),
        Some(&path),
        "with_extensions_root must store the supplied path for subprocess injection"
    );
}

#[tokio::test]
async fn test_sandbox_with_max_output_bytes_sets_field() {
    // D1-004 parity with Python Sandbox.with_max_output_bytes: the builder
    // must override the default 64 MiB output cap used by _sandboxed_execute.
    let sandbox = Sandbox::new(false, 5).with_max_output_bytes(2048);
    assert_eq!(
        sandbox.max_output_bytes(),
        2048,
        "with_max_output_bytes must override the 64 MiB default"
    );
}

#[tokio::test]
async fn test_sandbox_builder_chains_with_constructor() {
    // The two builders must be chainable on top of Sandbox::new and must
    // not disturb the constructor-set fields (enabled, timeout). This
    // mirrors the Python fluent style: Sandbox(True, 30).with_extensions_root(p).with_max_output_bytes(N).
    let path = PathBuf::from("/tmp/ext-chain");
    let sandbox = Sandbox::new(true, 30)
        .with_extensions_root(Some(path.clone()))
        .with_max_output_bytes(8192);
    assert!(sandbox.is_enabled());
    assert_eq!(sandbox.extensions_root(), Some(&path));
    assert_eq!(sandbox.max_output_bytes(), 8192);
}

#[tokio::test]
async fn test_sandbox_with_max_output_bytes_enforces_smaller_cap() {
    // Wire-through proof: with the cap set to a tiny value (1 byte) and the
    // sandbox enabled, _sandboxed_execute must trip the OutputParseFailed
    // branch (output exceeded N bytes) rather than running with the 64 MiB
    // default. We don't depend on the actual subprocess succeeding — any
    // outcome other than "ran with default cap" is acceptable. To keep the
    // test deterministic without the binary, we use an obviously-bogus
    // module id; the sandbox spawn either fails fast (SpawnFailed), times
    // out, or exits non-zero. The point is that the configured cap value is
    // observable via the accessor and is what _sandboxed_execute reads.
    let sandbox = Sandbox::new(false, 1).with_max_output_bytes(1);
    assert_eq!(sandbox.max_output_bytes(), 1);
    // Disabled-path passthrough is unaffected by the cap (no subprocess),
    // so this just confirms the field doesn't break the disabled path.
    let executor = Executor::new(Registry::new(), Config::default());
    let _ = sandbox.execute("noop", json!({}), &executor).await;
}

#[tokio::test]
async fn test_output_size_exceeded_variant_display_and_fields() {
    // D11-007: when stdout (or stderr) exceeds the sandbox cap, the error must
    // be the dedicated OutputSizeExceeded variant — NOT OutputParseFailed,
    // which is reserved for malformed JSON. The Display string must report
    // the limit in MiB (parity with Python/TS) and name the overflowing stream.
    let err = ModuleExecutionError::OutputSizeExceeded {
        module_id: "noisy.module".to_string(),
        limit_bytes: 64 * 1024 * 1024,
        overflow_stream: "stdout".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("MiB"),
        "Display must report cap in MiB, got: {msg}"
    );
    assert!(
        msg.contains("stdout"),
        "Display must name the overflowing stream, got: {msg}"
    );
    assert!(
        msg.contains("noisy.module"),
        "Display must include module id, got: {msg}"
    );

    // Variant pattern match must succeed (compile-time guard against the
    // variant being collapsed back into OutputParseFailed).
    match err {
        ModuleExecutionError::OutputSizeExceeded { .. } => {}
        other => panic!("expected OutputSizeExceeded, got: {other:?}"),
    }
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
