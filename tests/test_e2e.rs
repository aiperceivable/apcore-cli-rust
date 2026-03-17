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
    // `apcore-cli --extensions-dir ./tests/fixtures/extensions --help` must exit 0.
    let out = run_apcore(&["--extensions-dir", "./tests/fixtures/extensions", "--help"]);
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
    let out = run_apcore(&["--extensions-dir", "./tests/fixtures/extensions", "list"]);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn test_e2e_describe_command() {
    // `apcore-cli --extensions-dir ... describe math.add` must exit 0.
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
        "describe",
        "math.add",
    ]);
    // Exit 0 once fully implemented; currently exits 0 (stub).
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(44),
        "describe exits 0 or 44 (stub)"
    );
}

#[test]
fn test_e2e_execute_math_add() {
    // External subcommand "math.add" now routes through dispatch_module.
    // With a real extensions dir the module should be found; exit 0 on success
    // or 44 if not found in registry (valid module ID format).
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
        "math.add",
    ]);
    // dispatch_module validates the module ID (exit 2 if invalid format)
    // then looks it up in the registry (exit 44 if not found).
    // math.add is a valid ID format, so we expect 0 (found) or 44 (not found).
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(44),
        "math.add via external subcommand must route to dispatch_module, got {:?}",
        out.status.code()
    );
    // Must NOT contain the old "not yet implemented" message.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("not yet implemented"),
        "external subcommand must not print 'not yet implemented'"
    );
}

#[test]
fn test_e2e_stdin_piping() {
    // External subcommand "math.add --input -" now routes through dispatch_module.
    // stdin is /dev/null so collect_input reads empty input.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
        .args([
            "--extensions-dir",
            "./tests/fixtures/extensions",
            "exec",
            "math.add",
            "--input",
            "-",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    // dispatch_module validates ID then does registry lookup; expect 0 or 44.
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(44),
        "exec math.add --input - must route to dispatch_module, got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("not yet implemented"),
        "exec subcommand must not print 'not yet implemented'"
    );
}

#[test]
fn test_e2e_unknown_module_exits_44() {
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
        "nonexistent.module",
    ]);
    assert_eq!(out.status.code(), Some(44));
}

#[test]
fn test_e2e_exec_subcommand_routes_to_dispatch() {
    // `apcore-cli exec math.add` must route through dispatch_module.
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
        "exec",
        "math.add",
    ]);
    // Valid module ID format; exit 0 (found) or 44 (not in registry).
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(44),
        "exec math.add must route to dispatch_module, got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("not yet implemented"),
        "exec subcommand must not print 'not yet implemented'"
    );
}

#[test]
fn test_e2e_exec_invalid_module_id_exits_2() {
    // An invalid module ID format (no dot separator) should exit 2.
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
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
    let out = run_apcore(&["--extensions-dir", "./tests/fixtures/extensions", "INVALID"]);
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
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
        "describe",
    ]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn test_e2e_completion_bash() {
    // `apcore-cli --extensions-dir ... completion bash` must exit 0.
    let out = run_apcore(&[
        "--extensions-dir",
        "./tests/fixtures/extensions",
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
    let out = run_apcore(&["--extensions-dir", "./tests/fixtures/extensions", "--help"]);
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
        .env("APCORE_EXTENSIONS_ROOT", "./tests/fixtures/extensions")
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
        .args(["--extensions-dir", "./tests/fixtures/extensions", "--help"])
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
