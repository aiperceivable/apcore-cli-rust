// apcore-cli — Integration tests for the approval gate (Task 6: cli-integration).
// Protocol spec: FE-05

use apcore_cli::{ApprovalError, EXIT_APPROVAL_DENIED};
use serde_json::json;

/// Helper: map ApprovalError to exit code (mirrors main.rs logic).
fn exit_code_for(e: &ApprovalError) -> i32 {
    match e {
        ApprovalError::Denied { .. }
        | ApprovalError::NonInteractive { .. }
        | ApprovalError::Timeout { .. } => EXIT_APPROVAL_DENIED,
    }
}

#[tokio::test]
async fn all_approval_errors_map_to_exit_46() {
    let denied = ApprovalError::Denied {
        module_id: "m".into(),
    };
    let non_interactive = ApprovalError::NonInteractive {
        module_id: "m".into(),
    };
    let timeout = ApprovalError::Timeout {
        module_id: "m".into(),
        seconds: 60,
    };

    assert_eq!(exit_code_for(&denied), 46);
    assert_eq!(exit_code_for(&non_interactive), 46);
    assert_eq!(exit_code_for(&timeout), 46);
    assert_eq!(EXIT_APPROVAL_DENIED, 46);
}

#[tokio::test]
async fn module_without_requires_approval_skips_gate() {
    // No annotations field → gate must return Ok immediately.
    let module_def = json!({"module_id": "open-module"});
    let result = apcore_cli::approval::check_approval(&module_def, false).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn module_with_requires_approval_false_skips_gate() {
    let module_def = json!({
        "module_id": "open-module",
        "annotations": {"requires_approval": false}
    });
    let result = apcore_cli::approval::check_approval(&module_def, false).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn module_with_auto_approve_true_skips_gate() {
    let module_def = json!({
        "module_id": "guarded-module",
        "annotations": {"requires_approval": true}
    });
    let result = apcore_cli::approval::check_approval(&module_def, true).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn module_non_tty_no_bypass_returns_approval_error() {
    // This test is environment-sensitive. In a real TTY environment it
    // will invoke the interactive prompt. Skipped if stdin is a TTY.
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprintln!("Skipping non-TTY integration test (stdin is a TTY).");
        return;
    }
    let module_def = json!({
        "module_id": "guarded-module",
        "annotations": {"requires_approval": true}
    });
    let result = apcore_cli::approval::check_approval(&module_def, false).await;
    assert!(matches!(result, Err(ApprovalError::NonInteractive { .. })));
}
