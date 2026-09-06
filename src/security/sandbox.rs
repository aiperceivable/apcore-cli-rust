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

    /// Access to `module_id` was denied by the ACL attached to the
    /// `apcore::Executor` passed into [`Sandbox::execute`].
    ///
    /// Enforced in-method (FE-14 section 4.10) rather than only at the CLI's
    /// dispatch call site: `_sandboxed_execute` spawns a subprocess that
    /// builds its own bare `Registry` + `Executor` from inherited `APCORE_*`
    /// env vars and never sees the attached ACL, so `--sandbox` would
    /// otherwise be a complete access-control bypass regardless of whether
    /// the call site remembers to gate it.
    #[error("Permission denied for module '{module_id}'")]
    AclDenied { module_id: String },
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

/// Resolve `path` to an absolute path for injection into the sandbox child's
/// environment, WITHOUT requiring the target to exist on disk.
///
/// Tries `std::fs::canonicalize` first — it also resolves symlinks, which is
/// the more faithful resolution when the target is real — and falls back to a
/// purely lexical absolutize against the parent's current working directory
/// only when that fails (typically because the path does not exist yet, e.g.
/// an `extensions.root` that will be populated later, or simply a typo the
/// operator should see surfaced as "module not found" rather than a mysteriously
/// re-rooted sandbox).
///
/// This mirrors Python's `Path.resolve()` / TypeScript's `path.resolve()`,
/// both of which succeed lexically even for a path that does not exist.
/// `std::fs::canonicalize` has no such fallback — its previous unconditional
/// use here (`unwrap_or_else(|_| ext_root.clone())`) silently forwarded the
/// UNRESOLVED, possibly-relative path on failure, re-rooting it inside the
/// child's fresh tempdir cwd (invariant 6, security.md's "extensions root not
/// propagated" class of defect).
fn absolutize_sandbox_path(path: &std::path::Path) -> PathBuf {
    if let Ok(canon) = std::fs::canonicalize(path) {
        return canon;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        // No sensible fallback if even the parent's own cwd is unavailable;
        // return the path unchanged rather than panicking.
        Err(_) => path.to_path_buf(),
    }
}

/// Read one chunk (up to `buf.len()` bytes) from `*pipe`. Returns the byte
/// count read, or `0` on EOF or a read error — either of which means the
/// caller should stop selecting on this pipe. Only called from a `select!`
/// arm guarded by `pipe.is_some()`, but returns `0` rather than panicking if
/// that invariant is ever violated.
async fn read_one_chunk<R: tokio::io::AsyncRead + Unpin>(
    pipe: &mut Option<R>,
    buf: &mut [u8],
) -> usize {
    match pipe.as_mut() {
        Some(r) => r.read(buf).await.unwrap_or(0),
        None => 0,
    }
}

/// Collect stdout/stderr from an already-spawned, already-written-to child up
/// to `cap` bytes per stream, actively killing the child the instant EITHER
/// stream's bound is hit rather than letting it run to completion.
///
/// D11-007 follow-up. This reads the two pipes concurrently via `select!` —
/// one chunk at a time, whichever pipe is ready first — rather than
/// `tokio::join!`ing a bounded read-to-completion on each. That distinction
/// is load-bearing, not stylistic: `join!` waits for BOTH futures, and a
/// bounded read only resolves early on hitting `cap` OR on EOF. A module that
/// overflows stdout while leaving stderr open and silent (the common case —
/// most modules write only to stdout) never delivers EOF on stderr while it
/// keeps running, so `join!` would hang on the stderr half FOREVER regardless
/// of how promptly the stdout half detected its overflow — no placement of a
/// kill call after such a `join!` can help, since the join itself never
/// completes without the very death-by-kill it is waiting to be told to
/// trigger. Selecting on both, chunk by chunk, means the moment CANONICALLY
/// EITHER stream crosses `cap` this function can act — independent of
/// whether the other stream ever produces anything at all.
///
/// Once either stream crosses `cap`, the child is killed via
/// `Child::start_kill` (signal only, no wait) and then reaped via `wait()`,
/// so a runaway module is terminated promptly under `OutputSizeExceeded`
/// instead of surviving until the caller's outer timeout (default 300s)
/// reaps it and misreports the overflow as an opaque `Timeout`.
///
/// Extracted from `_sandboxed_execute` (mirroring `build_sandbox_env`) so
/// this behaviour is unit-testable against a real, cheaply-spawned process
/// instead of requiring the `--internal-sandbox-runner` subprocess machinery
/// (which needs the compiled `apcore-cli` binary at `current_exe()` and is
/// excluded from ordinary `cargo test` runs).
async fn collect_capped_output(
    mut child: tokio::process::Child,
    cap: usize,
    module_id: &str,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), ModuleExecutionError> {
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut stdout_buf: Vec<u8> = Vec::new();
    let mut stderr_buf: Vec<u8> = Vec::new();
    let mut stdout_chunk = [0u8; 65536];
    let mut stderr_chunk = [0u8; 65536];

    let overflowed = loop {
        if stdout_pipe.is_none() && stderr_pipe.is_none() {
            // Both streams reached EOF within the cap: normal completion.
            break false;
        }
        tokio::select! {
            n = read_one_chunk(&mut stdout_pipe, &mut stdout_chunk), if stdout_pipe.is_some() => {
                if n == 0 {
                    stdout_pipe = None;
                } else {
                    stdout_buf.extend_from_slice(&stdout_chunk[..n]);
                }
            }
            n = read_one_chunk(&mut stderr_pipe, &mut stderr_chunk), if stderr_pipe.is_some() => {
                if n == 0 {
                    stderr_pipe = None;
                } else {
                    stderr_buf.extend_from_slice(&stderr_chunk[..n]);
                }
            }
        }
        if stdout_buf.len() > cap || stderr_buf.len() > cap {
            break true;
        }
    };

    if overflowed {
        // Actively terminate — do not wait for the child to exit on its own.
        // `start_kill` sends the signal without waiting; the subsequent
        // `wait()` reaps the process so it does not linger as a zombie.
        // Both are best-effort: if the child already exited between the read
        // completing and this point, either call simply becomes a no-op /
        // returns quickly.
        let _ = child.start_kill();
        let _ = child.wait().await;

        // D11-007: classify the overflow stream so operators can identify
        // which direction breached the cap.
        let overflow_stream = match (stdout_buf.len() > cap, stderr_buf.len() > cap) {
            (true, true) => "stdout+stderr",
            (true, false) => "stdout",
            (false, true) => "stderr",
            // Unreachable given `overflowed`, but match exhaustively to avoid
            // a panic if the condition is widened.
            (false, false) => "stdout",
        }
        .to_string();
        return Err(ModuleExecutionError::OutputSizeExceeded {
            module_id: module_id.to_string(),
            limit_bytes: cap,
            overflow_stream,
        });
    }

    let status = child
        .wait()
        .await
        .map_err(|e| ModuleExecutionError::SpawnFailed(e.to_string()))?;
    Ok((stdout_buf, stderr_buf, status))
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
            // protocol-spec exit code. This path already carries the
            // attached ACL, since it runs through the same executor's own
            // pipeline (`acl_check` step).
            return executor
                .call(module_id, input_data, None, None)
                .await
                .map_err(ModuleExecutionError::ModuleError);
        }

        // Defense-in-depth ACL gate (FE-14 section 4.10). `_sandboxed_execute`
        // spawns a subprocess that builds its OWN bare `Registry` + `Executor`
        // from inherited `APCORE_*` env vars, which never sees `executor`'s
        // attached ACL — so without a check here, `--sandbox` is a complete
        // access-control bypass. `cli::dispatch_module` already gates this at
        // its call site, but that gate protects only callers that remember to
        // invoke it; checking again here means the guarantee holds for EVERY
        // caller of `Sandbox::execute`, present or future, regardless of
        // call-site correctness.
        //
        // `caller_id` is left as `None` (resolves to `@external`) for the same
        // reason the call-site gate does: apcore makes `Context::caller_id`
        // unsettable by callers, and a top-level dispatch is always
        // `@external`. The context and the arguments projection are both
        // required non-`None` — PROTOCOL_SPEC 6.5 makes a conditional rule a
        // non-match without a context, and an `arguments`-scoped rule inert
        // without the projection, so passing either as `None` would leave
        // those rule classes silently inert on this path while they fire
        // in-process.
        if let Some(acl) = executor.acl.as_deref() {
            let gate_ctx = crate::acl_cmd::delegated_gate_context();
            let projection = apcore::acl::GovernanceProjection::from_arguments(&input_data);
            let decision = acl.check_access(None, module_id, Some(&gate_ctx), Some(&projection));
            if decision.access == "deny" {
                return Err(ModuleExecutionError::AclDenied {
                    module_id: module_id.to_string(),
                });
            }
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
            let resolved = absolutize_sandbox_path(ext_root);
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
            // not exist. Absolutize here so the inherited path stays valid
            // after the cwd switch, matching the explicit-override branch
            // above.
            if let Some(idx) = env.iter().position(|(k, _)| k == "APCORE_EXTENSIONS_ROOT") {
                let raw = env[idx].1.clone();
                let p = std::path::PathBuf::from(&raw);
                env[idx].1 = absolutize_sandbox_path(&p).to_string_lossy().into_owned();
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

        // Per-instance cap; defaults to SANDBOX_OUTPUT_SIZE_LIMIT_BYTES (64 MiB)
        // unless overridden via `with_max_output_bytes` (D1-004 parity).
        let cap = self.max_output_bytes;
        // `collect_capped_output` actively kills `child` the instant either
        // stream's cap is breached (D11-007 follow-up), so a runaway module
        // is terminated promptly instead of surviving until this OUTER
        // timeout fires and misreporting the overflow as `Timeout`.
        let (stdout_bytes, stderr_bytes, status) =
            timeout(timeout_dur, collect_capped_output(child, cap, module_id))
                .await
                .map_err(|_| ModuleExecutionError::Timeout {
                    module_id: module_id.to_string(),
                    timeout_secs: self.timeout_secs,
                })??;

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

    // -----------------------------------------------------------------------
    // Regression: FE-14 §4.10 defense-in-depth ACL gate on Sandbox::execute
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_sandbox_execute_denies_when_acl_denies_module_without_spawning() {
        // BLOCKER: `_sandboxed_execute` builds its own bare Registry +
        // Executor in the child and never sees the ACL attached to the
        // `executor` argument, so `--sandbox` was previously a complete
        // access-control bypass regardless of anything the CLI's dispatch
        // call site did. `Sandbox::execute` itself must refuse before ever
        // spawning the subprocess. If this regresses to "spawn anyway", this
        // test would hang or error out on subprocess machinery instead of
        // returning promptly with `AclDenied`.
        use apcore::acl::{ACLRule, ACL};

        let rule = ACLRule::new(
            vec!["@external".to_string()],
            vec!["denied.module".to_string()],
            "deny",
        );
        let acl = ACL::try_new(vec![rule], "allow", None).expect("well-formed ACL");

        let mut executor =
            apcore::Executor::new(apcore::Registry::new(), apcore::Config::default());
        executor.set_acl(acl);

        let sandbox = Sandbox::new(true, 5);
        let result = sandbox.execute("denied.module", json!({}), &executor).await;

        match result {
            Err(ModuleExecutionError::AclDenied { module_id }) => {
                assert_eq!(module_id, "denied.module");
            }
            other => panic!("expected AclDenied without spawning a subprocess, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sandbox_execute_allows_when_acl_allows_module() {
        // Discriminating case for the fix above: a hardcoded deny would pass
        // the BLOCKER test trivially. An allowed module must still reach
        // `_sandboxed_execute` (and fail there for an unrelated, sandbox-
        // machinery reason — there is no real `denied.module` binary to run —
        // rather than being rejected by the ACL gate).
        use apcore::acl::{ACLRule, ACL};

        let rule = ACLRule::new(
            vec!["@external".to_string()],
            vec!["some.other.module".to_string()],
            "deny",
        );
        let acl = ACL::try_new(vec![rule], "allow", None).expect("well-formed ACL");

        let mut executor =
            apcore::Executor::new(apcore::Registry::new(), apcore::Config::default());
        executor.set_acl(acl);

        // Very short timeout: we only care that the ACL gate did not reject
        // it outright; the subprocess itself is expected to fail fast since
        // `allowed.module` is not a real registered module.
        let sandbox = Sandbox::new(true, 1);
        let result = sandbox
            .execute("allowed.module", json!({}), &executor)
            .await;

        assert!(
            !matches!(result, Err(ModuleExecutionError::AclDenied { .. })),
            "an allowed module must not be rejected by the ACL gate, got: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Regression: extensions-root absolutize must not require existence
    // -----------------------------------------------------------------------

    #[test]
    fn test_with_extensions_root_absolutizes_a_nonexistent_relative_path() {
        // D11-W3 follow-up (explicit-override branch). `with_extensions_root`
        // deliberately accepts a path that may not exist yet (security.md:
        // "A relative `path` is accepted and resolved here rather than
        // rejected"). The previous
        // `canonicalize(..).unwrap_or_else(|_| ext_root.clone())` forwarded
        // the RAW, unresolved relative path whenever the target didn't exist
        // — silently re-rooting it inside the sandbox child's fresh tempdir
        // cwd (invariant 6's "extensions root not propagated" defect class).
        let relative = PathBuf::from("no-such-extensions-dir-abc987");
        assert!(!relative.exists(), "premise: the path must not exist");

        let s = Sandbox::new(true, 5).with_extensions_root(Some(relative.clone()));
        let env = s.build_sandbox_env(&std::collections::HashMap::new());

        let forwarded = env
            .iter()
            .find(|(k, _)| k == "APCORE_EXTENSIONS_ROOT")
            .map(|(_, v)| v.clone())
            .expect("APCORE_EXTENSIONS_ROOT must be forwarded");

        assert_ne!(
            forwarded,
            relative.to_string_lossy(),
            "must not silently forward the unresolved relative path when canonicalize fails"
        );
        assert!(
            std::path::Path::new(&forwarded).is_absolute(),
            "forwarded extensions root must be absolute even when the target \
             doesn't exist yet, got {forwarded:?}"
        );
        assert_eq!(
            std::path::Path::new(&forwarded),
            std::env::current_dir().unwrap().join(&relative),
            "must fall back to a lexical absolutize against the parent's cwd"
        );
    }

    // -----------------------------------------------------------------------
    // Regression: overflow must kill the child promptly, not wait for it
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_collect_capped_output_kills_runaway_child_promptly_on_overflow() {
        // D11-007 follow-up: a module that keeps writing past the cap must be
        // killed as soon as the cap is hit rather than left running until the
        // caller's outer timeout (default 300s) reaps it, which previously
        // turned a clear OutputSizeExceeded into a misleading Timeout.
        //
        // Uses a real, cheaply-spawned subprocess rather than the sandbox's
        // `--internal-sandbox-runner` path, which needs the compiled
        // `apcore-cli` binary and is excluded from ordinary `cargo test` runs
        // (see the #[ignore] tests in tests/security/test_sandbox.rs).
        //
        // The child writes past the cap ONCE and then spins on CPU without
        // touching its pipes again — deliberately NOT a plain `cat
        // /dev/zero`. A pure never-stop-writing child would die from SIGPIPE
        // the instant `collect_capped_output`'s bounded reader is dropped
        // (which closes our end of the pipe) regardless of whether an
        // explicit kill is issued, making that shape unable to discriminate
        // the fix from the bug it fixes. A child that goes CPU-bound after
        // its initial write never touches the (now-closed) pipe again, so it
        // is unaffected by SIGPIPE/EPIPE and can only be stopped by an
        // explicit kill — reproducing the "runaway module that keeps
        // running" scenario the fix targets.
        use std::process::Stdio;
        use tokio::process::Command;

        let child = Command::new("sh")
            .arg("-c")
            .arg("head -c 2000 /dev/zero; while true; do :; done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn test child");

        let started = std::time::Instant::now();
        // Generous outer bound: if the fix regresses to "wait for the child
        // to exit on its own", this future never resolves (the child never
        // exits) and this outer timeout fires first, failing the test with a
        // clear message instead of hanging the test suite.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            collect_capped_output(child, 1024, "overflow.module"),
        )
        .await
        .expect(
            "must resolve well before a long outer sandbox timeout would, not hang \
             waiting for a runaway child to exit on its own",
        );

        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "overflow must be detected and the child killed promptly, took {:?}",
            started.elapsed()
        );
        match result {
            Err(ModuleExecutionError::OutputSizeExceeded {
                overflow_stream, ..
            }) => {
                assert_eq!(overflow_stream, "stdout");
            }
            other => panic!("expected OutputSizeExceeded, got: {other:?}"),
        }
    }
}
