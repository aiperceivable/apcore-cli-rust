// apcore-cli — Integration tests for CLI dispatcher.
// Protocol spec: FE-01 (build_module_command, collect_input, validate_module_id)

mod common;

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

use apcore_cli::cli::{collect_input_from_reader, validate_module_id, CliError};
use apcore_cli::collect_input;
use serde_json::{json, Value};

/// Mutex serializes tests that manipulate the global verbose help flag.
/// (`dead_code` false positive: statics in integration test files trigger
/// this warning even when used, because each test file is a separate crate.)
#[allow(dead_code)]
static VERBOSE_MUTEX: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// validate_module_id
// ---------------------------------------------------------------------------

#[test]
fn test_validate_module_id_valid_ids() {
    for id in ["math.add", "text.summarize", "a", "a.b.c"] {
        assert!(validate_module_id(id).is_ok(), "expected ok for '{id}'");
    }
}

#[test]
fn test_validate_module_id_too_long() {
    let long_id = "a".repeat(129);
    assert!(validate_module_id(&long_id).is_err());
}

#[test]
fn test_validate_module_id_invalid_formats() {
    for id in ["INVALID!ID", "123abc", ".leading.dot", "a..b", "a."] {
        assert!(validate_module_id(id).is_err(), "expected error for '{id}'");
    }
}

#[test]
fn test_validate_module_id_max_length_ok() {
    let max_id = "a".repeat(128);
    assert!(validate_module_id(&max_id).is_ok());
}

// ---------------------------------------------------------------------------
// collect_input
// ---------------------------------------------------------------------------

#[test]
fn test_collect_input_no_stdin_drops_null_values() {
    let mut kwargs = HashMap::new();
    kwargs.insert("a".to_string(), json!(5));
    kwargs.insert("b".to_string(), Value::Null);
    let result = collect_input(None, kwargs, false).unwrap();
    assert_eq!(result.get("a"), Some(&json!(5)));
    assert!(!result.contains_key("b"), "Null values must be dropped");
}

#[test]
fn test_collect_input_stdin_valid_json() {
    let stdin_bytes = b"{\"x\": 42}";
    let reader = Cursor::new(stdin_bytes.to_vec());
    let result = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap();
    assert_eq!(result.get("x"), Some(&json!(42)));
}

#[test]
fn test_collect_input_cli_overrides_stdin() {
    let stdin_bytes = b"{\"a\": 5}";
    let reader = Cursor::new(stdin_bytes.to_vec());
    let mut kwargs = HashMap::new();
    kwargs.insert("a".to_string(), json!(99));
    let result = collect_input_from_reader(Some("-"), kwargs, false, reader).unwrap();
    assert_eq!(result.get("a"), Some(&json!(99)), "CLI must override STDIN");
}

#[test]
fn test_collect_input_oversized_stdin_rejected() {
    let big = vec![b' '; 10 * 1024 * 1024 + 1];
    let reader = Cursor::new(big);
    let err = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap_err();
    assert!(matches!(err, CliError::InputTooLarge { .. }));
}

#[test]
fn test_collect_input_large_input_allowed() {
    let mut payload = b"{\"k\": \"".to_vec();
    payload.extend(vec![b'x'; 11 * 1024 * 1024]);
    payload.extend(b"\"}");
    let reader = Cursor::new(payload);
    let result = collect_input_from_reader(Some("-"), HashMap::new(), true, reader);
    assert!(
        result.is_ok(),
        "large_input=true must accept oversized payload"
    );
}

#[test]
fn test_collect_input_invalid_json_returns_error() {
    let reader = Cursor::new(b"not json at all".to_vec());
    let err = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap_err();
    assert!(matches!(err, CliError::JsonParse(_)));
}

#[test]
fn test_collect_input_non_object_json_returns_error() {
    let reader = Cursor::new(b"[1, 2, 3]".to_vec());
    let err = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap_err();
    assert!(matches!(err, CliError::NotAnObject));
}

#[test]
fn test_collect_input_empty_stdin_returns_empty_map() {
    let reader = Cursor::new(b"".to_vec());
    let result = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_collect_input_no_stdin_flag_returns_cli_kwargs() {
    let mut kwargs = HashMap::new();
    kwargs.insert("foo".to_string(), json!("bar"));
    let result = collect_input(None, kwargs.clone(), false).unwrap();
    assert_eq!(result.get("foo"), Some(&json!("bar")));
}

// ---------------------------------------------------------------------------
// build_module_command
// ---------------------------------------------------------------------------

/// Helper: build a test module command with a simple two-property schema.
fn build_test_module_command(name: &str) -> clap::Command {
    use apcore_cli::cli::{build_module_command, ModuleExecutor};
    use std::sync::Arc;

    struct NoOpExecutor;
    impl ModuleExecutor for NoOpExecutor {}

    let module_def = apcore::registry::registry::ModuleDescriptor {
        name: name.to_string(),
        annotations: apcore::module::ModuleAnnotations::default(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer"},
                "b": {"type": "integer"}
            }
        }),
        output_schema: json!({}),
        enabled: true,
        tags: vec![],
        dependencies: vec![],
    };
    let executor: Arc<dyn ModuleExecutor> = Arc::new(NoOpExecutor);
    build_module_command(&module_def, executor).expect("should build command")
}

#[test]
fn test_build_module_command_creates_command() {
    let _guard = VERBOSE_MUTEX.lock().unwrap();
    // Ensure verbose is on so built-in flags are visible, then restore.
    apcore_cli::cli::set_verbose_help(true);
    let cmd = build_test_module_command("math.add");
    apcore_cli::cli::set_verbose_help(false);

    assert_eq!(cmd.get_name(), "math.add");
    // Verify built-in flags are present.
    let arg_names: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
    assert!(arg_names.contains(&"input"), "missing --input flag");
    assert!(arg_names.contains(&"yes"), "missing --yes flag");
    assert!(arg_names.contains(&"format"), "missing --format flag");
    assert!(arg_names.contains(&"sandbox"), "missing --sandbox flag");
    // Verify schema-derived args are present.
    assert!(arg_names.contains(&"a"), "missing schema arg --a");
    assert!(arg_names.contains(&"b"), "missing schema arg --b");
}

// ---------------------------------------------------------------------------
// verbose help flag — built-in option visibility
// ---------------------------------------------------------------------------

#[test]
fn builtin_flags_hidden_by_default() {
    let _guard = VERBOSE_MUTEX.lock().unwrap();
    apcore_cli::cli::set_verbose_help(false);
    let cmd = build_test_module_command("test.hidden");
    let input_arg = cmd.get_arguments().find(|a| a.get_id() == "input").unwrap();
    assert!(
        input_arg.is_hide_set(),
        "--input should be hidden when verbose is off"
    );
    let yes_arg = cmd.get_arguments().find(|a| a.get_id() == "yes").unwrap();
    assert!(
        yes_arg.is_hide_set(),
        "--yes should be hidden when verbose is off"
    );
    let sandbox_arg = cmd
        .get_arguments()
        .find(|a| a.get_id() == "sandbox")
        .unwrap();
    assert!(
        sandbox_arg.is_hide_set(),
        "--sandbox should be hidden when verbose is off"
    );
}

#[test]
fn builtin_flags_shown_when_verbose() {
    let _guard = VERBOSE_MUTEX.lock().unwrap();
    apcore_cli::cli::set_verbose_help(true);
    let cmd = build_test_module_command("test.visible");
    let input_arg = cmd.get_arguments().find(|a| a.get_id() == "input").unwrap();
    assert!(
        !input_arg.is_hide_set(),
        "--input should be visible when verbose is on"
    );
    let yes_arg = cmd.get_arguments().find(|a| a.get_id() == "yes").unwrap();
    assert!(
        !yes_arg.is_hide_set(),
        "--yes should be visible when verbose is on"
    );
    // sandbox is always hidden (not yet implemented)
    let sandbox_arg = cmd
        .get_arguments()
        .find(|a| a.get_id() == "sandbox")
        .unwrap();
    assert!(
        sandbox_arg.is_hide_set(),
        "--sandbox should always be hidden (not yet implemented)"
    );
    // Reset to default state.
    apcore_cli::cli::set_verbose_help(false);
}
