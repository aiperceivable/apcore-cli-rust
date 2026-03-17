// apcore-cli — Integration tests for check_approval().
// Protocol spec: FE-05

mod common;

use apcore_cli::approval::{check_approval, ApprovalError};
use serde_json::json;

#[tokio::test]
async fn test_check_approval_auto_approve_skips_prompt() {
    // auto_approve=true must return Ok without any TTY interaction.
    let module_def = json!({
        "module_id": "math.add",
        "annotations": {"requires_approval": true}
    });
    let result = check_approval(&module_def, true).await;
    assert!(
        result.is_ok(),
        "expected Ok for auto_approve=true: {result:?}"
    );
}

#[tokio::test]
async fn test_check_approval_no_tty_returns_error() {
    // In CI / non-TTY environments, approval must fail with NonInteractive.
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprintln!("Skipping non-TTY test (stdin is a TTY).");
        return;
    }
    let module_def = json!({
        "module_id": "math.add",
        "annotations": {"requires_approval": true}
    });
    let result = check_approval(&module_def, false).await;
    assert!(
        matches!(result, Err(ApprovalError::NonInteractive { .. })),
        "expected NonInteractive error, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_check_approval_denied_returns_error() {
    // Uses check_approval_with_tty + prompt_with_reader is internal, so
    // we test the NonInteractive path (no TTY = denial path in non-tty env).
    // The full Denied path is covered in unit tests via prompt_with_reader.
    let module_def = json!({
        "module_id": "math.add",
        "annotations": {"requires_approval": true}
    });
    // With is_tty=false and auto_approve=false, expect NonInteractive (not Denied).
    let result = apcore_cli::approval::check_approval_with_tty(&module_def, false, false).await;
    assert!(
        matches!(result, Err(ApprovalError::NonInteractive { .. })),
        "expected NonInteractive error, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_check_approval_timeout_returns_error() {
    // Timeout is covered in unit tests (prompt_with_reader with timeout_secs=0).
    // Here we verify the Timeout variant Display message.
    let err = ApprovalError::Timeout {
        module_id: "math.add".to_string(),
        seconds: 1,
    };
    assert!(err.to_string().contains("math.add"));
    assert!(err.to_string().contains("1s"));
}

#[tokio::test]
async fn test_approval_timeout_error_display() {
    let err = ApprovalError::Timeout {
        module_id: "math.add".to_string(),
        seconds: 30,
    };
    assert!(err.to_string().contains("math.add"));
    assert!(err.to_string().contains("30"));
}
