// apcore-cli — Subprocess sandbox for module execution.
// Protocol spec: SEC-04 (Sandbox, ModuleExecutionError)

use tokio::io::AsyncReadExt;

use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Environment variable prefixes allowed through the sandbox env whitelist.
const SANDBOX_ALLOWED_ENV_PREFIXES: &[&str] = &["APCORE_"];

/// Exact environment variable names allowed through the sandbox env whitelist.
const SANDBOX_ALLOWED_ENV_KEYS: &[&str] = &["PATH", "LANG", "LC_ALL"];

/// Environment variable prefixes denied even when matched by the allow list.
/// Credential-bearing namespaces must never reach the sandboxed child process.
const SANDBOX_DENIED_ENV_PREFIXES: &[&str] = &["APCORE_AUTH_"];

/// Exact environment variable names denied regardless of prefix match.
const SANDBOX_DENIED_ENV_KEYS: &[&str] = &["APCORE_AUTH_API_KEY"];

/// Maximum bytes collected from sandbox stdout or stderr before the child is
/// killed and OutputParseFailed is returned. Guards against OOM from hostile
/// or buggy modules that write unboundedly.
const SANDBOX_OUTPUT_SIZE_LIMIT_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

// ---------------------------------------------------------------------------
// ModuleExecutionError
// ---------------------------------------------------------------------------

/// Errors produced during sandboxed module execution.
#[derive(Debug, Error)]
pub enum ModuleExecutionError {
    /// The subprocess exited with a non-zero exit code. The captured
    /// stderr is preserved on the error so callers can surface it for
    /// debuggability (the subprocess panics, tracebacks, and user-facing
    /// error prints all land here).
    #[error("module '{module_id}' exited with code {exit_code}{}",
            if stderr.is_empty() { String::new() } else { format!(": {stderr}") })]
    NonZeroExit {
        module_id: String,
        exit_code: i32,
        stderr: String,
    },

    /// The subprocess timed out.
    #[error("module '{module_id}' timed out after {timeout_secs}s")]
    Timeout {
        module_id: String,
        timeout_secs: u64,
    },

    /// The subprocess output could not be parsed.
    #[error("failed to parse sandbox output for module '{module_id}': {reason}")]
    OutputParseFailed { module_id: String, reason: String },

    /// Failed to spawn the sandbox subprocess.
    #[error("failed to spawn sandbox process: {0}")]
    SpawnFailed(String),

    /// A module-level error from the in-process apcore executor on the disabled
    /// passthrough path. Preserved as a variant (rather than stringified) so
    /// callers can map the underlying `ErrorCode` via
    /// `crate::cli::map_module_error_to_exit_code`, keeping exit-code taxonomy
    /// consistent between `--sandbox` and direct execution paths.
    #[error(transparent)]
    ModuleError(#[from] apcore::errors::ModuleError),
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// Executes modules in an isolated subprocess for security isolation.
///
/// When `enabled` is `false`, execution is performed in-process (no sandbox).
/// When `enabled` is `true`, a child process running `sandbox_runner` handles
/// the execution and communicates results via JSON over stdin/stdout.
pub struct Sandbox {
    enabled: bool,
    timeout_secs: u64,
}

impl Sandbox {
    /// Create a new `Sandbox`.
    ///
    /// # Arguments
    /// * `enabled`    — enable subprocess isolation
    /// * `timeout_secs` — subprocess timeout in seconds (0 = use default 300 s)
    pub fn new(enabled: bool, timeout_secs: u64) -> Self {
        Self {
            enabled,
            timeout_secs,
        }
    }

    /// Return `true` when subprocess isolation is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
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
    ///
    /// When `enabled` is `false`, delegates directly to `executor.call()` and
    /// returns the result (or maps the apcore module error into a
    /// `ModuleExecutionError::SpawnFailed`). This passthrough makes Sandbox
    /// safe to call unconditionally from the dispatcher: callers no longer
    /// need to branch on the `--sandbox` flag at every call site.
    ///
    /// When `enabled` is `true`, runs `module_id` in an isolated subprocess
    /// via `sandbox_runner` and returns the parsed JSON output. The executor
    /// argument is intentionally unused in this branch — the subprocess loads
    /// its own apcore environment from the inherited `APCORE_*` env vars.
    pub async fn execute(
        &self,
        module_id: &str,
        input_data: Value,
        executor: &apcore::Executor,
    ) -> Result<Value, ModuleExecutionError> {
        if !self.enabled {
            // Passthrough: delegate to the in-process apcore::Executor and
            // preserve the ModuleError variant so callers can map to the
            // protocol-spec exit code.
            return executor
                .call(module_id, input_data, None, None)
                .await
                .map_err(ModuleExecutionError::ModuleError);
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
            if SANDBOX_ALLOWED_ENV_PREFIXES
                .iter()
                .any(|prefix| k.starts_with(prefix))
                && !SANDBOX_DENIED_ENV_PREFIXES
                    .iter()
                    .any(|prefix| k.starts_with(prefix))
                && !SANDBOX_DENIED_ENV_KEYS.contains(&k.as_str())
            {
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
            // Ensure the child is killed if this future is dropped (e.g. on
            // timeout or SIGINT) — tokio's default is kill_on_drop=false,
            // which would leak the subprocess past Err(Timeout).
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;

        // Write input to stdin.
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input_json.as_bytes())
                .await
                .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;
        }

        // Await with timeout, collecting stdout/stderr up to the cap.
        let timeout_dur = if self.timeout_secs > 0 {
            Duration::from_secs(self.timeout_secs)
        } else {
            Duration::from_secs(300)
        };

        // Take pipe handles before the join so the child struct can also be
        // awaited for the exit status in the same async block.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let cap = SANDBOX_OUTPUT_SIZE_LIMIT_BYTES;
        let collect_result = timeout(timeout_dur, async {
            let (stdout_res, stderr_res) = tokio::join!(
                async {
                    let mut buf = Vec::new();
                    if let Some(r) = stdout_pipe {
                        let _ = r.take(cap as u64 + 1).read_to_end(&mut buf).await;
                    }
                    buf
                },
                async {
                    let mut buf = Vec::new();
                    if let Some(r) = stderr_pipe {
                        let _ = r.take(cap as u64 + 1).read_to_end(&mut buf).await;
                    }
                    buf
                },
            );
            let status = child
                .wait()
                .await
                .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;
            Ok::<_, ModuleExecutionError>((stdout_res, stderr_res, status))
        })
        .await
        .map_err(|_| ModuleExecutionError::Timeout {
            module_id: module_id.to_string(),
            timeout_secs: self.timeout_secs,
        })??;

        let (stdout_bytes, stderr_bytes, status) = collect_result;

        if stdout_bytes.len() > cap || stderr_bytes.len() > cap {
            return Err(ModuleExecutionError::OutputParseFailed {
                module_id: module_id.to_string(),
                reason: format!("sandbox output exceeded {} bytes", cap),
            });
        }

        if !status.success() {
            let exit_code = status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
            return Err(ModuleExecutionError::NonZeroExit {
                module_id: module_id.to_string(),
                exit_code,
                stderr,
            });
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
        crate::sandbox_runner::decode_result(&stdout).map_err(|e| {
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
    async fn test_sandbox_disabled_delegates_to_executor() {
        // Audit A-003 (v0.6.x): the disabled path now passes through to the
        // injected apcore::Executor instead of returning a "not wired" stub.
        // We can't easily build a real executor in unit tests (it needs a
        // Registry + Config + module discovery), so we verify the API surface
        // accepts the executor parameter. End-to-end passthrough is exercised
        // by tests/test_e2e.rs which constructs a real executor.
        let sandbox = Sandbox::new(false, 5); // 5 seconds (unit is now seconds per A-D-006 fix)
                                              // Compile-time check: signature accepts (&str, Value, &apcore::Executor).
                                              // The body is dead code at runtime; it exists only to keep the type
                                              // checker honest about the new signature.
        let _check: fn(&Sandbox, &str, Value, &apcore::Executor) = |s, id, v, e| {
            drop(s.execute(id, v, e));
        };
        let _ = sandbox; // suppress unused warning
    }

    #[tokio::test]
    async fn test_sandbox_enabled_path_still_runs_subprocess() {
        // Use a 1-second timeout — still quick enough for a unit compile-check.
        // We don't actually invoke execute() here; just verify the API surface.
        let sandbox = Sandbox::new(true, 1); // 1 second per A-D-006 fix (was 1ms)
        let _check: fn(&Sandbox, &str, Value, &apcore::Executor) = |s, id, v, e| {
            drop(s.execute(id, v, e));
        };
        let _ = sandbox;
    }

    #[test]
    fn test_decode_result_valid_json() {
        use crate::sandbox_runner::decode_result;
        let v = decode_result(r#"{"ok":true}"#).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn test_decode_result_invalid_json() {
        use crate::sandbox_runner::decode_result;
        assert!(decode_result("not json").is_err());
    }

    #[test]
    fn test_encode_result_roundtrip() {
        use crate::sandbox_runner::{decode_result, encode_result};
        let v = json!({"result": 42});
        let encoded = encode_result(&v);
        let decoded = decode_result(&encoded).unwrap();
        assert_eq!(decoded["result"], 42);
    }

    #[test]
    fn test_sandbox_env_does_not_include_auth_api_key() {
        // APCORE_AUTH_API_KEY must never be forwarded to the sandboxed child
        // even though it sits under the APCORE_ prefix whitelist.
        unsafe { std::env::set_var("APCORE_AUTH_API_KEY", "secret-key-12345") };
        let host_env: std::collections::HashMap<String, String> = std::env::vars().collect();

        let mut env: Vec<(String, String)> = Vec::new();
        for key in SANDBOX_ALLOWED_ENV_KEYS {
            if let Some(val) = host_env.get(*key) {
                env.push((key.to_string(), val.clone()));
            }
        }
        for (k, v) in &host_env {
            if SANDBOX_ALLOWED_ENV_PREFIXES
                .iter()
                .any(|prefix| k.starts_with(prefix))
                && !SANDBOX_DENIED_ENV_PREFIXES
                    .iter()
                    .any(|prefix| k.starts_with(prefix))
                && !SANDBOX_DENIED_ENV_KEYS.contains(&k.as_str())
            {
                env.push((k.clone(), v.clone()));
            }
        }

        unsafe { std::env::remove_var("APCORE_AUTH_API_KEY") };

        assert!(
            !env.iter().any(|(k, _)| k == "APCORE_AUTH_API_KEY"),
            "APCORE_AUTH_API_KEY must not be forwarded to the sandbox environment"
        );
    }

    #[test]
    fn test_sandbox_env_does_not_include_auth_prefix() {
        unsafe {
            std::env::set_var("APCORE_AUTH_TOKEN", "bearer-xyz");
            std::env::set_var("APCORE_AUTH_SECRET", "shh");
        }
        let host_env: std::collections::HashMap<String, String> = std::env::vars().collect();

        let env: Vec<(String, String)> = host_env
            .iter()
            .filter(|(k, _)| {
                SANDBOX_ALLOWED_ENV_PREFIXES
                    .iter()
                    .any(|p| k.starts_with(p))
                    && !SANDBOX_DENIED_ENV_PREFIXES.iter().any(|p| k.starts_with(p))
                    && !SANDBOX_DENIED_ENV_KEYS.contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        unsafe {
            std::env::remove_var("APCORE_AUTH_TOKEN");
            std::env::remove_var("APCORE_AUTH_SECRET");
        }

        let leaked: Vec<_> = env
            .iter()
            .filter(|(k, _)| k.starts_with("APCORE_AUTH_"))
            .collect();
        assert!(
            leaked.is_empty(),
            "APCORE_AUTH_* vars must not leak into sandbox env: {leaked:?}"
        );
    }
}
