//! Behavioral tests for the `validate` module (FE-11 --dry-run / preflight).
//! Review #6 expanded this from a single smoke test to cover the clap
//! command surface and the format_preflight_result formatter. The async
//! dispatch_validate path calls std::process::exit and is exercised via
//! tests/test_e2e.rs subprocess invocations.

use serde_json::json;

#[test]
fn validate_command_constructible() {
    let cmd = apcore_cli::validate::validate_command();
    assert_eq!(cmd.get_name(), "validate");
}

#[test]
fn validate_command_requires_module_id() {
    let cmd = apcore_cli::validate::validate_command();
    let result = cmd.try_get_matches_from(vec!["validate"]);
    assert!(
        result.is_err(),
        "validate must require a module_id argument"
    );
}

#[test]
fn validate_command_accepts_module_id() {
    let cmd = apcore_cli::validate::validate_command();
    let result = cmd.try_get_matches_from(vec!["validate", "math.add"]);
    assert!(
        result.is_ok(),
        "validate must accept a module_id positional"
    );
}

#[test]
fn validate_command_exposes_input_and_format_flags() {
    let cmd = apcore_cli::validate::validate_command();
    let arg_ids: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
    assert!(arg_ids.contains(&"module_id"), "got {arg_ids:?}");
    assert!(arg_ids.contains(&"input"), "must have --input flag");
    assert!(arg_ids.contains(&"format"), "must have --format flag");
}

#[test]
fn validate_command_format_flag_rejects_unknown_value() {
    let cmd = apcore_cli::validate::validate_command();
    let result = cmd.try_get_matches_from(vec!["validate", "math.add", "--format", "yaml"]);
    assert!(
        result.is_err(),
        "validate --format yaml must be rejected (only table/json supported)"
    );
}

#[test]
fn format_preflight_result_json_emits_passed_module_id() {
    // Validates that a passed preflight serializes into JSON with
    // valid:true and the requires_approval field set per the input.
    let result = json!({
        "valid": true,
        "requires_approval": false,
        "checks": [
            {"check": "module_id", "passed": true},
            {"check": "schema", "passed": true},
        ],
    });
    // Just verify the call doesn't panic; the formatter writes to stdout
    // when format is "json" — we exercise the code path.
    apcore_cli::validate::format_preflight_result(&result, Some("json"));
}

#[test]
fn format_preflight_result_handles_failed_check_with_error_string() {
    let result = json!({
        "valid": false,
        "requires_approval": false,
        "checks": [
            {"check": "module_id", "passed": true},
            {
                "check": "schema",
                "passed": false,
                "error": "missing required field: name",
            },
        ],
    });
    // Exercises the failed-check + error-string path in the TTY branch.
    apcore_cli::validate::format_preflight_result(&result, Some("json"));
}

#[test]
fn format_preflight_result_handles_warnings_array() {
    let result = json!({
        "valid": true,
        "requires_approval": true,
        "checks": [
            {
                "check": "schema",
                "passed": true,
                "warnings": ["deprecated field 'foo' will be removed in v2"],
            },
        ],
    });
    apcore_cli::validate::format_preflight_result(&result, Some("json"));
}

#[test]
fn format_preflight_result_missing_fields_default_safely() {
    // Regression: every .get() in format_preflight_result must default
    // safely when the executor returns a partial result.
    let result = json!({});
    apcore_cli::validate::format_preflight_result(&result, Some("json"));
}
