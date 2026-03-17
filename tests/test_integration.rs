// apcore-cli — Integration tests: cross-component interactions.

mod common;

use apcore_cli::{
    config::ConfigResolver,
    output::{format_exec_result, format_module_list},
    ref_resolver::resolve_refs,
    schema_parser::schema_to_clap_args,
};
use serde_json::json;

#[test]
fn test_schema_to_args_then_format_result() {
    // Parse schema into args, then format an execution result.
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "description": "User name"}
        }
    });
    let schema_args = schema_to_clap_args(&schema).expect("schema_to_clap_args should succeed");
    assert!(!schema_args.args.is_empty(), "should produce at least one arg");

    // Simulate an execution result and format it.
    let result = json!({"greeting": "Hello, Alice!"});
    let output = format_exec_result(&result, "json");
    assert!(output.contains("greeting"), "JSON output should contain result key");
}

#[test]
fn test_config_resolver_feeds_extensions_dir() {
    // ConfigResolver must correctly resolve the extensions dir from defaults.
    let resolver = ConfigResolver::new(None, None);
    let ext_root = resolver.resolve("extensions.root", None, None);
    assert_eq!(
        ext_root,
        Some("./extensions".to_string()),
        "default extensions.root must be ./extensions"
    );
}

#[test]
fn test_resolve_refs_then_schema_to_clap_args() {
    // A schema with $refs must be resolvable before arg generation.
    let mut schema = json!({
        "$defs": {
            "MyString": {"type": "string"}
        },
        "type": "object",
        "properties": {
            "name": {"$ref": "#/$defs/MyString"}
        },
        "required": ["name"]
    });
    let resolved = resolve_refs(&mut schema, 10, "test.module");
    assert!(resolved.is_ok(), "resolve_refs should succeed");
    let resolved_schema = resolved.unwrap();

    // The resolved schema should have inlined the $ref.
    let name_prop = resolved_schema
        .get("properties")
        .and_then(|p| p.get("name"));
    assert!(name_prop.is_some(), "name property should exist after resolution");
    assert_eq!(
        name_prop.unwrap().get("type").and_then(|t| t.as_str()),
        Some("string"),
        "$ref should be inlined to type: string"
    );

    // Now generate clap args from the resolved schema.
    let schema_args = schema_to_clap_args(&resolved_schema)
        .expect("schema_to_clap_args should succeed on resolved schema");
    let arg_names: Vec<&str> = schema_args.args.iter()
        .filter_map(|a| a.get_long())
        .collect();
    assert!(arg_names.contains(&"name"), "should have --name arg from schema");
}

#[test]
fn test_format_module_list_empty() {
    // An empty module list must not panic and must return a valid string.
    let result = format_module_list(&[], "json", &[]);
    // JSON format of empty list should be "[]".
    assert_eq!(result.trim(), "[]", "empty module list in JSON should be []");
}
