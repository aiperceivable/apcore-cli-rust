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
/// * `schema`    — JSON Schema value (deep-copy is used internally)
/// * `max_depth` — maximum recursion depth before raising `MaxDepthExceeded`
/// * `module_id` — module identifier for error messages
///
/// # Errors
/// * `RefResolverError::Unresolvable` — unknown `$ref` target (exit 45)
/// * `RefResolverError::Circular`     — circular reference (exit 48)
/// * `RefResolverError::MaxDepthExceeded` — depth limit reached
pub fn resolve_refs(
    schema: &Value,
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
// Composition helpers
// ---------------------------------------------------------------------------

/// Merge all branches for allOf: union properties (later wins on conflict),
/// concatenate required arrays.
fn merge_allof(branches: Vec<Value>) -> Value {
    let mut merged_props = Map::new();
    let mut merged_required: Vec<Value> = Vec::new();

    for branch in branches {
        if let Some(props) = branch.get("properties").and_then(|v| v.as_object()) {
            for (k, v) in props {
                merged_props.insert(k.clone(), v.clone());
            }
        }
        if let Some(req) = branch.get("required").and_then(|v| v.as_array()) {
            merged_required.extend(req.iter().cloned());
        }
    }

    let mut result = Map::new();
    result.insert("properties".to_string(), Value::Object(merged_props));
    result.insert("required".to_string(), Value::Array(merged_required));
    Value::Object(result)
}

/// Compute the intersection of required field sets across branches.
fn intersect_required_sets(sets: Vec<HashSet<String>>) -> Vec<Value> {
    if sets.is_empty() {
        return Vec::new();
    }
    let mut iter = sets.into_iter();
    let first = iter.next().unwrap();
    iter.fold(first, |acc, set| acc.intersection(&set).cloned().collect())
        .into_iter()
        .map(Value::String)
        .collect()
}

/// Merge all branches for anyOf/oneOf: union properties, required = intersection.
fn merge_anyof(branches: Vec<Value>) -> Value {
    let mut merged_props = Map::new();
    let mut all_required_sets: Vec<HashSet<String>> = Vec::new();

    for branch in branches {
        if let Some(props) = branch.get("properties").and_then(|v| v.as_object()) {
            for (k, v) in props {
                merged_props.insert(k.clone(), v.clone());
            }
        }
        let set: HashSet<String> = branch
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        all_required_sets.push(set);
    }

    let intersection = intersect_required_sets(all_required_sets);

    let mut result = Map::new();
    result.insert("properties".to_string(), Value::Object(merged_props));
    result.insert("required".to_string(), Value::Array(intersection));
    Value::Object(result)
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
        let key = ref_path.split('/').next_back().unwrap_or("").to_string();

        let def = defs
            .get(&key)
            .cloned()
            .ok_or_else(|| RefResolverError::Unresolvable {
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

    // Handle allOf: merge properties (later wins), concatenate required.
    if obj.contains_key("allOf") {
        let sub_schemas = obj
            .get("allOf")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Resolve each branch first (handles nested $refs).
        let mut resolved_branches = Vec::with_capacity(sub_schemas.len());
        for sub in sub_schemas {
            let resolved_sub = resolve_node(sub, defs, depth + 1, max_depth, visiting, module_id)?;
            resolved_branches.push(resolved_sub);
        }

        let merged = merge_allof(resolved_branches);
        let merged_map = match merged {
            Value::Object(m) => m,
            _ => Map::new(),
        };

        // Carry over non-composition keys from the parent node.
        let mut result_map = merged_map;

        // Seed parent node's own `properties`/`required` into the merged result
        // AFTER branch merging — parent properties that are NOT already present
        // from any branch are inserted here. This matches Python behaviour where
        // `{properties:{x:...}, allOf:[{properties:{y:...}}]}` preserves both
        // x and y (branches win on conflict; parent fills gaps).
        if let Some(parent_props) = obj.get("properties").and_then(|v| v.as_object()) {
            if let Some(Value::Object(merged_props)) = result_map.get_mut("properties") {
                for (k, v) in parent_props {
                    merged_props.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        if let Some(parent_req) = obj.get("required").and_then(|v| v.as_array()) {
            if let Some(Value::Array(merged_req)) = result_map.get_mut("required") {
                for item in parent_req {
                    if !merged_req.contains(item) {
                        merged_req.push(item.clone());
                    }
                }
            }
        }

        for (k, v) in &obj {
            if k != "allOf" && !result_map.contains_key(k) {
                result_map.insert(k.clone(), v.clone());
            }
        }
        return Ok(Value::Object(result_map));
    }

    // Handle anyOf / oneOf (same merge logic, intersection of required).
    for keyword in &["anyOf", "oneOf"] {
        if obj.contains_key(*keyword) {
            let sub_schemas = obj
                .get(*keyword)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let mut resolved_branches = Vec::with_capacity(sub_schemas.len());
            for sub in sub_schemas {
                let resolved_sub =
                    resolve_node(sub, defs, depth + 1, max_depth, visiting, module_id)?;
                resolved_branches.push(resolved_sub);
            }

            let merged = merge_anyof(resolved_branches);
            let merged_map = match merged {
                Value::Object(m) => m,
                _ => Map::new(),
            };

            let mut result_map = merged_map;
            for (k, v) in &obj {
                if k != *keyword && !result_map.contains_key(k) {
                    result_map.insert(k.clone(), v.clone());
                }
            }
            return Ok(Value::Object(result_map));
        }
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
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        });
        let result = resolve_refs(&schema, 32, "test.module");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["name"]["type"], "string");
    }

    #[test]
    fn test_resolve_refs_simple_ref() {
        // A single $ref must be inlined from $defs.
        let schema = json!({
            "$defs": {
                "MyString": {"type": "string", "description": "A name"}
            },
            "type": "object",
            "properties": {
                "name": {"$ref": "#/$defs/MyString"}
            }
        });
        let result = resolve_refs(&schema, 32, "test.module");
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
        let schema = json!({
            "definitions": {
                "Addr": {"type": "string"}
            },
            "properties": {
                "city": {"$ref": "#/definitions/Addr"}
            }
        });
        let result = resolve_refs(&schema, 32, "test.module");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["city"]["type"], "string");
        assert!(resolved.get("definitions").is_none());
    }

    #[test]
    fn test_resolve_refs_unresolvable_returns_error() {
        // An unknown $ref must yield RefResolverError::Unresolvable.
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {"$ref": "#/$defs/DoesNotExist"}
            }
        });
        let result = resolve_refs(&schema, 32, "test.module");
        assert!(
            matches!(result, Err(RefResolverError::Unresolvable { .. })),
            "expected Unresolvable, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_refs_circular_returns_error() {
        // A circular $ref chain must yield RefResolverError::Circular or MaxDepthExceeded.
        let schema = json!({
            "$defs": {
                "A": {"$ref": "#/$defs/B"},
                "B": {"$ref": "#/$defs/A"}
            },
            "properties": {
                "x": {"$ref": "#/$defs/A"}
            }
        });
        let result = resolve_refs(&schema, 32, "test.module");
        assert!(
            matches!(
                result,
                Err(RefResolverError::Circular { .. })
                    | Err(RefResolverError::MaxDepthExceeded { .. })
            ),
            "expected Circular or MaxDepthExceeded, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_refs_max_depth_exceeded() {
        // max_depth=0 means the first $ref hit immediately fails.
        let schema = json!({
            "$defs": {
                "Inner": {"type": "string"}
            },
            "properties": {
                "x": {"$ref": "#/$defs/Inner"}
            }
        });
        let result = resolve_refs(&schema, 0, "test.module");
        assert!(
            matches!(result, Err(RefResolverError::MaxDepthExceeded { .. })),
            "expected MaxDepthExceeded, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_refs_nested_defs() {
        // $refs inside nested object properties must all be resolved.
        let schema = json!({
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
        let result = resolve_refs(&schema, 32, "test.module");
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
        let schema = json!({
            "$defs": {"T": {"type": "integer"}},
            "properties": {"x": {"$ref": "#/$defs/T"}}
        });
        let _ = resolve_refs(&schema, 32, "test.module");
        // Input schema still has $ref (not mutated).
        assert_eq!(schema["properties"]["x"]["$ref"], "#/$defs/T");
    }

    #[test]
    fn test_resolve_refs_sibling_refs_same_def() {
        // Two different properties referencing the same $def must both resolve correctly.
        let schema = json!({
            "$defs": {
                "Str": {"type": "string"}
            },
            "properties": {
                "a": {"$ref": "#/$defs/Str"},
                "b": {"$ref": "#/$defs/Str"}
            }
        });
        let result = resolve_refs(&schema, 32, "test.module");
        assert!(result.is_ok(), "sibling refs failed: {result:?}");
        let resolved = result.unwrap();
        assert_eq!(resolved["properties"]["a"]["type"], "string");
        assert_eq!(resolved["properties"]["b"]["type"], "string");
    }

    // --- Schema composition tests ---

    #[test]
    fn test_allof_merges_properties() {
        let schema = json!({
            "allOf": [
                {
                    "properties": {"a": {"type": "string"}},
                    "required": ["a"]
                },
                {
                    "properties": {"b": {"type": "integer"}},
                    "required": ["b"]
                }
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        assert_eq!(result["properties"]["a"]["type"], "string");
        assert_eq!(result["properties"]["b"]["type"], "integer");
        let required: Vec<&str> = result["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"a"));
        assert!(required.contains(&"b"));
    }

    #[test]
    fn test_allof_later_schema_wins_on_conflict() {
        let schema = json!({
            "allOf": [
                {"properties": {"x": {"type": "string"}}},
                {"properties": {"x": {"type": "integer"}}}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        // Later sub-schema wins: x must be integer.
        assert_eq!(result["properties"]["x"]["type"], "integer");
    }

    #[test]
    fn test_allof_copies_non_composition_keys() {
        let schema = json!({
            "description": "My type",
            "allOf": [
                {"properties": {"a": {"type": "string"}}}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        // "description" must survive in the merged result.
        assert_eq!(result["description"], "My type");
    }

    #[test]
    fn test_anyof_unions_properties() {
        let schema = json!({
            "anyOf": [
                {"properties": {"a": {"type": "string"}}, "required": ["a"]},
                {"properties": {"b": {"type": "integer"}}, "required": ["b"]}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        // Both properties must appear.
        assert!(result["properties"].get("a").is_some());
        assert!(result["properties"].get("b").is_some());
    }

    #[test]
    fn test_anyof_required_is_intersection() {
        let schema = json!({
            "anyOf": [
                {"properties": {"a": {"type": "string"}, "b": {"type": "string"}}, "required": ["a", "b"]},
                {"properties": {"a": {"type": "string"}, "c": {"type": "string"}}, "required": ["a", "c"]}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        let required: Vec<&str> = result["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // Only "a" appears in both branches — it is the intersection.
        assert!(
            required.contains(&"a"),
            "a must be required (in both branches)"
        );
        assert!(
            !required.contains(&"b"),
            "b must not be required (only in first branch)"
        );
        assert!(
            !required.contains(&"c"),
            "c must not be required (only in second branch)"
        );
    }

    #[test]
    fn test_anyof_empty_required_when_no_overlap() {
        let schema = json!({
            "anyOf": [
                {"properties": {"a": {"type": "string"}}, "required": ["a"]},
                {"properties": {"b": {"type": "integer"}}, "required": ["b"]}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        let required = result["required"].as_array().unwrap();
        assert!(
            required.is_empty(),
            "no fields are required in both branches"
        );
    }

    #[test]
    fn test_oneof_behaves_like_anyof() {
        let schema = json!({
            "oneOf": [
                {"properties": {"x": {"type": "string"}}, "required": ["x"]},
                {"properties": {"y": {"type": "integer"}}, "required": ["y"]}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        assert!(result["properties"].get("x").is_some());
        assert!(result["properties"].get("y").is_some());
        assert!(result["required"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_allof_with_nested_ref() {
        // allOf sub-schema that itself contains a $ref.
        let schema = json!({
            "$defs": {
                "Base": {"properties": {"id": {"type": "integer"}}, "required": ["id"]}
            },
            "allOf": [
                {"$ref": "#/$defs/Base"},
                {"properties": {"name": {"type": "string"}}}
            ]
        });
        let result = resolve_refs(&schema, 32, "mod").unwrap();
        assert_eq!(result["properties"]["id"]["type"], "integer");
        assert_eq!(result["properties"]["name"]["type"], "string");
        let required: Vec<&str> = result["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"id"));
    }
}
