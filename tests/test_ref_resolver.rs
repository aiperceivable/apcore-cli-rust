// apcore-cli — Integration tests for JSON Schema $ref inliner.
// Protocol spec: FE-08

mod common;

use apcore_cli::ref_resolver::{resolve_refs, RefResolverError};
use serde_json::json;

#[test]
fn test_resolve_refs_no_refs_returns_unchanged() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"}
        }
    });
    let resolved = resolve_refs(&schema, 10, "test.module");
    assert!(resolved.is_ok(), "schema with no $refs must resolve successfully");
    let resolved = resolved.unwrap();
    assert_eq!(
        resolved["properties"]["name"]["type"],
        json!("string"),
        "schema must remain unchanged"
    );
}

#[test]
fn test_resolve_refs_simple_inline() {
    let schema = json!({
        "$defs": {
            "MyString": {"type": "string", "description": "A name"}
        },
        "type": "object",
        "properties": {
            "name": {"$ref": "#/$defs/MyString"}
        }
    });
    let resolved = resolve_refs(&schema, 10, "test.module");
    assert!(resolved.is_ok(), "simple $ref must resolve successfully");
    let resolved = resolved.unwrap();
    assert_eq!(
        resolved["properties"]["name"]["type"],
        json!("string"),
        "inlined $ref must have type=string"
    );
}

#[test]
fn test_resolve_refs_unresolvable_returns_error() {
    let schema = json!({
        "type": "object",
        "properties": {
            "x": {"$ref": "#/$defs/DoesNotExist"}
        }
    });
    let result = resolve_refs(&schema, 10, "test.module");
    assert!(matches!(result, Err(RefResolverError::Unresolvable { .. })));
}

#[test]
fn test_resolve_refs_circular_returns_error() {
    // A → B → A must produce a Circular or MaxDepthExceeded error.
    let schema = json!({
        "$defs": {
            "A": {"$ref": "#/$defs/B"},
            "B": {"$ref": "#/$defs/A"}
        },
        "type": "object",
        "properties": {
            "x": {"$ref": "#/$defs/A"}
        }
    });
    let result = resolve_refs(&schema, 20, "test.module");
    assert!(
        matches!(
            result,
            Err(RefResolverError::Circular { .. }) | Err(RefResolverError::MaxDepthExceeded { .. })
        ),
        "circular $ref must produce Circular or MaxDepthExceeded error, got: {result:?}"
    );
}

#[test]
fn test_resolve_refs_max_depth_exceeded() {
    // max_depth=1 on a 2-level schema must return MaxDepthExceeded.
    let schema = json!({
        "$defs": {
            "Inner": {"type": "string"}
        },
        "type": "object",
        "properties": {
            "x": {"$ref": "#/$defs/Inner"}
        }
    });
    let result = resolve_refs(&schema, 0, "test.module");
    assert!(matches!(
        result,
        Err(RefResolverError::MaxDepthExceeded { .. })
    ));
}

#[test]
fn test_resolve_refs_nested_properties() {
    // $refs inside nested properties must all be resolved.
    let schema = json!({
        "$defs": {
            "Coord": {"type": "number", "description": "A coordinate"}
        },
        "type": "object",
        "properties": {
            "point": {
                "type": "object",
                "properties": {
                    "x": {"$ref": "#/$defs/Coord"},
                    "y": {"$ref": "#/$defs/Coord"}
                }
            }
        }
    });
    let resolved = resolve_refs(&schema, 10, "test.module");
    assert!(resolved.is_ok(), "nested $refs must resolve successfully");
    let resolved = resolved.unwrap();
    assert_eq!(
        resolved["properties"]["point"]["properties"]["x"]["type"],
        json!("number"),
        "nested x $ref must be inlined"
    );
    assert_eq!(
        resolved["properties"]["point"]["properties"]["y"]["type"],
        json!("number"),
        "nested y $ref must be inlined"
    );
}
