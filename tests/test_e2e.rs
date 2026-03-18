// apcore-cli — End-to-end CLI invocation tests.
// These tests invoke the binary-level CLI and check exit codes + stdout.

mod common;

/// Helper: invoke the CLI binary with given args and return the full Output.
fn run_apcore(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
        .args(args)
        .output()
        .expect("failed to spawn apcore-cli")
}

// ---------------------------------------------------------------------------
// Original placeholder tests (converted to real tests)
// ---------------------------------------------------------------------------

#[test]
fn test_e2e_help_flag_exits_0() {
    // `apcore-cli --extensions-dir ./examples/extensions --help` must exit 0.
    let out = run_apcore(&["--extensions-dir", "./examples/extensions", "--help"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn test_e2e_version_flag() {
    // `apcore-cli --version` must print a version string and exit 0.
    let out = run_apcore(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.is_empty(), "version output must not be empty");
}

#[test]
fn test_e2e_list_command() {
    // `apcore-cli --extensions-dir ... list` must exit 0.
    let out = run_apcore(&["--extensions-dir", "./examples/extensions", "list"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn test_e2e_describe_command() {
    let out = run_apcore(&[
        "--extensions-dir",
        "./examples/extensions",
        "describe",
        "math.add",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "describe math.add must exit 0 with real extensions"
    );
}

#[test]
fn test_e2e_execute_math_add() {
    // External subcommand "math.add" routes through dispatch_module and
    // executes via run.sh with real example extensions.
    let out = run_apcore(&[
        "--extensions-dir",
        "./examples/extensions",
        "math.add",
        "--a",
        "3",
        "--b",
        "4",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "math.add --a 3 --b 4 must exit 0, got {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"sum\""),
        "output must contain sum field: {stdout}"
    );
}

#[test]
fn test_e2e_stdin_piping() {
    // Pipe JSON input via stdin to exec math.add.
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
        .args([
            "--extensions-dir",
            "./examples/extensions",
            "exec",
            "math.add",
            "--input",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"a\": 10, \"b\": 20}")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "exec math.add --input - must exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"sum\""),
        "output must contain sum: {stdout}"
    );
}

#[test]
fn test_e2e_unknown_module_exits_44() {
    let out = run_apcore(&[
        "--extensions-dir",
        "./examples/extensions",
        "nonexistent.module",
    ]);
    assert_eq!(out.status.code(), Some(44));
}

#[test]
fn test_e2e_exec_subcommand_routes_to_dispatch() {
    // exec subcommand uses --input - for JSON input (schema flags like --a
    // are only available via the external subcommand path, not exec).
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
        .args([
            "--extensions-dir",
            "./examples/extensions",
            "exec",
            "math.add",
            "--input",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"a\": 1, \"b\": 2}")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "exec math.add must exit 0, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"sum\""),
        "output must contain sum: {stdout}"
    );
}

#[test]
fn test_e2e_exec_invalid_module_id_exits_2() {
    // An invalid module ID format (no dot separator) should exit 2.
    let out = run_apcore(&[
        "--extensions-dir",
        "./examples/extensions",
        "exec",
        "INVALID",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "exec with invalid module ID format must exit 2, got {:?}",
        out.status.code()
    );
}

#[test]
fn test_e2e_external_invalid_module_id_exits_2() {
    // An invalid module ID format via external subcommand should exit 2.
    let out = run_apcore(&["--extensions-dir", "./examples/extensions", "INVALID"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "external subcommand with invalid module ID must exit 2, got {:?}",
        out.status.code()
    );
}

#[test]
fn test_e2e_invalid_input_exits_2() {
    // Missing required positional for describe exits 2.
    let out = run_apcore(&["--extensions-dir", "./examples/extensions", "describe"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn test_e2e_completion_bash() {
    // `apcore-cli --extensions-dir ... completion bash` must exit 0.
    let out = run_apcore(&[
        "--extensions-dir",
        "./examples/extensions",
        "completion",
        "bash",
    ]);
    assert_eq!(out.status.code(), Some(0));
}

// ---------------------------------------------------------------------------
// Tests from the task specification (RED phase)
// ---------------------------------------------------------------------------

#[test]
fn test_help_flag_exits_0_contains_builtins() {
    let out = run_apcore(&["--extensions-dir", "./examples/extensions", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    for builtin in ["list", "describe", "completion"] {
        assert!(stdout.contains(builtin), "help must mention '{builtin}'");
    }
}

#[test]
fn test_version_flag_format() {
    let out = run_apcore(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let output = String::from_utf8_lossy(&out.stdout);
    // Must match "apcore-cli, version X.Y.Z" per FR-01-04.
    assert!(
        output.contains("apcore-cli") && output.contains("version"),
        "version output: {output}"
    );
}

#[test]
fn test_extensions_dir_missing_exits_47() {
    let out = run_apcore(&[
        "--extensions-dir",
        "/tmp/definitely_does_not_exist_apcore_test",
    ]);
    assert_eq!(out.status.code(), Some(47));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Extensions directory not found") || stderr.contains("not found"));
}

#[test]
fn test_extensions_dir_env_var_respected() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
        .env("APCORE_EXTENSIONS_ROOT", "./examples/extensions")
        .args(["--help"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn test_extensions_dir_flag_overrides_env() {
    // --extensions-dir flag takes precedence over APCORE_EXTENSIONS_ROOT.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
        .env("APCORE_EXTENSIONS_ROOT", "/nonexistent/path")
        .args(["--extensions-dir", "./examples/extensions", "--help"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn test_prog_name_in_version_output() {
    // When invoked as "apcore-cli", version output must contain "apcore-cli".
    let out = run_apcore(&["--version"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("apcore-cli"), "stdout: {stdout}");
}
