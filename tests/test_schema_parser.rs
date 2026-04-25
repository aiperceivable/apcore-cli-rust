// apcore-cli — Integration tests for JSON Schema → clap Arg translator.
// Protocol spec: FE-09

mod common;

use std::collections::HashMap;

use apcore_cli::schema_parser::{reconvert_enum_values, schema_to_clap_args};
use serde_json::{json, Value};

fn find_arg<'a>(args: &'a [clap::Arg], long: &str) -> Option<&'a clap::Arg> {
    args.iter().find(|a| a.get_long() == Some(long))
}

fn make_kwargs(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect()
}

#[test]
fn test_schema_to_clap_args_empty_schema() {
    let schema = json!({});
    let result = schema_to_clap_args(&schema).unwrap();
    assert!(result.args.is_empty(), "empty schema must produce no args");
    assert!(result.bool_pairs.is_empty());
    assert!(result.enum_maps.is_empty());
}

#[test]
fn test_schema_to_clap_args_string_property() {
    let schema = json!({
        "type": "object",
        "properties": {
            "text": {"type": "string", "description": "The input text"}
        },
        "required": []
    });
    let result = schema_to_clap_args(&schema).unwrap();
    assert_eq!(result.args.len(), 1);
    let arg = find_arg(&result.args, "text").expect("--text must exist");
    assert_eq!(arg.get_id(), "text");
    assert!(!arg.is_required_set());
}

#[test]
fn test_schema_to_clap_args_required_field_is_required() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "integer", "description": "First operand"}
        },
        "required": ["a"]
    });
    let result = schema_to_clap_args(&schema).unwrap();
    let arg = find_arg(&result.args, "a").expect("--a must exist");
    assert!(
        arg.is_required_set(),
        "required field must be marked required"
    );
}

#[test]
fn test_schema_to_clap_args_enum_field() {
    let schema = json!({
        "type": "object",
        "properties": {
            "mode": {"type": "string", "enum": ["fast", "slow"]}
        },
        "required": []
    });
    let result = schema_to_clap_args(&schema).unwrap();
    let arg = find_arg(&result.args, "mode").expect("--mode must exist");
    let pvs = arg.get_possible_values();
    let names: Vec<&str> = pvs.iter().map(|pv| pv.get_name()).collect();
    assert!(
        names.contains(&"fast"),
        "possible values must contain 'fast'"
    );
    assert!(
        names.contains(&"slow"),
        "possible values must contain 'slow'"
    );
}

#[test]
fn test_reconvert_enum_values_string_passthrough() {
    let schema = json!({
        "properties": {"output_type": {"type": "string", "enum": ["json", "csv"]}}
    });
    let schema_args = schema_to_clap_args(&schema).unwrap();
    let kwargs = make_kwargs(&[("output_type", "json")]);
    let result = reconvert_enum_values(kwargs, &schema_args);
    assert_eq!(result["output_type"], Value::String("json".to_string()));
}

#[test]
fn test_reconvert_enum_values_integer_coercion() {
    let schema = json!({
        "properties": {"level": {"type": "integer", "enum": [1, 2, 3]}}
    });
    let schema_args = schema_to_clap_args(&schema).unwrap();
    let kwargs = make_kwargs(&[("level", "2")]);
    let result = reconvert_enum_values(kwargs, &schema_args);
    assert!(
        result["level"].is_number(),
        "integer enum must be a JSON number"
    );
    assert_eq!(result["level"], json!(2));
}

#[test]
fn test_reconvert_enum_values_boolean_coercion() {
    let schema = json!({
        "properties": {"strict": {"type": "string", "enum": [true, false]}}
    });
    let schema_args = schema_to_clap_args(&schema).unwrap();
    let kwargs = make_kwargs(&[("strict", "true")]);
    let result = reconvert_enum_values(kwargs, &schema_args);
    assert_eq!(result["strict"], Value::Bool(true));
}

// --- Full pipeline integration tests ---

#[test]
fn test_full_pipeline_integer_enum_roundtrip() {
    let schema = json!({
        "properties": {
            "level": {"type": "integer", "enum": [1, 2, 3]}
        }
    });
    let schema_args = schema_to_clap_args(&schema).unwrap();

    let cmd = schema_args
        .args
        .iter()
        .cloned()
        .fold(clap::Command::new("test"), |c, a| c.arg(a));
    let matches = cmd.try_get_matches_from(["test", "--level", "2"]).unwrap();

    let raw_val = matches.get_one::<String>("level").cloned().unwrap();
    let mut kwargs = HashMap::new();
    kwargs.insert("level".to_string(), Value::String(raw_val));

    let result = reconvert_enum_values(kwargs, &schema_args);
    assert_eq!(result["level"], json!(2));
    assert!(result["level"].is_number());
}

#[test]
fn test_full_pipeline_boolean_flag_pair() {
    let schema = json!({
        "properties": {"log_output": {"type": "boolean"}}
    });
    let schema_args = schema_to_clap_args(&schema).unwrap();

    let cmd = schema_args
        .args
        .iter()
        .cloned()
        .fold(clap::Command::new("test"), |c, a| c.arg(a));

    let matches = cmd
        .clone()
        .try_get_matches_from(["test", "--log-output"])
        .unwrap();
    assert!(
        matches.get_flag("log_output"),
        "--log-output must set log_output=true"
    );

    // --no-log-output can be parsed without error; log_output stays false.
    let matches2 = cmd
        .try_get_matches_from(["test", "--no-log-output"])
        .unwrap();
    assert!(
        !matches2.get_flag("log_output"),
        "--no-log-output must leave log_output unset"
    );
}
