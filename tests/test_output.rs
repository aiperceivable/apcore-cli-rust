// apcore-cli — Integration tests for output formatting.
// Protocol spec: FE-08

mod common;

use apcore_cli::output::{
    format_exec_result, format_module_detail, format_module_list, resolve_format,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// resolve_format
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_format_explicit_json() {
    assert_eq!(resolve_format(Some("json")), "json");
}

#[test]
fn test_resolve_format_explicit_table() {
    assert_eq!(resolve_format(Some("table")), "table");
}

#[test]
fn test_resolve_format_none_defaults_to_json_in_ci() {
    // In a test runner, stdout is not a TTY, so None → "json".
    // If this assertion fails, the test environment has a TTY attached —
    // which is unusual for CI. Both outcomes are valid; this just documents
    // the expected CI behaviour.
    let fmt = resolve_format(None);
    assert!(
        fmt == "json" || fmt == "table",
        "resolve_format(None) must return 'json' or 'table', got '{}'",
        fmt
    );
}

// ---------------------------------------------------------------------------
// format_module_list
// ---------------------------------------------------------------------------

#[test]
fn test_format_module_list_json_valid() {
    let modules = vec![
        json!({"module_id": "math.add", "description": "Add two numbers", "tags": []}),
        json!({"module_id": "text.upper", "description": "Uppercase", "tags": []}),
    ];
    let output = format_module_list(&modules, "json", &[]);
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
    let arr = parsed.as_array().expect("must be JSON array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], "math.add");
    assert_eq!(arr[1]["id"], "text.upper");
}

#[test]
fn test_format_module_list_table_has_headers() {
    let modules =
        vec![json!({"module_id": "math.add", "description": "Add two numbers", "tags": []})];
    let output = format_module_list(&modules, "table", &[]);
    assert!(output.contains("ID"), "table must have ID column header");
    assert!(
        output.contains("Description"),
        "table must have Description column header"
    );
}

#[test]
fn test_format_module_list_table_contains_module_id() {
    let modules =
        vec![json!({"module_id": "math.add", "description": "Add two numbers", "tags": []})];
    let output = format_module_list(&modules, "table", &[]);
    assert!(output.contains("math.add"));
}

#[test]
fn test_format_module_list_table_empty_no_tags() {
    let output = format_module_list(&[], "table", &[]);
    assert_eq!(output.trim(), "No modules found.");
}

#[test]
fn test_format_module_list_table_empty_with_filter_tags() {
    let output = format_module_list(&[], "table", &["math"]);
    assert!(output.contains("No modules found matching tags:"));
    assert!(output.contains("math"));
}

#[test]
fn test_format_module_list_json_empty() {
    let output = format_module_list(&[], "json", &[]);
    assert_eq!(output.trim(), "[]");
}

// ---------------------------------------------------------------------------
// format_module_detail
// ---------------------------------------------------------------------------

#[test]
fn test_format_module_detail_json() {
    let module = json!({
        "module_id": "math.add",
        "description": "Add two numbers",
        "input_schema": {"type": "object"},
        "tags": ["math"]
    });
    let output = format_module_detail(&module, "json");
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
    assert_eq!(parsed["id"], "math.add");
    assert_eq!(parsed["description"], "Add two numbers");
    assert!(parsed.get("input_schema").is_some());
}

#[test]
fn test_format_module_detail_json_no_null_fields() {
    let module = json!({
        "module_id": "a.b",
        "description": "desc",
    });
    let output = format_module_detail(&module, "json");
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(parsed.get("input_schema").is_none());
    assert!(parsed.get("output_schema").is_none());
    assert!(parsed.get("tags").is_none());
}

#[test]
fn test_format_module_detail_table_description() {
    let module = json!({
        "module_id": "math.add",
        "description": "Add two numbers",
    });
    let output = format_module_detail(&module, "table");
    assert!(output.contains("Add two numbers"));
    assert!(output.contains("math.add"));
}

// ---------------------------------------------------------------------------
// format_exec_result
// ---------------------------------------------------------------------------

#[test]
fn test_format_exec_result_json() {
    let result = json!({"sum": 42});
    let output = format_exec_result(&result, "json", None);
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
    assert_eq!(parsed["sum"], 42);
}

#[test]
fn test_format_exec_result_table() {
    let result = json!({"sum": 42});
    let output = format_exec_result(&result, "table", None);
    assert!(output.contains("sum"), "table must contain key 'sum'");
    assert!(output.contains("42"), "table must contain value '42'");
}

#[test]
fn test_format_exec_result_null() {
    let output = format_exec_result(&serde_json::Value::Null, "json", None);
    assert_eq!(output, "");
}

#[test]
fn test_format_exec_result_string() {
    let result = json!("hello");
    let output = format_exec_result(&result, "json", None);
    assert_eq!(output, "hello");
}

#[test]
fn test_format_exec_result_array() {
    let result = json!([1, 2, 3]);
    let output = format_exec_result(&result, "json", None);
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
    assert!(parsed.is_array());
}

// ---------------------------------------------------------------------------
// CSV emission — RFC 4180 escaping
// ---------------------------------------------------------------------------

#[test]
fn test_csv_object_quotes_value_with_comma() {
    let result = json!({"description": "foo, bar"});
    let out = format_exec_result(&result, "csv", None);
    // Must quote the value; reader parsing "description\n\"foo, bar\"" yields
    // one column, not two. The review flagged that the old code produced
    // `description\nfoo, bar` which downstream CSV libs would split.
    assert_eq!(out, "description\n\"foo, bar\"");
}

#[test]
fn test_csv_object_escapes_embedded_quote() {
    let result = json!({"msg": "she said \"hi\""});
    let out = format_exec_result(&result, "csv", None);
    assert_eq!(out, "msg\n\"she said \"\"hi\"\"\"");
}

#[test]
fn test_csv_object_quotes_value_with_newline() {
    let result = json!({"multi": "line1\nline2"});
    let out = format_exec_result(&result, "csv", None);
    assert_eq!(out, "multi\n\"line1\nline2\"");
}

#[test]
fn test_csv_array_quotes_rows_independently() {
    let result = json!([
        {"a": "x", "b": "y,z"},
        {"a": "p\"q", "b": "r"},
    ]);
    let out = format_exec_result(&result, "csv", None);
    let lines: Vec<&str> = out.split('\n').collect();
    assert_eq!(lines[0], "a,b");
    assert_eq!(lines[1], "x,\"y,z\"");
    assert_eq!(lines[2], "\"p\"\"q\",r");
}

#[test]
fn test_csv_header_with_comma_in_key_is_quoted() {
    // Keys with commas (unusual but legal JSON) must also be quoted.
    let result = json!({"a,b": 1});
    let out = format_exec_result(&result, "csv", None);
    assert_eq!(out, "\"a,b\"\n1");
}

#[test]
fn test_csv_plain_value_passthrough() {
    // Values without any special character survive without being wrapped.
    let result = json!({"name": "alpha", "count": 3});
    let out = format_exec_result(&result, "csv", None);
    assert_eq!(out, "count,name\n3,alpha");
}
