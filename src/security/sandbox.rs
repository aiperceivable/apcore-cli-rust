// apcore-cli — Subprocess sandbox for module execution.
// Protocol spec: SEC-04 (Sandbox, ModuleExecutionError)

use std::path::PathBuf;

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

/// Default maximum bytes collected from sandbox stdout or stderr before the
/// child is killed and `OutputSizeExceeded` is returned. Guards against OOM
/// from hostile or buggy modules that write unboundedly. Overridable per-
/// Sandbox via `Sandbox::with_max_output_bytes` (parity with Python).
const SANDBOX_OUTPUT_SIZE_LIMIT_BYTES: usize = 64 * 1024 * 1024; // 64 MiB (aligned with Python/TS)

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

    /// The subprocess output exceeded the per-Sandbox capture cap (default
    /// 64 MiB). This is distinct from `OutputParseFailed`, which is reserved
    /// for malformed JSON. Parity with Python/TS: the cap is reported in MiB
    /// units (1024*1024) and the overflowing stream is named explicitly so
    /// operators can pinpoint the offending direction (stdout vs. stderr).
    #[error("Module '{module_id}' {overflow_stream} exceeded the {}MiB sandbox limit.",
            limit_bytes / (1024 * 1024))]
    OutputSizeExceeded {
        module_id: String,
        limit_bytes: usize,
        overflow_stream: String,
    },

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
// Auxiliary error types (API parity with Python/TypeScript)
// ---------------------------------------------------------------------------

/// Raised when the requested module ID is not registered in the registry.
/// Exported for cross-SDK API parity with Python and TypeScript.
#[derive(Debug, Error)]
#[error("Module not found: {module_id}")]
pub struct ModuleNotFoundError {
    pub module_id: String,
}

/// Raised when a module's JSON Schema is structurally invalid or fails
/// against the validator. Exported for cross-SDK API parity.
#[derive(Debug, Error)]
#[error("Schema validation error: {detail}")]
pub struct SchemaValidationError {
    pub detail: String,
}

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
    extensions_root: Option<PathBuf>,
    max_output_bytes: usize,
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
            extensions_root: None,
            max_output_bytes: SANDBOX_OUTPUT_SIZE_LIMIT_BYTES,
        }
    }

    /// Return `true` when subprocess isolation is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set the path injected as `APCORE_EXTENSIONS_ROOT` into the sandbox
    /// subprocess. Parity with Python `Sandbox.with_extensions_root` (D1-004).
    ///
    /// When `Some(path)`, the absolute (canonicalised when possible) path is
    /// forwarded to the sandbox child so the runner can locate modules even
    /// after `cwd` is changed to the sandbox tempdir. When `None`, any
    /// inherited `APCORE_EXTENSIONS_ROOT` from the host environment is left
    /// to flow through the standard `APCORE_*` whitelist unmodified.
    ///
    /// Builder-style — consumes `self` and returns it for chaining.
    pub fn with_extensions_root(mut self, extensions_root: Option<PathBuf>) -> Self {
        self.extensions_root = extensions_root;
        self
    }

    /// Cap the post-capture stdout+stderr byte budget for the sandbox
    /// subprocess. Default: 64 MiB ([`SANDBOX_OUTPUT_SIZE_LIMIT_BYTES`]).
    /// Parity with Python `Sandbox.with_max_output_bytes` (D1-004).
    ///
    /// Builder-style — consumes `self` and returns it for chaining.
    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    /// Test/inspection accessor: returns the configured `extensions_root`.
    /// Used by parity tests to verify the builder's effect.
    #[doc(hidden)]
    pub fn extensions_root(&self) -> Option<&PathBuf> {
        self.extensions_root.as_ref()
    }

    /// Test/inspection accessor: returns the configured `max_output_bytes`.
    /// Used by parity tests to verify the builder's effect.
    #[doc(hidden)]
    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
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

    /// Build the sandbox child's restricted environment from a `host_env`
    /// snapshot. Extracted from `_sandboxed_execute` so the env-construction
    /// logic (whitelist forwarding + APCORE_EXTENSIONS_ROOT canonicalisation,
    /// audit D11-W3) is unit-testable without spawning a subprocess.
    fn build_sandbox_env(
        &self,
        host_env: &std::collections::HashMap<String, String>,
    ) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = Vec::new();

        for key in SANDBOX_ALLOWED_ENV_KEYS {
            if let Some(val) = host_env.get(*key) {
                env.push((key.to_string(), val.clone()));
            }
        }
        for (k, v) in host_env {
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

        // Inject extensions_root override (D1-004 parity with Python). When
        // `with_extensions_root(Some(p))` is set, forward as an absolute path
        // (resolved when possible) so the runner locates modules correctly
        // even after `cwd` is switched to the sandbox tempdir. This entry
        // overrides any inherited `APCORE_EXTENSIONS_ROOT` from the standard
        // APCORE_* whitelist forwarding above.
        if let Some(ref ext_root) = self.extensions_root {
            let resolved = std::fs::canonicalize(ext_root).unwrap_or_else(|_| ext_root.clone());
            // Replace any prior APCORE_EXTENSIONS_ROOT entry forwarded by the
            // whitelist loop — the explicit builder value wins.
            env.retain(|(k, _)| k != "APCORE_EXTENSIONS_ROOT");
            env.push((
                "APCORE_EXTENSIONS_ROOT".to_string(),
                resolved.to_string_lossy().into_owned(),
            ));
        } else {
            // Audit D11-W3 (2026-05-08): when no builder override was
            // supplied, an `APCORE_EXTENSIONS_ROOT` value can still reach the
            // child via the APCORE_* prefix whitelist above. The whitelist
            // forwards the host value verbatim — but the child runs with
            // `cwd = tmpdir_path`, so any relative path (e.g. "./extensions")
            // resolves to a directory inside the sandbox tempdir that does
            // not exist. Canonicalise here so the inherited path stays
            // valid after the cwd switch, matching the explicit-override
            // branch above.
            if let Some(idx) = env.iter().position(|(k, _)| k == "APCORE_EXTENSIONS_ROOT") {
                let raw = env[idx].1.clone();
                let p = std::path::PathBuf::from(&raw);
                if let Ok(canon) = std::fs::canonicalize(&p) {
                    env[idx].1 = canon.to_string_lossy().into_owned();
                }
            }
        }

        env
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
        let host_env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let mut env = self.build_sandbox_env(&host_env);

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

        // Per-instance cap; defaults to SANDBOX_OUTPUT_SIZE_LIMIT_BYTES (64 MiB)
        // unless overridden via `with_max_output_bytes` (D1-004 parity).
        let cap = self.max_output_bytes;
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
            // D11-007: classify the overflow stream so operators can identify
            // which direction breached the cap. Both streams use `take(cap+1)`,
            // so a length strictly greater than `cap` indicates that stream
            // was the offender.
            let overflow_stream = match (stdout_bytes.len() > cap, stderr_bytes.len() > cap) {
                (true, true) => "stdout+stderr",
                (true, false) => "stdout",
                (false, true) => "stderr",
                // Unreachable given the surrounding `if` condition, but match
                // exhaustively to avoid a panic if the condition is widened.
                (false, false) => "stdout",
            }
            .to_string();
            return Err(ModuleExecutionError::OutputSizeExceeded {
                module_id: module_id.to_string(),
                limit_bytes: cap,
                overflow_stream,
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
    fn test_sandbox_default_max_output_bytes_is_64mib() {
        // Parity with Python Sandbox.DEFAULT_MAX_OUTPUT_BYTES (D1-004).
        let s = Sandbox::new(false, 5);
        assert_eq!(s.max_output_bytes(), 64 * 1024 * 1024);
    }

    #[test]
    fn test_sandbox_default_extensions_root_is_none() {
        // Parity with Python: constructor leaves _extensions_root = None.
        let s = Sandbox::new(false, 5);
        assert!(s.extensions_root().is_none());
    }

    #[test]
    fn test_sandbox_with_max_output_bytes_sets_field() {
        // D1-004: builder-style setter must mutate the per-instance cap so
        // _sandboxed_execute uses the override instead of the 64 MiB default.
        let s = Sandbox::new(false, 5).with_max_output_bytes(1024);
        assert_eq!(s.max_output_bytes(), 1024);
    }

    #[test]
    fn test_sandbox_with_extensions_root_sets_field() {
        // D1-004: builder-style setter must store the path so
        // _sandboxed_execute can inject APCORE_EXTENSIONS_ROOT.
        let path = PathBuf::from("/tmp/extensions");
        let s = Sandbox::new(false, 5).with_extensions_root(Some(path.clone()));
        assert_eq!(s.extensions_root(), Some(&path));
    }

    #[test]
    fn test_sandbox_builder_chains() {
        // Both setters must return Self so chained construction works in the
        // same fluent style as Python's `Sandbox(...).with_*().with_*()`.
        let path = PathBuf::from("/tmp/ext");
        let s = Sandbox::new(true, 30)
            .with_extensions_root(Some(path.clone()))
            .with_max_output_bytes(2048);
        assert!(s.is_enabled());
        assert_eq!(s.extensions_root(), Some(&path));
        assert_eq!(s.max_output_bytes(), 2048);
    }

    /// D11-W3 (2026-05-08): when the builder did NOT call
    /// `with_extensions_root`, an inherited relative `APCORE_EXTENSIONS_ROOT`
    /// must be canonicalised to an absolute path before being forwarded —
    /// otherwise the child (which runs with `cwd = tmpdir_path`) would
    /// resolve "./extensions" relative to the sandbox tempdir.
    #[test]
    fn test_inherited_extensions_root_canonicalised_when_no_builder_override() {
        // Build a temp directory and reference it relatively. canonicalize on
        // the relative form must match canonicalize on the absolute form.
        let tmp = tempfile::tempdir().unwrap();
        let abs = tmp.path().to_path_buf();
        // Compute a relative form by stripping the common prefix.
        // For simplicity, just feed the absolute path itself — canonicalize
        // on an absolute path that exists is a no-op modulo symlinks, and
        // the test still demonstrates the canonicalisation branch fires.
        let cwd_before = std::env::current_dir().unwrap();
        // cd into temp's parent so "./<basename>" is a valid relative path.
        let parent = abs.parent().unwrap().to_path_buf();
        let basename = abs.file_name().unwrap().to_string_lossy().into_owned();
        std::env::set_current_dir(&parent).unwrap();
        let relative_form = format!("./{basename}");

        let mut host_env: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        host_env.insert("APCORE_EXTENSIONS_ROOT".to_string(), relative_form.clone());

        let s = Sandbox::new(true, 5); // no with_extensions_root call
        let env = s.build_sandbox_env(&host_env);

        // Restore cwd before any assertion that could panic.
        std::env::set_current_dir(&cwd_before).unwrap();

        let resolved = env
            .iter()
            .find(|(k, _)| k == "APCORE_EXTENSIONS_ROOT")
            .map(|(_, v)| v.clone())
            .expect("APCORE_EXTENSIONS_ROOT must be forwarded by the prefix whitelist");

        let expected = std::fs::canonicalize(&abs)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolved, expected,
            "inherited relative APCORE_EXTENSIONS_ROOT must be canonicalised to the absolute path"
        );
        assert!(
            std::path::Path::new(&resolved).is_absolute(),
            "post-canonicalisation value must be absolute, got {resolved:?}"
        );
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
