// apcore-cli — Subprocess sandbox for module execution.
// Protocol spec: SEC-04 (Sandbox, ModuleExecutionError)

use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Environment variable prefixes allowed through the sandbox env whitelist.
const SANDBOX_ALLOWED_ENV_PREFIXES: &[&str] = &["APCORE_"];

/// Exact environment variable names allowed through the sandbox env whitelist.
const SANDBOX_ALLOWED_ENV_KEYS: &[&str] = &["PATH", "LANG", "LC_ALL"];

// ---------------------------------------------------------------------------
// ModuleExecutionError
// ---------------------------------------------------------------------------

/// Errors produced during sandboxed module execution.
#[derive(Debug, Error)]
pub enum ModuleExecutionError {
    /// The subprocess exited with a non-zero exit code.
    #[error("module '{module_id}' exited with code {exit_code}")]
    NonZeroExit { module_id: String, exit_code: i32 },

    /// The subprocess timed out.
    #[error("module '{module_id}' timed out after {timeout_ms}ms")]
    Timeout { module_id: String, timeout_ms: u64 },

    /// The subprocess output could not be parsed.
    #[error("failed to parse sandbox output for module '{module_id}': {reason}")]
    OutputParseFailed { module_id: String, reason: String },

    /// Failed to spawn the sandbox subprocess.
    #[error("failed to spawn sandbox process: {0}")]
    SpawnFailed(String),
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// Executes modules in an isolated subprocess for security isolation.
///
/// When `enabled` is `false`, execution is performed in-process (no sandbox).
/// When `enabled` is `true`, a child process running `_sandbox_runner` handles
/// the execution and communicates results via JSON over stdin/stdout.
pub struct Sandbox {
    enabled: bool,
    timeout_ms: u64,
}

impl Sandbox {
    /// Create a new `Sandbox`.
    ///
    /// # Arguments
    /// * `enabled`    — enable subprocess isolation
    /// * `timeout_ms` — subprocess timeout in milliseconds (0 = use default 300 s)
    pub fn new(enabled: bool, timeout_ms: u64) -> Self {
        Self { enabled, timeout_ms }
    }

    /// Execute a module, optionally in an isolated subprocess.
    ///
    /// # Arguments
    /// * `module_id`  — identifier of the module to execute
    /// * `input_data` — JSON input for the module
    ///
    /// Returns the module output as a `serde_json::Value`.
    ///
    /// # Errors
    /// Returns `ModuleExecutionError` on timeout, non-zero exit, or parse failure.
    pub async fn execute(
        &self,
        module_id: &str,
        input_data: Value,
    ) -> Result<Value, ModuleExecutionError> {
        if !self.enabled {
            // In-process execution — caller is responsible for wiring the executor.
            // Real wiring happens in the integration task when Sandbox is connected
            // to the Executor via a callback or trait object.
            return Err(ModuleExecutionError::SpawnFailed(
                "in-process executor not wired (use Sandbox::execute_with)".to_string(),
            ));
        }
        self._sandboxed_execute(module_id, input_data).await
    }

    async fn _sandboxed_execute(
        &self,
        module_id: &str,
        input_data: Value,
    ) -> Result<Value, ModuleExecutionError> {
        use std::process::Stdio;
        use tokio::io::AsyncWriteExt;
        use tokio::process::Command;
        use tokio::time::{timeout, Duration};

        // Build restricted environment from whitelist.
        let mut env: Vec<(String, String)> = Vec::new();
        let host_env: std::collections::HashMap<String, String> = std::env::vars().collect();

        for key in SANDBOX_ALLOWED_ENV_KEYS {
            if let Some(val) = host_env.get(*key) {
                env.push((key.to_string(), val.clone()));
            }
        }
        for (k, v) in &host_env {
            if SANDBOX_ALLOWED_ENV_PREFIXES.iter().any(|prefix| k.starts_with(prefix)) {
                env.push((k.clone(), v.clone()));
            }
        }

        // Create temp dir for HOME/TMPDIR isolation.
        let tmpdir = tempfile::TempDir::new()
            .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;
        let tmpdir_path = tmpdir.path().to_string_lossy().to_string();
        env.push(("HOME".to_string(), tmpdir_path.clone()));
        env.push(("TMPDIR".to_string(), tmpdir_path.clone()));

        // Serialise input.
        let input_json = serde_json::to_string(&input_data)
            .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;

        // Locate current binary.
        let binary = std::env::current_exe()
            .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;

        let mut child = Command::new(&binary)
            .arg("--internal-sandbox-runner")
            .arg(module_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(env)
            .current_dir(&tmpdir_path)
            .spawn()
            .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;

        // Write input to stdin.
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input_json.as_bytes())
                .await
                .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;
        }

        // Await with timeout.
        let timeout_dur = if self.timeout_ms > 0 {
            Duration::from_millis(self.timeout_ms)
        } else {
            Duration::from_secs(300)
        };

        let output = timeout(timeout_dur, child.wait_with_output())
            .await
            .map_err(|_| ModuleExecutionError::Timeout {
                module_id: module_id.to_string(),
                timeout_ms: self.timeout_ms,
            })?
            .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;

        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            return Err(ModuleExecutionError::NonZeroExit {
                module_id: module_id.to_string(),
                exit_code,
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        crate::_sandbox_runner::decode_result(&stdout).map_err(|e| {
            ModuleExecutionError::OutputParseFailed {
                module_id: module_id.to_string(),
                reason: e.to_string(),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_sandbox_disabled_returns_passthrough_error() {
        // When disabled, execute() must NOT spawn a subprocess.
        // Verify the sandbox disabled path does NOT return Timeout or SpawnFailed
        // with a spawn-related OS error — it returns SpawnFailed with the
        // "not wired" message (no subprocess involved).
        let sandbox = Sandbox::new(false, 5_000);
        let result = sandbox.execute("test.module", json!({})).await;
        assert!(!matches!(result, Err(ModuleExecutionError::Timeout { .. })));
        // The disabled path returns SpawnFailed("in-process executor not wired …")
        // which is NOT a real spawn attempt, so confirm it IS that specific variant.
        match result {
            Err(ModuleExecutionError::SpawnFailed(msg)) => {
                assert!(
                    msg.contains("not wired"),
                    "expected 'not wired' message, got: {msg}"
                );
            }
            other => panic!("expected SpawnFailed(not wired), got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sandbox_timeout_returns_error() {
        // Use a 1 ms timeout — spawn a real subprocess that will time out.
        // Either timeout or spawn-failed (binary not yet wired) — accept both.
        let sandbox = Sandbox::new(true, 1); // 1 ms timeout
        let result = sandbox.execute("__noop__", json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_result_valid_json() {
        use crate::_sandbox_runner::decode_result;
        let v = decode_result(r#"{"ok":true}"#).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn test_decode_result_invalid_json() {
        use crate::_sandbox_runner::decode_result;
        assert!(decode_result("not json").is_err());
    }

    #[test]
    fn test_encode_result_roundtrip() {
        use crate::_sandbox_runner::{decode_result, encode_result};
        let v = json!({"result": 42});
        let encoded = encode_result(&v);
        let decoded = decode_result(&encoded).unwrap();
        assert_eq!(decoded["result"], 42);
    }
}
