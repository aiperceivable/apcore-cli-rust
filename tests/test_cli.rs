// apcore-cli — Integration tests for CLI dispatcher.
// Protocol spec: FE-01 (build_module_command, collect_input, validate_module_id)

mod common;

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

use apcore_cli::cli::{collect_input_from_reader, validate_module_id, CliError};
use apcore_cli::collect_input;
use serde_json::{json, Value};

/// Mutex serializes tests that manipulate the global all-options help flag.
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
    // PROTOCOL_SPEC §2.7 — bumped from 128 to 192 in spec 1.6.0-draft.
    let long_id = "a".repeat(193);
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
    // PROTOCOL_SPEC §2.7 — bumped from 128 to 192 in spec 1.6.0-draft.
    let max_id = "a".repeat(192);
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
    use apcore_cli::cli::build_module_command;

    let module_def = apcore::registry::registry::ModuleDescriptor {
        module_id: name.to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer"},
                "b": {"type": "integer"}
            }
        }),
        output_schema: json!({}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(apcore::module::ModuleAnnotations::default()),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    build_module_command(&module_def).expect("should build command")
}

#[test]
fn test_build_module_command_creates_command() {
    let _guard = VERBOSE_MUTEX.lock().unwrap();
    // Ensure all-options is on so built-in flags are visible, then restore.
    apcore_cli::cli::set_all_options_help(true);
    let cmd = build_test_module_command("math.add");
    apcore_cli::cli::set_all_options_help(false);

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
// all-options help flag — built-in option visibility
// ---------------------------------------------------------------------------

#[test]
fn builtin_flags_hidden_by_default() {
    let _guard = VERBOSE_MUTEX.lock().unwrap();
    apcore_cli::cli::set_all_options_help(false);
    let cmd = build_test_module_command("test.hidden");
    let input_arg = cmd.get_arguments().find(|a| a.get_id() == "input").unwrap();
    assert!(
        input_arg.is_hide_set(),
        "--input should be hidden when all-options is off"
    );
    let yes_arg = cmd.get_arguments().find(|a| a.get_id() == "yes").unwrap();
    assert!(
        yes_arg.is_hide_set(),
        "--yes should be hidden when all-options is off"
    );
    let sandbox_arg = cmd
        .get_arguments()
        .find(|a| a.get_id() == "sandbox")
        .unwrap();
    assert!(
        sandbox_arg.is_hide_set(),
        "--sandbox should be hidden when all-options is off"
    );
}

#[test]
fn builtin_flags_shown_when_all_options() {
    let _guard = VERBOSE_MUTEX.lock().unwrap();
    apcore_cli::cli::set_all_options_help(true);
    let cmd = build_test_module_command("test.visible");
    let input_arg = cmd.get_arguments().find(|a| a.get_id() == "input").unwrap();
    assert!(
        !input_arg.is_hide_set(),
        "--input should be visible when all-options is on"
    );
    let yes_arg = cmd.get_arguments().find(|a| a.get_id() == "yes").unwrap();
    assert!(
        !yes_arg.is_hide_set(),
        "--yes should be visible when all-options is on"
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
    apcore_cli::cli::set_all_options_help(false);
}

// ---------------------------------------------------------------------------
// Schema $ref resolution error propagation (review #8)
// ---------------------------------------------------------------------------

#[test]
fn test_build_module_command_propagates_circular_ref_error() {
    // Regression for review #8: a circular $ref in input_schema must surface
    // as CliError::SchemaRefResolution mapped to EXIT_SCHEMA_CIRCULAR_REF (48),
    // not silently fall back to the un-resolved schema.
    use apcore_cli::cli::{build_module_command, CliError};

    let module_def = apcore::registry::registry::ModuleDescriptor {
        module_id: "circular.test".to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: json!({
            "$ref": "#/definitions/x",
            "definitions": { "x": { "$ref": "#/definitions/x" } }
        }),
        output_schema: json!({}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(apcore::module::ModuleAnnotations::default()),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    let err = build_module_command(&module_def).expect_err("circular ref must surface as error");
    assert!(
        matches!(err, CliError::SchemaRefResolution { .. }),
        "expected SchemaRefResolution, got {err:?}"
    );
    assert_eq!(err.exit_code(), apcore_cli::EXIT_SCHEMA_CIRCULAR_REF);
}

#[test]
fn test_build_module_command_unresolvable_ref_maps_to_exit_45() {
    // Audit D10-002 (2026-05-08): an unresolvable `$ref` (target missing)
    // must surface with EXIT_SCHEMA_VALIDATION_ERROR (45). Previously every
    // SchemaRefResolution variant collapsed to 48, so callers could not
    // distinguish "schema is invalid" from "schema cannot be inlined".
    use apcore_cli::cli::{build_module_command, CliError};

    let module_def = apcore::registry::registry::ModuleDescriptor {
        module_id: "unresolvable.test".to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: json!({
            "$ref": "#/definitions/missing",
            "definitions": { "x": { "type": "string" } }
        }),
        output_schema: json!({}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(apcore::module::ModuleAnnotations::default()),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    let err =
        build_module_command(&module_def).expect_err("unresolvable ref must surface as error");
    assert!(
        matches!(err, CliError::SchemaRefResolution { .. }),
        "expected SchemaRefResolution, got {err:?}"
    );
    assert_eq!(
        err.exit_code(),
        apcore_cli::EXIT_SCHEMA_VALIDATION_ERROR,
        "unresolvable $ref must map to EXIT_SCHEMA_VALIDATION_ERROR (45), not 48"
    );
}

struct StatusModule {
    streaming: bool,
}

#[async_trait::async_trait]
impl apcore::module::Module for StatusModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn description(&self) -> &str {
        "Read status"
    }

    fn annotations(&self) -> apcore::ModuleAnnotations {
        apcore::ModuleAnnotations {
            streaming: self.streaming,
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        _inputs: Value,
        _ctx: &apcore::Context<Value>,
    ) -> Result<Value, apcore::errors::ModuleError> {
        Ok(json!({"status": "embedded-result-ready", "details": {"progress": 100}}))
    }

    fn as_streaming(&self) -> Option<&dyn apcore::StreamingModule> {
        self.streaming.then_some(self)
    }

    fn stream(&self, inputs: Value, ctx: &apcore::Context<Value>) -> Option<apcore::ChunkStream> {
        self.streaming
            .then(|| apcore::StreamingModule::stream_typed(self, inputs, ctx))
    }
}

impl apcore::StreamingModule for StatusModule {
    fn stream_typed(&self, _inputs: Value, _ctx: &apcore::Context<Value>) -> apcore::ChunkStream {
        Box::pin(futures_util::stream::iter([
            Ok(json!({"status": "embedded-stream-ready"})),
            Ok(json!({"details": {"progress": 100}})),
        ]))
    }
}

fn status_registry(streaming: bool) -> apcore::Registry {
    let registry = apcore::Registry::new();
    registry
        .register_module("post.status", Box::new(StatusModule { streaming }))
        .unwrap();
    registry
}

#[tokio::test]
async fn embedded_dispatch_returns_raw_result() {
    use apcore_cli::cli::exec_command;
    use apcore_cli::discovery::{ApCoreRegistryProvider, RegistryProvider};
    use apcore_cli::{build_module_command, dispatch_module_with_result};

    let provider = ApCoreRegistryProvider::new(status_registry(false));
    let executor = apcore::Executor::new(status_registry(false), apcore::Config::default());
    let descriptor = provider.get_module_descriptor("post.status").unwrap();
    let module_matches = build_module_command(&descriptor)
        .unwrap()
        .get_matches_from(["status", "--format", "json", "--fields", "status"]);
    let exec_matches =
        exec_command().get_matches_from(["exec", "post.status", "--format", "json", "--trace"]);

    for matches in [module_matches, exec_matches] {
        let result =
            dispatch_module_with_result("post.status", &matches, &provider, &executor, false)
                .await
                .unwrap();
        assert_eq!(
            result,
            Some(json!({"status": "embedded-result-ready", "details": {"progress": 100}})),
        );
    }

    let preflight = exec_command().get_matches_from(["exec", "post.status", "--dry-run"]);
    assert_eq!(
        dispatch_module_with_result("post.status", &preflight, &provider, &executor, false).await,
        Ok(None),
    );
}

#[tokio::test]
async fn embedded_dispatch_returns_exit_codes_to_the_host() {
    use apcore_cli::cli::exec_command;
    use apcore_cli::discovery::ApCoreRegistryProvider;
    use apcore_cli::{dispatch_module_with_result, EXIT_INVALID_INPUT, EXIT_MODULE_NOT_FOUND};

    let provider = ApCoreRegistryProvider::new(apcore::Registry::new());
    let executor = apcore::Executor::new(apcore::Registry::new(), apcore::Config::default());

    for (id, code) in [
        ("INVALID!", EXIT_INVALID_INPUT),
        ("missing.module", EXIT_MODULE_NOT_FOUND),
    ] {
        let matches = exec_command().get_matches_from(["exec", id]);
        assert_eq!(
            dispatch_module_with_result(id, &matches, &provider, &executor, false).await,
            Err(code),
        );
    }
}

#[tokio::test]
async fn embedded_dispatch_suppresses_stdout_and_preserves_terminal_trace() {
    use apcore_cli::cli::exec_command;
    use apcore_cli::discovery::ApCoreRegistryProvider;
    use apcore_cli::dispatch_module_with_result;

    const CHILD_MODE: &str = "APCORE_CLI_TEST_EMBEDDED_OUTPUT";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        let streaming = mode == "stream";
        let provider = ApCoreRegistryProvider::new(status_registry(streaming));
        let executor = apcore::Executor::new(status_registry(streaming), apcore::Config::default());
        if streaming {
            let matches = exec_command().get_matches_from(["exec", "post.status", "--stream"]);
            assert_eq!(
                dispatch_module_with_result("post.status", &matches, &provider, &executor, false)
                    .await,
                Ok(None),
            );
            return;
        }
        for flags in [
            &["--format", "json"][..],
            &["--trace", "--format", "json"][..],
            &["--trace", "--format", "table"][..],
            &["--stream"][..],
        ] {
            let matches = exec_command().get_matches_from(
                ["exec", "post.status"]
                    .into_iter()
                    .chain(flags.iter().copied()),
            );
            let result =
                dispatch_module_with_result("post.status", &matches, &provider, &executor, false)
                    .await
                    .unwrap();
            assert_eq!(
                result,
                Some(json!({"status": "embedded-result-ready", "details": {"progress": 100}})),
            );
        }
        return;
    }

    let run_child = |mode| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "embedded_dispatch_suppresses_stdout_and_preserves_terminal_trace",
                "--nocapture",
            ])
            .env(CHILD_MODE, mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };

    let quiet = run_child("quiet");
    let stdout = String::from_utf8(quiet.stdout).unwrap();
    assert!(!stdout.contains("embedded-result-ready"), "{stdout}");
    let stderr = String::from_utf8(quiet.stderr).unwrap();
    assert!(stderr.contains("Pipeline Trace"), "{stderr}");

    let stream = run_child("stream");
    let stdout = String::from_utf8(stream.stdout).unwrap();
    let chunks: Vec<Value> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert_eq!(
        chunks,
        [
            json!({"status": "embedded-stream-ready"}),
            json!({"details": {"progress": 100}}),
        ],
    );
}

#[tokio::test]
async fn embedded_dispatch_rejects_sandbox_before_constructing_dispatch_future() {
    use apcore_cli::cli::exec_command;
    use apcore_cli::discovery::ApCoreRegistryProvider;
    use apcore_cli::{dispatch_module_with_result, EXIT_INVALID_INPUT};

    let provider = ApCoreRegistryProvider::new(status_registry(false));
    let executor = apcore::Executor::new(status_registry(false), apcore::Config::default());
    for flags in [
        &["--sandbox"][..],
        &["--sandbox", "--trace"][..],
        &["--sandbox", "--stream"][..],
        &["--sandbox", "--strategy", "standard"][..],
    ] {
        let matches = exec_command().get_matches_from(
            ["exec", "post.status"]
                .into_iter()
                .chain(flags.iter().copied()),
        );
        assert_eq!(
            dispatch_module_with_result("post.status", &matches, &provider, &executor, false).await,
            Err(EXIT_INVALID_INPUT),
        );
    }

    let matches = exec_command().get_matches_from([
        "exec",
        "post.status",
        "--sandbox",
        "--trace",
        "--dry-run",
    ]);
    assert_eq!(
        dispatch_module_with_result("post.status", &matches, &provider, &executor, false).await,
        Ok(None),
    );
}
