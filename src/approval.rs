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
            tracing::warn!(
                "APCORE_CLI_AUTO_APPROVE is set to '{}', expected '1'. Ignoring.",
                val
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
/// Otherwise prompts the user with a 60-second timeout.
///
/// # Errors
/// * `ApprovalError::NonInteractive` — stdin is not an interactive terminal
/// * `ApprovalError::Denied`         — user typed anything other than `y`/`yes`
/// * `ApprovalError::Timeout`        — prompt timed out
pub async fn check_approval(
    module_def: &serde_json::Value,
    auto_approve: bool,
) -> Result<(), ApprovalError> {
    check_approval_with_timeout(module_def, auto_approve, DEFAULT_APPROVAL_TIMEOUT_SECS).await
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

// CliApprovalHandler struct deleted per audit finding D9-003 / A-001:
// the struct had zero callers and no methods beyond `new()`. The actual
// approval gating is performed by the standalone functions `check_approval`
// and `check_approval_with_tty` above. The --yes and --approval-timeout
// CLI flags flow directly into those functions without a wrapper struct.

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
        let result =
            check_approval(&json!({"annotations": {"requires_approval": false}}), false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn skip_when_no_annotations() {
        let result = check_approval(&json!({}), false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn skip_when_requires_approval_string_true() {
        let result = check_approval(
            &json!({"annotations": {"requires_approval": "true"}}),
            false,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn bypass_auto_approve_true() {
        let result = check_approval(&module(true), true).await;
        assert!(result.is_ok(), "auto_approve=true must bypass");
    }

    #[test]
    fn bypass_env_var_one() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval(&module(true), false));
        unsafe { std::env::remove_var("APCORE_CLI_AUTO_APPROVE") };
        assert!(result.is_ok(), "APCORE_CLI_AUTO_APPROVE=1 must bypass");
    }

    #[test]
    fn yes_flag_priority_over_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("APCORE_CLI_AUTO_APPROVE", "1") };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(check_approval(&module(true), true));
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
}
