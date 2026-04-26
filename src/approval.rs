// apcore-cli — Human-in-the-loop approval gate.
// Protocol spec: FE-05 (check_approval, ApprovalError)

use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types  (Task 1: error-types)
// ---------------------------------------------------------------------------

/// Errors returned by the approval gate.
/// All variants map to exit code 46 (EXIT_APPROVAL_DENIED).
#[derive(Debug, Error)]
pub enum ApprovalError {
    /// The operator denied execution.
    #[error("approval denied for module '{module_id}'")]
    Denied { module_id: String },

    /// No interactive TTY is available to prompt the user.
    #[error("no interactive terminal available for module '{module_id}'")]
    NonInteractive { module_id: String },

    /// The approval prompt timed out.
    #[error("approval timed out after {seconds}s for module '{module_id}'")]
    Timeout { module_id: String, seconds: u64 },
}

// ---------------------------------------------------------------------------
// Annotation extraction helpers  (Task 2: annotation-extraction)
// ---------------------------------------------------------------------------

/// Returns true only when `module_def["annotations"]["requires_approval"]`
/// is exactly `Value::Bool(true)`. Strings, integers, and null all return false.
fn get_requires_approval(module_def: &serde_json::Value) -> bool {
    module_def
        .get("annotations")
        .and_then(|a| a.get("requires_approval"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

/// Returns the custom approval message if present and non-empty, otherwise
/// the default: "Module '{module_id}' requires approval to execute."
fn get_approval_message(module_def: &serde_json::Value, module_id: &str) -> String {
    module_def
        .get("annotations")
        .and_then(|a| a.get("approval_message"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Module '{module_id}' requires approval to execute."))
}

/// Returns `module_def["module_id"]` or `module_def["canonical_id"]` if
/// either is a string, otherwise `"unknown"`.
fn get_module_id(module_def: &serde_json::Value) -> String {
    module_def
        .get("module_id")
        .or_else(|| module_def.get("canonical_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string()
}

// ---------------------------------------------------------------------------
// Prompt with injectable reader  (Task 5: tty-prompt-timeout)
// ---------------------------------------------------------------------------

/// Internal prompt implementation with an injectable reader function.
/// This enables unit testing without a real TTY.
///
/// `reader` is called on a blocking thread via `spawn_blocking`; it must
/// read one line from whatever source is appropriate (real stdin in
/// production, a mock in tests).
///
/// On timeout the blocking thread remains parked — this is acceptable
/// because the process exits immediately with code 46 after this function
/// returns `Err(ApprovalError::Timeout)`.
async fn prompt_with_reader<F>(
    module_id: &str,
    message: &str,
    timeout_secs: u64,
    reader: F,
) -> Result<(), ApprovalError>
where
    F: FnOnce() -> std::io::Result<String> + Send + 'static,
{
    // Display message and prompt to stderr.
    eprint!("{}\nProceed? [y/N]: ", message);
    // Flush stderr so the prompt appears before blocking.
    use std::io::Write;
    let _ = std::io::stderr().flush();

    let module_id_owned = module_id.to_string();
    let read_handle = tokio::task::spawn_blocking(reader);

    tokio::select! {
        result = read_handle => {
            match result {
                Ok(Ok(line)) => {
                    let input = line.trim().to_lowercase();
                    if input == "y" || input == "yes" {
                        tracing::info!(
                            "User approved execution of module '{}'.",
                            module_id_owned
                        );
                        Ok(())
                    } else {
                        tracing::warn!(
                            "Approval rejected by user for module '{}'.",
                            module_id_owned
                        );
                        eprintln!("Error: Approval denied.");
                        Err(ApprovalError::Denied { module_id: module_id_owned })
                    }
                }
                Ok(Err(io_err)) => {
                    // stdin closed (EOF) without input — treat as denial.
                    tracing::warn!(
                        "stdin read error for module '{}': {}",
                        module_id_owned,
                        io_err
                    );
                    eprintln!("Error: Approval denied.");
                    Err(ApprovalError::Denied { module_id: module_id_owned })
                }
                Err(join_err) => {
                    // spawn_blocking task panicked.
                    tracing::error!("spawn_blocking panicked: {}", join_err);
                    Err(ApprovalError::Denied { module_id: module_id_owned })
                }
            }
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)) => {
            tracing::warn!(
                "Approval timed out after {}s for module '{}'.",
                timeout_secs,
                module_id_owned
            );
            eprintln!("Error: Approval prompt timed out after {} seconds.", timeout_secs);
            Err(ApprovalError::Timeout {
                module_id: module_id_owned,
                seconds: timeout_secs,
            })
        }
    }
}

/// Production prompt: uses real stdin with a 60-second timeout.
async fn prompt_with_timeout(
    module_id: &str,
    message: &str,
    timeout_secs: u64,
) -> Result<(), ApprovalError> {
    prompt_with_reader(module_id, message, timeout_secs, || {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line)
    })
    .await
}

// ---------------------------------------------------------------------------
// check_approval_with_tty  (Tasks 3, 4, 5)
// ---------------------------------------------------------------------------

/// Default approval prompt timeout in seconds.
pub const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 60;

/// Internal implementation accepting `is_tty` for testability.
///
/// Delegates to [`check_approval_with_tty_timeout`] with
/// [`DEFAULT_APPROVAL_TIMEOUT_SECS`] so existing callers keep the 60-second
/// default.
pub async fn check_approval_with_tty(
    module_def: &serde_json::Value,
    auto_approve: bool,
    is_tty: bool,
) -> Result<(), ApprovalError> {
    check_approval_with_tty_timeout(
        module_def,
        auto_approve,
        is_tty,
        DEFAULT_APPROVAL_TIMEOUT_SECS,
    )
    .await
}

/// Testable gate that honors a configurable timeout.
///
/// Decision order:
/// 1. Skip entirely if `requires_approval` is not strict bool `true`.
/// 2. Bypass if `auto_approve == true` (--yes flag).
/// 3. Bypass if `APCORE_CLI_AUTO_APPROVE == "1"` (exact match).
/// 4. Reject if `!is_tty` (NonInteractive).
/// 5. Prompt interactively with `timeout_secs` timeout.
pub async fn check_approval_with_tty_timeout(
    module_def: &serde_json::Value,
    auto_approve: bool,
    is_tty: bool,
    timeout_secs: u64,
) -> Result<(), ApprovalError> {
    if !get_requires_approval(module_def) {
        return Ok(());
    }

    let module_id = get_module_id(module_def);

    // Bypass: --yes flag (highest priority)
    if auto_approve {
        tracing::info!(
            "Approval bypassed via --yes flag for module '{}'.",
            module_id
        );
        return Ok(());
    }

    // Bypass: APCORE_CLI_AUTO_APPROVE env var
    match std::env::var("APCORE_CLI_AUTO_APPROVE").as_deref() {
        Ok("1") => {
            tracing::info!(
                "Approval bypassed via APCORE_CLI_AUTO_APPROVE for module '{}'.",
                module_id
            );
            return Ok(());
        }
        Ok("") | Err(_) => {
            // Not set or empty — fall through silently.
        }
        Ok(val) => {
            // D10-009 cross-SDK parity: emit to stderr (matching TS and
            // Python) so the user-visible channel is consistent regardless
            // of whether a tracing subscriber is configured. Spec at
            // apcore-cli/docs/features/approval-gate.md:122 says "Log
            // WARNING" which was ambiguous; the three SDKs now agree on
            // stderr.
            eprintln!(
                "Warning: APCORE_CLI_AUTO_APPROVE is set to '{val}', expected '1'. Ignoring."
            );
        }
    }

    // Non-TTY rejection
    if !is_tty {
        eprintln!(
            "Error: Module '{}' requires approval but no interactive terminal is available. \
             Use --yes or set APCORE_CLI_AUTO_APPROVE=1 to bypass.",
            module_id
        );
        tracing::error!(
            "Non-interactive environment, no bypass provided for module '{}'.",
            module_id
        );
        return Err(ApprovalError::NonInteractive { module_id });
    }

    // TTY prompt with caller-specified timeout.
    let message = get_approval_message(module_def, &module_id);
    prompt_with_timeout(&module_id, &message, timeout_secs).await
}

// ---------------------------------------------------------------------------
// check_approval — public API  (Task 5)
// ---------------------------------------------------------------------------

/// Gate module execution behind an interactive approval prompt.
///
/// Returns `Ok(())` immediately if `requires_approval` is not `true`.
/// Bypasses the prompt if `auto_approve` is `true` or the env var
/// `APCORE_CLI_AUTO_APPROVE` is set to exactly `"1"`.
/// Returns `Err(ApprovalError::NonInteractive)` if stdin is not a TTY.
/// Otherwise prompts the user with a per-call timeout.
///
/// `timeout` accepts `Option<u64>` for cross-SDK parity with Python and TS,
/// which both expose `(module_def, auto_approve, timeout)` 3-arg signatures.
/// `None` falls back to [`DEFAULT_APPROVAL_TIMEOUT_SECS`]; `Some(n)` selects
/// an explicit window. Internally delegates to
/// [`check_approval_with_timeout`] (Rust convention: `_with_*` suffix for the
/// concrete-parameter variant).
///
/// # Errors
/// * `ApprovalError::NonInteractive` — stdin is not an interactive terminal
/// * `ApprovalError::Denied`         — user typed anything other than `y`/`yes`
/// * `ApprovalError::Timeout`        — prompt timed out
pub async fn check_approval(
    module_def: &serde_json::Value,
    auto_approve: bool,
    timeout: Option<u64>,
) -> Result<(), ApprovalError> {
    let secs = timeout.unwrap_or(DEFAULT_APPROVAL_TIMEOUT_SECS);
    check_approval_with_timeout(module_def, auto_approve, secs).await
}

/// Configurable-timeout variant of [`check_approval`]. Resolve the timeout
/// from the `--approval-timeout` CLI flag or `cli.approval_timeout` config
/// before calling.
pub async fn check_approval_with_timeout(
    module_def: &serde_json::Value,
    auto_approve: bool,
    timeout_secs: u64,
) -> Result<(), ApprovalError> {
    use std::io::IsTerminal;
    let is_tty = std::io::stdin().is_terminal();
    check_approval_with_tty_timeout(module_def, auto_approve, is_tty, timeout_secs).await
}

// ---------------------------------------------------------------------------
// ApprovalResult — apcore ApprovalHandler protocol shape (D10-006)
// ---------------------------------------------------------------------------

/// Outcome of an [`CliApprovalHandler::request_approval`] / `check_approval`
/// invocation. Mirrors the apcore ApprovalHandler protocol shape that Python
/// and TypeScript SDKs return as a `dict { status, approved_by | reason }`
/// duck-typed against `ApprovalResult`.
///
/// Cross-SDK parity (D10-006, 2026-04-26): previously the Rust handler
/// returned `Result<(), ApprovalError>`, which meant a Rust handler instance
/// could not satisfy the apcore protocol callback signature. Callers of the
/// standalone [`check_approval`] / [`check_approval_with_timeout`] still get
/// the typed-error form for compose-friendly error chaining; the
/// protocol-callback path goes through `CliApprovalHandler` and returns
/// `ApprovalResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalStatus {
    /// User (or a bypass mechanism) authorised the call.
    Approved,
    /// User denied, or no TTY was available.
    Rejected,
    /// The interactive prompt did not receive a response in time.
    Timeout,
}

/// Result of an approval request. Equivalent to the Python/TS protocol shape
/// `{ status, approved_by, reason }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResult {
    /// Approval state.
    pub status: ApprovalStatus,
    /// Identifier of the approver when `status == Approved`. Standard values
    /// match Python parity: `"auto_approve"` (--yes flag),
    /// `"env_auto_approve"` (`APCORE_CLI_AUTO_APPROVE=1`), or `"tty_user"`
    /// (interactive prompt). `None` for non-Approved results.
    pub approved_by: Option<String>,
    /// Human-readable reason when `status == Rejected` or `Timeout`.
    /// `None` for `Approved` results.
    pub reason: Option<String>,
}

impl ApprovalResult {
    /// Convenience constructor for the approved-via-flag case.
    pub fn approved_via(approved_by: impl Into<String>) -> Self {
        Self {
            status: ApprovalStatus::Approved,
            approved_by: Some(approved_by.into()),
            reason: None,
        }
    }

    /// Convenience constructor for rejection.
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            status: ApprovalStatus::Rejected,
            approved_by: None,
            reason: Some(reason.into()),
        }
    }

    /// Convenience constructor for timeout.
    pub fn timed_out(reason: impl Into<String>) -> Self {
        Self {
            status: ApprovalStatus::Timeout,
            approved_by: None,
            reason: Some(reason.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// CliApprovalHandler — ApprovalHandler protocol adapter
// ---------------------------------------------------------------------------

/// Implements the apcore ApprovalHandler protocol so SDK consumers can pass
/// a CLI-backed handler to `executor.set_approval_handler(handler)`.
///
/// `request_approval` and `check_approval` return [`ApprovalResult`] for
/// cross-SDK protocol parity (D10-006). The standalone module-level
/// `check_approval` / `check_approval_with_timeout` continue to return
/// `Result<(), ApprovalError>` for callers that prefer typed-error
/// semantics in pure Rust code.
pub struct CliApprovalHandler {
    /// Auto-approve without prompting the user.
    pub auto_approve: bool,
    /// Maximum seconds to wait for interactive approval (0 = wait indefinitely).
    pub timeout_secs: u64,
}

impl CliApprovalHandler {
    /// Create a new handler.
    pub fn new(auto_approve: bool, timeout_secs: u64) -> Self {
        Self {
            auto_approve,
            timeout_secs,
        }
    }

    /// Request approval for a module, using the CLI interactive prompt.
    ///
    /// Returns an [`ApprovalResult`] matching the Python/TS protocol shape:
    /// `Approved/auto_approve` for the `--yes` flag bypass,
    /// `Approved/env_auto_approve` for `APCORE_CLI_AUTO_APPROVE=1`,
    /// `Approved/tty_user` for an interactive yes,
    /// `Rejected` for non-TTY or user denial,
    /// `Timeout` when the prompt window expires.
    ///
    /// Mirrors the bypass-priority and message logic of
    /// [`check_approval_with_tty_timeout`] but folded into the protocol-shape
    /// return path.
    pub async fn request_approval(&self, module_def: &serde_json::Value) -> ApprovalResult {
        let module_id = get_module_id(module_def);

        // Skip if approval is not required.
        if !get_requires_approval(module_def) {
            return ApprovalResult::approved_via("not_required");
        }

        // Bypass: --yes flag (highest priority).
        if self.auto_approve {
            tracing::info!(
                "Approval bypassed via --yes flag for module '{}'.",
                module_id
            );
            return ApprovalResult::approved_via("auto_approve");
        }

        // Bypass: APCORE_CLI_AUTO_APPROVE env var.
        match std::env::var("APCORE_CLI_AUTO_APPROVE").as_deref() {
            Ok("1") => {
                tracing::info!(
                    "Approval bypassed via APCORE_CLI_AUTO_APPROVE for module '{}'.",
                    module_id
                );
                return ApprovalResult::approved_via("env_auto_approve");
            }
            Ok("") | Err(_) => {}
            Ok(val) => {
                tracing::warn!(
                    "APCORE_CLI_AUTO_APPROVE is set to '{}', expected '1'. Ignoring.",
                    val
                );
            }
        }

        // Non-TTY rejection.
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            tracing::error!(
                "Non-interactive environment, no bypass provided for module '{}'.",
                module_id
            );
            return ApprovalResult::rejected(format!(
                "Module '{module_id}' requires approval but no interactive terminal is available. \
                 Use --yes or set APCORE_CLI_AUTO_APPROVE=1 to bypass."
            ));
        }

        // TTY prompt with caller-specified timeout.
        let message = get_approval_message(module_def, &module_id);
        match prompt_with_timeout(&module_id, &message, self.timeout_secs).await {
            Ok(()) => ApprovalResult::approved_via("tty_user"),
            Err(ApprovalError::Timeout { seconds, .. }) => ApprovalResult::timed_out(format!(
                "Approval prompt timed out after {seconds} seconds."
            )),
            Err(_) => ApprovalResult::rejected("User denied approval".to_string()),
        }
    }

    /// Alias for [`request_approval`] (matches the Python / TypeScript
    /// `check_approval` method name on the handler).
    pub async fn check_approval(&self, module_def: &serde_json::Value) -> ApprovalResult {
        self.request_approval(module_def).await
    }
}

// Type aliases so callers can match by variant-like name (parity with Python/TS).
/// Alias for [`ApprovalError`] — the denial variant. Use `ApprovalError::Denied` to match.
pub type ApprovalDeniedError = ApprovalError;
/// Alias for [`ApprovalError`] — the timeout variant. Use `ApprovalError::Timeout` to match.
pub type ApprovalTimeoutError = ApprovalError;

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// Global mutex serializes all tests that read or write env vars.
    /// Env vars are process-global; parallel tokio tests will race without this.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // --- Task 1: error-types ---

    #[test]
    fn error_denied_display() {
        let e = ApprovalError::Denied {
            module_id: "my-module".into(),
        };
        assert_eq!(e.to_string(), "approval denied for module 'my-module'");
    }

    #[test]
    fn error_non_interactive_display() {
        let e = ApprovalError::NonInteractive {
            module_id: "my-module".into(),
        };
        assert_eq!(
            e.to_string(),
            "no interactive terminal available for module 'my-module'"
        );
    }

    #[test]
    fn error_timeout_display() {
        let e = ApprovalError::Timeout {
            module_id: "my-module".into(),
            seconds: 60,
        };
        assert_eq!(
            e.to_string(),
            "approval timed out after 60s for module 'my-module'"
        );
    }

    #[test]
    fn error_variants_are_debug() {
        let d = format!(
            "{:?}",
            ApprovalError::Denied {
                module_id: "x".into()
            }
        );
        assert!(d.contains("Denied"));
    }

    // --- Task 2: annotation-extraction ---

    #[test]
    fn requires_approval_true_returns_true() {
        let v = json!({"annotations": {"requires_approval": true}});
        assert!(get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_false_returns_false() {
        let v = json!({"annotations": {"requires_approval": false}});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_string_true_returns_false() {
        let v = json!({"annotations": {"requires_approval": "true"}});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_int_one_returns_false() {
        let v = json!({"annotations": {"requires_approval": 1}});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_null_returns_false() {
        let v = json!({"annotations": {"requires_approval": null}});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_absent_returns_false() {
        let v = json!({"annotations": {}});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_no_annotations_returns_false() {
        let v = json!({});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn requires_approval_annotations_null_returns_false() {
        let v = json!({"annotations": null});
        assert!(!get_requires_approval(&v));
    }

    #[test]
    fn approval_message_custom() {
        let v = json!({"annotations": {"approval_message": "Please confirm."}});
        assert_eq!(get_approval_message(&v, "mod-x"), "Please confirm.");
    }

    #[test]
    fn approval_message_default_when_absent() {
        let v = json!({"annotations": {}});
        assert_eq!(
            get_approval_message(&v, "mod-x"),
            "Module 'mod-x' requires approval to execute."
        );
    }

    #[test]
    fn approval_message_default_when_not_string() {
        let v = json!({"annotations": {"approval_message": 42}});
        assert_eq!(
            get_approval_message(&v, "mod-x"),
            "Module 'mod-x' requires approval to execute."
        );
    }

    #[test]
    fn module_id_from_module_id_field() {
        let v = json!({"module_id": "my-module"});
        assert_eq!(get_module_id(&v), "my-module");
    }

    #[test]
    fn module_id_from_canonical_id_field() {
        let v = json!({"canonical_id": "canon-module"});
        assert_eq!(get_module_id(&v), "canon-module");
    }

    #[test]
    fn module_id_unknown_when_absent() {
        let v = json!({});
        assert_eq!(get_module_id(&v), "unknown");
    }

    // --- Task 3: bypass-logic ---

    fn module(requires: bool) -> serde_json::Value {
        json!({
            "module_id": "test-module",
            "annotations": { "requires_approval": requires }
        })
    }

    #[tokio::test]
    async fn skip_when_requires_approval_false() {
        let result = check_approval(
            &json!({"annotations": {"requires_approval": false}}),
            false,
            None,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn skip_when_no_annotations() {
        let result = check_approval(&json!({}), false, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn skip_when_requires_approval_string_true() {
        let result = check_approval(
            &json!({"annotations": {"requires_approval": "true"}}),
            false,
            None,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn bypass_auto_approve_true() {
        let result = check_approval(&module(true), true, None).await;
        assert!(result.is_ok(), "auto_approve=true must bypass");
    }

    #[tokio::test]
    async fn explicit_timeout_some_delegates_to_with_timeout() {
        // Some(0) is the strongest evidence the timeout argument is wired
        // through to check_approval_with_timeout — a 0-second timeout would
        // immediately time out an actual TTY prompt. Bypass via auto_approve
        // so this test does not need a TTY.
        let result = check_approval(&module(true), true, Some(0)).await;
        assert!(
            result.is_ok(),
            "auto_approve must bypass before timeout matters"
        );
    }

    #[test]
    fn bypass_env_var_one() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval(&module(true), false, None));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        assert!(result.is_ok(), "APCORE_CLI_AUTO_APPROVE=1 must bypass");
    }

    #[test]
    fn yes_flag_priority_over_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval(&module(true), true, None));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        assert!(result.is_ok());
    }

    // --- Task 4: non-tty-rejection ---

    fn module_requiring_approval() -> serde_json::Value {
        json!({
            "module_id": "test-module",
            "annotations": { "requires_approval": true }
        })
    }

    #[test]
    fn non_tty_no_bypass_returns_non_interactive_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval_with_tty(
            &module_requiring_approval(),
            false,
            false,
        ));
        match result {
            Err(ApprovalError::NonInteractive { module_id }) => {
                assert_eq!(module_id, "test-module");
            }
            other => panic!("expected NonInteractive error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_tty_with_yes_flag_bypasses_before_tty_check() {
        let result = check_approval_with_tty(&module_requiring_approval(), true, false).await;
        assert!(result.is_ok(), "auto_approve bypasses TTY check");
    }

    #[test]
    fn non_tty_with_env_var_bypasses_before_tty_check() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval_with_tty(
            &module_requiring_approval(),
            false,
            false,
        ));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        assert!(result.is_ok(), "env var bypass happens before TTY check");
    }

    #[test]
    fn non_tty_env_var_not_one_returns_non_interactive() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "true") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval_with_tty(
            &module_requiring_approval(),
            false,
            false,
        ));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        assert!(matches!(result, Err(ApprovalError::NonInteractive { .. })));
    }

    // --- Task 5: tty-prompt-timeout ---

    #[tokio::test]
    async fn user_types_y_returns_ok() {
        let result = prompt_with_reader("test-module", "Requires approval.", 60, || {
            Ok("y\n".to_string())
        })
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn user_types_yes_returns_ok() {
        let result = prompt_with_reader("test-module", "Requires approval.", 60, || {
            Ok("yes\n".to_string())
        })
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn user_types_yes_uppercase_returns_ok() {
        let result = prompt_with_reader("test-module", "Requires approval.", 60, || {
            Ok("YES\n".to_string())
        })
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn user_types_n_returns_denied() {
        let result = prompt_with_reader("test-module", "Requires approval.", 60, || {
            Ok("n\n".to_string())
        })
        .await;
        assert!(matches!(result, Err(ApprovalError::Denied { .. })));
    }

    #[tokio::test]
    async fn user_presses_enter_returns_denied() {
        let result = prompt_with_reader("test-module", "Requires approval.", 60, || {
            Ok("\n".to_string())
        })
        .await;
        assert!(matches!(result, Err(ApprovalError::Denied { .. })));
    }

    #[tokio::test]
    async fn user_types_garbage_returns_denied() {
        let result = prompt_with_reader("test-module", "Requires approval.", 60, || {
            Ok("maybe\n".to_string())
        })
        .await;
        assert!(matches!(result, Err(ApprovalError::Denied { .. })));
    }

    #[tokio::test]
    async fn timeout_returns_timeout_error() {
        let result = prompt_with_reader(
            "test-module",
            "Requires approval.",
            0, // fires immediately
            || {
                // Simulate a slow/blocking read that never returns in time.
                std::thread::sleep(std::time::Duration::from_secs(10));
                Ok("y\n".to_string())
            },
        )
        .await;
        match result {
            Err(ApprovalError::Timeout { module_id, seconds }) => {
                assert_eq!(module_id, "test-module");
                assert_eq!(seconds, 0);
            }
            other => panic!("expected Timeout, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn check_approval_custom_message_displayed() {
        let module_def = json!({
            "module_id": "mod-custom",
            "annotations": {
                "requires_approval": true,
                "approval_message": "Custom: please confirm."
            }
        });
        // With auto_approve=true, bypass fires before TTY prompt.
        let result = check_approval_with_tty(&module_def, true, true).await;
        assert!(result.is_ok());
    }

    async fn check_approval_with_tty_timeout_honors_custom_value_before_prompt_inner() {
        let module_def = json!({
            "module_id": "mod-non-interactive",
            "annotations": {"requires_approval": true}
        });
        // is_tty=false with non-default timeout should still return
        // NonInteractive (the timeout only applies to the interactive
        // prompt path).
        let result = check_approval_with_tty_timeout(&module_def, false, false, 42).await;
        match result {
            Err(ApprovalError::NonInteractive { module_id }) => {
                assert_eq!(module_id, "mod-non-interactive");
            }
            other => panic!("expected NonInteractive, got {other:?}"),
        }
    }

    #[test]
    fn check_approval_with_tty_timeout_honors_custom_value_before_prompt() {
        // Closes the review finding: --approval-timeout was captured and
        // discarded, prompt_with_timeout got a literal 60. The new
        // _with_timeout variant must accept a caller-specified timeout and
        // not break the pre-prompt decision order (requires_approval,
        // --yes, env var, is_tty). Run on a dedicated runtime so the sync
        // ENV_MUTEX guard is never held across an await point (clippy
        // await_holding_lock).
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(check_approval_with_tty_timeout_honors_custom_value_before_prompt_inner());
    }

    #[tokio::test]
    async fn check_approval_with_timeout_honors_auto_approve_bypass() {
        let module_def = json!({
            "module_id": "mod-bypass",
            "annotations": {"requires_approval": true}
        });
        // --yes bypass must fire regardless of timeout setting.
        let result = check_approval_with_timeout(&module_def, true, 7).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn prompt_with_reader_timeout_respects_nonzero_value() {
        // Directly verifies the timeout value is threaded through and
        // surfaces in the ApprovalError::Timeout.seconds field — guards
        // against regression where a hard-coded 60 overrides the caller's
        // input (the exact bug the review flagged at approval.rs:230).
        let result = prompt_with_reader("mod-threaded", "Needs approval.", 3, || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            Ok("y\n".to_string())
        })
        .await;
        match result {
            Err(ApprovalError::Timeout { module_id, seconds }) => {
                assert_eq!(module_id, "mod-threaded");
                assert_eq!(seconds, 3, "timeout must propagate caller value, not 60");
            }
            other => panic!("expected Timeout with seconds=3, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // CliApprovalHandler — ApprovalResult shape (D10-006)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn handler_returns_approved_via_auto_approve_for_yes_flag() {
        let handler = CliApprovalHandler::new(true, 60);
        let result = handler.request_approval(&module(true)).await;
        assert_eq!(result.status, ApprovalStatus::Approved);
        assert_eq!(result.approved_by.as_deref(), Some("auto_approve"));
        assert!(result.reason.is_none());
    }

    #[tokio::test]
    async fn handler_returns_approved_not_required_when_no_annotation() {
        let handler = CliApprovalHandler::new(false, 60);
        let result = handler.request_approval(&module(false)).await;
        assert_eq!(result.status, ApprovalStatus::Approved);
        assert_eq!(result.approved_by.as_deref(), Some("not_required"));
    }

    #[test]
    fn handler_returns_approved_via_env_for_one_value() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handler = CliApprovalHandler::new(false, 60);
        let result = rt.block_on(handler.request_approval(&module(true)));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        assert_eq!(result.status, ApprovalStatus::Approved);
        assert_eq!(result.approved_by.as_deref(), Some("env_auto_approve"));
    }

    #[test]
    fn handler_yes_flag_priority_over_env() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let handler = CliApprovalHandler::new(true, 60);
        let result = rt.block_on(handler.request_approval(&module(true)));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        // Both bypass paths qualify; --yes takes priority — must say
        // "auto_approve" not "env_auto_approve".
        assert_eq!(result.status, ApprovalStatus::Approved);
        assert_eq!(result.approved_by.as_deref(), Some("auto_approve"));
    }

    #[test]
    fn approval_result_constructors_set_status_and_fields() {
        let approved = ApprovalResult::approved_via("tty_user");
        assert_eq!(approved.status, ApprovalStatus::Approved);
        assert_eq!(approved.approved_by.as_deref(), Some("tty_user"));
        assert!(approved.reason.is_none());

        let rejected = ApprovalResult::rejected("user said no");
        assert_eq!(rejected.status, ApprovalStatus::Rejected);
        assert!(rejected.approved_by.is_none());
        assert_eq!(rejected.reason.as_deref(), Some("user said no"));

        let timeout = ApprovalResult::timed_out("60s expired");
        assert_eq!(timeout.status, ApprovalStatus::Timeout);
        assert!(timeout.approved_by.is_none());
        assert_eq!(timeout.reason.as_deref(), Some("60s expired"));
    }

    #[tokio::test]
    async fn handler_check_approval_aliases_request_approval() {
        let handler = CliApprovalHandler::new(true, 60);
        let request_result = handler.request_approval(&module(true)).await;
        let check_result = handler.check_approval(&module(true)).await;
        assert_eq!(request_result, check_result);
    }
}
