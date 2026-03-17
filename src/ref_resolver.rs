// apcore-cli — JSON Schema $ref inliner.
// Protocol spec: FE-08 (resolve_refs)

use serde_json::{Map, Value};
use std::collections::HashSet;
use thiserror::Error;

/// Maximum recursion depth for $ref resolution.
pub const MAX_REF_DEPTH: usize = 32;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors produced during `$ref` resolution.
#[derive(Debug, Error)]
pub enum RefResolverError {
    /// A `$ref` target could not be found in the schema's `$defs`.
    #[error("unresolvable $ref '{reference}' in module '{module_id}' (exit 45)")]
    Unresolvable {
        reference: String,
        module_id: String,
    },

    /// A circular reference chain was detected (exit 48).
    #[error("circular $ref detected in module '{module_id}' (exit 48)")]
    Circular { module_id: String },

    /// The maximum recursion depth was exceeded.
    #[error("$ref resolution exceeded max depth {max_depth} in module '{module_id}'")]
    MaxDepthExceeded { max_depth: usize, module_id: String },
}

// ---------------------------------------------------------------------------
// resolve_refs
// ---------------------------------------------------------------------------

/// Inline all `$ref` pointers in a JSON Schema value.
///
/// Resolves `$ref` values by looking them up in `schema["$defs"]` and
/// substituting the referenced schema in-place. Handles nested schemas
/// recursively up to `max_depth`.
///
/// # Arguments
/// * `schema`    — mutable JSON Schema value (not mutated; deep-copy is used internally)
/// * `max_depth` — maximum recursion depth before raising `MaxDepthExceeded`
/// * `module_id` — module identifier for error messages
///
/// # Errors
/// * `RefResolverError::Unresolvable` — unknown `$ref` target (exit 45)
/// * `RefResolverError::Circular`     — circular reference (exit 48)
/// * `RefResolverError::MaxDepthExceeded` — depth limit reached
pub fn resolve_refs(
    schema: &mut Value,
    max_depth: usize,
    module_id: &str,
) -> Result<Value, RefResolverError> {
    // Deep-copy; do not modify the caller's value.
    let copy = schema.clone();

    // Extract $defs / definitions ($defs takes precedence).
    let defs: Map<String, Value> = copy
        .get("$defs")
        .or_else(|| copy.get("definitions"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut visiting: HashSet<String> = HashSet::new();
    let resolved = resolve_node(copy, &defs, 0, max_depth, &mut visiting, module_id)?;

    // Strip definition keys from result.
    let mut result = resolved;
    if let Some(obj) = result.as_object_mut() {
        obj.remove("$defs");
        obj.remove("definitions");
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// resolve_node (private helper)
// ---------------------------------------------------------------------------

fn resolve_node(
    node: Value,
    defs: &Map<String, Value>,
    depth: usize,
    max_depth: usize,
    visiting: &mut HashSet<String>,
    module_id: &str,
) -> Result<Value, RefResolverError> {
    let obj = match node {
        Value::Object(map) => map,
        other => return Ok(other),
    };

    // Handle $ref substitution.
    if let Some(ref_val) = obj.get("$ref") {
        let ref_path = ref_val.as_str().unwrap_or("").to_string();

        if depth >= max_depth {
            return Err(RefResolverError::MaxDepthExceeded {
                max_depth,
                module_id: module_id.to_string(),
            });
        }

        if visiting.contains(&ref_path) {
            return Err(RefResolverError::Circular {
                module_id: module_id.to_string(),
            });
        }

        // Extract key: "#/$defs/Address" → "Address"
        let key = ref_path.split('/').last().unwrap_or("").to_string();

        let def = defs.get(&key).cloned().ok_or_else(|| RefResolverError::Unresolvable {
            reference: ref_path.clone(),
            module_id: module_id.to_string(),
        })?;

        visiting.insert(ref_path.clone());
        let result = resolve_node(def, defs, depth + 1, max_depth, visiting, module_id)?;
        // Keep ref_path in visiting for the duration of this chain to detect cycles.
        // It remains in visiting intentionally — siblings go through a fresh chain
        // because we only remove entries when unwinding past the insertion point.
        // However, for sibling $refs (two different properties referencing the same def),
        // we must remove the entry after resolving so they don't block each other.
        visiting.remove(&ref_path);
        return Ok(result);
    }

    // Recursively resolve all values in the object map.
    let mut resolved_map = Map::with_capacity(obj.len());
    for (k, v) in obj {
        let resolved_v = resolve_node(v, defs, depth, max_depth, visiting, module_id)?;
        resolved_map.insert(k, resolved_v);
    }

    Ok(Value::Object(resolved_map))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_resolve_refs_no_refs_unchanged() {
        // A schema without any $ref must be returned unchanged.
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["name"]["type"], "string");
    }

    #[test]
    fn test_resolve_refs_simple_ref() {
        // A single $ref must be inlined from $defs.
        let mut schema = json!({
            "$defs": {
                "MyString": {"type": "string", "description": "A name"}
            },
            "type": "object",
            "properties": {
                "name": {"$ref": "#/$defs/MyString"}
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["name"]["type"], "string");
        assert_eq!(resolved["properties"]["name"]["description"], "A name");
        // $defs must be stripped from result.
        assert!(resolved.get("$defs").is_none());
    }

    #[test]
    fn test_resolve_refs_definitions_key_also_supported() {
        // Some schemas use "definitions" instead of "$defs".
        let mut schema = json!({
            "definitions": {
                "Addr": {"type": "string"}
            },
            "properties": {
                "city": {"$ref": "#/definitions/Addr"}
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["city"]["type"], "string");
        assert!(resolved.get("definitions").is_none());
    }

    #[test]
    fn test_resolve_refs_unresolvable_returns_error() {
        // An unknown $ref must yield RefResolverError::Unresolvable.
        let mut schema = json!({
            "type": "object",
            "properties": {
                "x": {"$ref": "#/$defs/DoesNotExist"}
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(
            matches!(result, Err(RefResolverError::Unresolvable { .. })),
            "expected Unresolvable, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_refs_circular_returns_error() {
        // A circular $ref chain must yield RefResolverError::Circular or MaxDepthExceeded.
        let mut schema = json!({
            "$defs": {
                "A": {"$ref": "#/$defs/B"},
                "B": {"$ref": "#/$defs/A"}
            },
            "properties": {
                "x": {"$ref": "#/$defs/A"}
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(
            matches!(
                result,
                Err(RefResolverError::Circular { .. }) | Err(RefResolverError::MaxDepthExceeded { .. })
            ),
            "expected Circular or MaxDepthExceeded, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_refs_max_depth_exceeded() {
        // max_depth=0 means the first $ref hit immediately fails.
        let mut schema = json!({
            "$defs": {
                "Inner": {"type": "string"}
            },
            "properties": {
                "x": {"$ref": "#/$defs/Inner"}
            }
        });
        let result = resolve_refs(&mut schema, 0, "test.module");
        assert!(
            matches!(result, Err(RefResolverError::MaxDepthExceeded { .. })),
            "expected MaxDepthExceeded, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_refs_nested_defs() {
        // $refs inside nested object properties must all be resolved.
        let mut schema = json!({
            "$defs": {
                "City": {"type": "string"}
            },
            "properties": {
                "address": {
                    "type": "object",
                    "properties": {
                        "city": {"$ref": "#/$defs/City"}
                    }
                }
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(
            resolved["properties"]["address"]["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn test_resolve_refs_does_not_mutate_input() {
        // The original schema must not be modified.
        let original = json!({
            "$defs": {"T": {"type": "integer"}},
            "properties": {"x": {"$ref": "#/$defs/T"}}
        });
        let mut schema = original.clone();
        let _ = resolve_refs(&mut schema, 32, "test.module");
        // Input schema still has $ref (not mutated).
        assert_eq!(schema["properties"]["x"]["$ref"], "#/$defs/T");
    }

    #[test]
    fn test_resolve_refs_sibling_refs_same_def() {
        // Two different properties referencing the same $def must both resolve correctly.
        let mut schema = json!({
            "$defs": {
                "Str": {"type": "string"}
            },
            "properties": {
                "a": {"$ref": "#/$defs/Str"},
                "b": {"$ref": "#/$defs/Str"}
            }
        });
        let result = resolve_refs(&mut schema, 32, "test.module");
        assert!(result.is_ok(), "sibling refs failed: {result:?}");
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["a"]["type"], "string");
        assert_eq!(resolved["properties"]["b"]["type"], "string");
    }
}
