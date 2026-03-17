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
    // Parse schema into args, then format the execution result.
    // TODO: build schema, call schema_to_clap_args, run exec, format_exec_result.
    assert!(false, "not implemented");
}

#[test]
fn test_config_resolver_feeds_extensions_dir() {
    // ConfigResolver must correctly feed the extensions dir to the CLI.
    // TODO: create resolver with known config, verify extensions.root resolves.
    assert!(false, "not implemented");
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
    // TODO: assert resolved is Ok, then call schema_to_clap_args on result.
    assert!(false, "not implemented");
}

#[test]
fn test_format_module_list_empty() {
    // An empty module list must not panic and must return a valid string.
    let result = format_module_list(&[], "json", &[]);
    // TODO: assert result is valid JSON array "[]".
    assert!(false, "not implemented");
}
