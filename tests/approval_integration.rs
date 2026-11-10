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
    let result = apcore_cli::approval::check_approval(&module_def, false, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn module_with_requires_approval_false_skips_gate() {
    let module_def = json!({
        "module_id": "open-module",
        "annotations": {"requires_approval": false}
    });
    let result = apcore_cli::approval::check_approval(&module_def, false, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn module_with_auto_approve_true_skips_gate() {
    let module_def = json!({
        "module_id": "guarded-module",
        "annotations": {"requires_approval": true}
    });
    let result = apcore_cli::approval::check_approval(&module_def, true, None).await;
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
    let result = apcore_cli::approval::check_approval(&module_def, false, None).await;
    assert!(matches!(result, Err(ApprovalError::NonInteractive { .. })));
}

#[cfg(unix)]
#[test]
fn embedded_approval_rejects_without_reading_tty() {
    use apcore_cli::{cli::exec_command, dispatch_module_with_result, MockRegistry};
    use std::fs::File;
    use std::io::IsTerminal;
    use std::os::fd::FromRawFd;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CHILD_MARKER: &str = "APCORE_CLI_TEST_EMBEDDED_APPROVAL_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        assert!(std::io::stdin().is_terminal());
        let registry = MockRegistry::new(vec![json!({
            "module_id": "test.guarded",
            "input_schema": {"type": "object"},
            "output_schema": {"type": "object"},
            "annotations": {"requires_approval": true}
        })]);
        let executor = apcore::Executor::new(apcore::Registry::new(), apcore::Config::default());
        let matches =
            exec_command().get_matches_from(["exec", "test.guarded", "--approval-timeout", "1"]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(
            runtime.block_on(dispatch_module_with_result(
                "test.guarded",
                &matches,
                &registry,
                &executor,
                false,
            )),
            Err(EXIT_APPROVAL_DENIED)
        );
        return;
    }

    let mut master = -1;
    let mut slave = -1;
    // The PTY is created with system defaults. Keep its master open without
    // supplying input so an accidental blocking approval reader cannot finish.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
    let _terminal = unsafe { File::from_raw_fd(master) };
    let input = unsafe { File::from_raw_fd(slave) };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "embedded_approval_rejects_without_reading_tty",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .env("RUST_MIN_STACK", "8388608")
        .env_remove("APCORE_CLI_AUTO_APPROVE")
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("Embedded approval left a blocking terminal reader running");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("Proceed?"), "{stderr}");
}
