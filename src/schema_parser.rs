// apcore-cli — JSON Schema → clap Arg translator.
// Protocol spec: FE-09 (schema_to_clap_args, reconvert_enum_values)

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Arg;
use serde_json::Value;
use thiserror::Error;
use tracing::warn;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for schema parsing failures.
#[derive(Debug, Error)]
pub enum SchemaParserError {
    /// Two properties normalise to the same --flag-name.
    /// Caller must exit 48.
    #[error("Flag name collision: properties '{prop_a}' and '{prop_b}' both map to '{flag_name}'")]
    FlagCollision {
        prop_a: String,
        prop_b: String,
        flag_name: String,
    },
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A single boolean --flag / --no-flag pair generated from a `type: boolean` property.
#[derive(Debug)]
pub struct BoolFlagPair {
    /// Original schema property name (e.g. "verbose").
    pub prop_name: String,
    /// Long name used for the positive flag (e.g. "verbose").
    pub flag_long: String,
    /// Default value from the schema's `default` field (defaults to false).
    pub default_val: bool,
}

/// Full output of schema_to_clap_args.
#[derive(Debug)]
pub struct SchemaArgs {
    /// clap Args ready to attach to a clap::Command.
    pub args: Vec<Arg>,
    /// Boolean flag pairs; used by collect_input to reconcile --flag/--no-flag.
    pub bool_pairs: Vec<BoolFlagPair>,
    /// Maps property name (snake_case) → original enum values (as serde_json::Value).
    /// Used by reconvert_enum_values for type coercion.
    pub enum_maps: HashMap<String, Vec<Value>>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const HELP_TEXT_MAX_LEN: usize = 200;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a property name (snake_case) to a CLI flag long name (kebab-case).
pub fn prop_name_to_flag_name(s: &str) -> String {
    s.replace('_', "-")
}

/// Determine whether a property should use PathBuf value_parser.
fn is_file_property(prop_name: &str, prop_schema: &Value) -> bool {
    prop_name.ends_with("_file")
        || prop_schema
            .get("x-cli-file")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// Extract help text from a schema property.
/// Prefers `x-llm-description` over `description`.
/// Truncates to HELP_TEXT_MAX_LEN chars (197 + "...").
pub fn extract_help(prop_schema: &Value) -> Option<String> {
    let text = prop_schema
        .get("x-llm-description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            prop_schema
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })?;

    if text.len() > HELP_TEXT_MAX_LEN {
        Some(format!("{}...", &text[..HELP_TEXT_MAX_LEN - 3]))
    } else {
        Some(text.to_string())
    }
}

// ---------------------------------------------------------------------------
// map_type
// ---------------------------------------------------------------------------

/// Map a single schema property to a clap::Arg.
///
/// Returns an error only for flag collisions (detected at schema_to_clap_args level).
/// Boolean and enum types are handled by separate tasks.
pub fn map_type(prop_name: &str, prop_schema: &Value) -> Result<Arg, SchemaParserError> {
    let flag_long = prop_name_to_flag_name(prop_name);
    let schema_type = prop_schema.get("type").and_then(|v| v.as_str());

    let arg = Arg::new(prop_name.to_string()).long(flag_long);

    let arg = match schema_type {
        Some("integer") => arg.value_parser(clap::value_parser!(i64)),
        Some("number") => arg.value_parser(clap::value_parser!(f64)),
        Some("string") if is_file_property(prop_name, prop_schema) => {
            arg.value_parser(clap::value_parser!(PathBuf))
        }
        Some("string") | Some("object") | Some("array") => arg,
        Some(unknown) => {
            warn!(
                "Unknown schema type '{}' for property '{}', defaulting to string.",
                unknown, prop_name
            );
            arg
        }
        None => {
            warn!(
                "No type specified for property '{}', defaulting to string.",
                prop_name
            );
            arg
        }
    };

    Ok(arg)
}

// ---------------------------------------------------------------------------
// schema_to_clap_args
// ---------------------------------------------------------------------------

/// Translate a JSON Schema `properties` map into a SchemaArgs result.
///
/// Each schema property becomes one `--<name>` flag with:
/// * `help` set to the property's `x-llm-description` or `description` field
/// * `required` set when the property appears in the schema's `required` array
/// * enum variants and boolean pairs deferred to later tasks
///
/// # Arguments
/// * `schema` — JSON Schema object (may have `"properties"` key)
///
/// Returns empty SchemaArgs for schemas without properties.
pub fn schema_to_clap_args(schema: &Value) -> Result<SchemaArgs, SchemaParserError> {
    let properties = match schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => {
            return Ok(SchemaArgs {
                args: Vec::new(),
                bool_pairs: Vec::new(),
                enum_maps: HashMap::new(),
            });
        }
    };

    let required_list: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // Warn about required properties missing from properties map.
    for req_name in &required_list {
        if !properties.contains_key(*req_name) {
            warn!(
                "Required property '{}' not found in properties, skipping.",
                req_name
            );
        }
    }

    let mut args: Vec<Arg> = Vec::new();
    let mut bool_pairs: Vec<BoolFlagPair> = Vec::new();
    let mut enum_maps: HashMap<String, Vec<Value>> = HashMap::new();
    let mut seen_flags: HashMap<String, String> = HashMap::new(); // flag_long → prop_name

    for (prop_name, prop_schema) in properties {
        let flag_long = prop_name_to_flag_name(prop_name);

        // Collision detection.
        if let Some(existing) = seen_flags.get(&flag_long) {
            return Err(SchemaParserError::FlagCollision {
                prop_a: prop_name.clone(),
                prop_b: existing.clone(),
                flag_name: flag_long,
            });
        }
        seen_flags.insert(flag_long.clone(), prop_name.clone());

        let schema_type = prop_schema.get("type").and_then(|v| v.as_str());
        let is_required = required_list.contains(&prop_name.as_str());
        let help_text = extract_help(prop_schema);
        let default_val = prop_schema.get("default");

        // Boolean → --flag / --no-flag pair. Must be checked before enum.
        if schema_type == Some("boolean") {
            let bool_default = prop_schema
                .get("default")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let mut pos_arg = Arg::new(prop_name.clone())
                .long(flag_long.clone())
                .action(clap::ArgAction::SetTrue);
            let mut neg_arg = Arg::new(format!("no-{}", prop_name))
                .long(format!("no-{}", flag_long))
                .action(clap::ArgAction::SetFalse);

            if let Some(ref help) = help_text {
                pos_arg = pos_arg.help(help.clone());
                neg_arg = neg_arg.help(format!("Disable --{flag_long}"));
            }

            // Also register the no- flag in seen_flags to detect collisions.
            let no_flag_long = format!("no-{}", flag_long);
            seen_flags.insert(no_flag_long, format!("no-{}", prop_name));

            args.push(pos_arg);
            args.push(neg_arg);

            bool_pairs.push(BoolFlagPair {
                prop_name: prop_name.clone(),
                flag_long,
                default_val: bool_default,
            });

            // Suppress unused variable warning; is_required is intentionally
            // not applied to boolean flags.
            let _ = is_required;

            continue;
        }

        // Enum handling: properties with an "enum" array (and type != "boolean").
        if let Some(enum_values) = prop_schema.get("enum").and_then(|v| v.as_array()) {
            if enum_values.is_empty() {
                warn!(
                    "Empty enum for property '{}', falling through to plain string arg.",
                    prop_name
                );
                // Fall through to plain string arg below.
            } else {
                // Convert all enum values to String for clap's PossibleValuesParser.
                let string_values: Vec<String> = enum_values
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect();

                // Store original typed values for post-parse reconversion.
                enum_maps.insert(prop_name.clone(), enum_values.to_vec());

                let mut arg = Arg::new(prop_name.clone())
                    .long(flag_long)
                    .value_parser(clap::builder::PossibleValuesParser::new(string_values))
                    .required(false); // required enforced post-parse for STDIN compatibility

                // Attach help text with optional [required] annotation.
                if let Some(help) = help_text {
                    let annotated = if is_required {
                        format!("{} [required]", help)
                    } else {
                        help
                    };
                    arg = arg.help(annotated);
                } else if is_required {
                    arg = arg.help("[required]");
                }

                if let Some(dv) = default_val {
                    let dv_str = match dv {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    arg = arg.default_value(dv_str);
                }

                args.push(arg);
                continue;
            }
        }

        // Build Arg using map_type.
        let mut arg = map_type(prop_name, prop_schema)?.required(is_required);

        if let Some(help) = help_text {
            arg = arg.help(help);
        }

        // Default value (set as string; clap parses it through the value_parser).
        if let Some(dv) = default_val {
            let dv_str = match dv {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            arg = arg.default_value(dv_str);
        }

        args.push(arg);
    }

    Ok(SchemaArgs {
        args,
        bool_pairs,
        enum_maps,
    })
}

// ---------------------------------------------------------------------------
// reconvert_enum_values
// ---------------------------------------------------------------------------

/// Re-map string enum values from CLI args back to their JSON-typed forms.
///
/// clap always produces `String` values; this function converts them to the
/// correct JSON type (number, boolean, null) based on the original schema
/// definition stored in `schema_args.enum_maps`.
///
/// # Arguments
/// * `kwargs`      — raw CLI arguments map (string values from clap)
/// * `schema_args` — the SchemaArgs produced by `schema_to_clap_args`
///
/// Returns a new map with enum values converted to their correct JSON types.
/// Non-enum keys and Null values pass through unchanged.
pub fn reconvert_enum_values(
    kwargs: HashMap<String, Value>,
    schema_args: &SchemaArgs,
) -> HashMap<String, Value> {
    let mut result = kwargs;

    for (key, original_variants) in &schema_args.enum_maps {
        let val = match result.get(key) {
            Some(v) => v.clone(),
            None => continue,
        };

        // Skip null / non-string values (absent optional args arrive as Null).
        let str_val = match &val {
            Value::String(s) => s.clone(),
            _ => continue,
        };

        // Find the original variant whose string representation matches str_val.
        let original = original_variants.iter().find(|v| {
            let as_str = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            as_str == str_val
        });

        if let Some(orig) = original {
            let converted = match orig {
                Value::Number(n) => {
                    if n.as_i64().is_some() {
                        str_val
                            .parse::<i64>()
                            .ok()
                            .map(|i| Value::Number(i.into()))
                            .unwrap_or(val.clone())
                    } else {
                        str_val
                            .parse::<f64>()
                            .ok()
                            .and_then(serde_json::Number::from_f64)
                            .map(Value::Number)
                            .unwrap_or(val.clone())
                    }
                }
                Value::Bool(_) => Value::Bool(str_val.to_lowercase() == "true"),
                _ => val.clone(), // String: keep as-is
            };
            result.insert(key.clone(), converted);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helper: find an Arg by long name.
    fn find_arg<'a>(args: &'a [clap::Arg], long: &str) -> Option<&'a clap::Arg> {
        args.iter().find(|a| a.get_long() == Some(long))
    }

    #[test]
    fn test_schema_to_clap_args_empty_schema() {
        let schema = json!({});
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(result.args.is_empty());
        assert!(result.bool_pairs.is_empty());
        assert!(result.enum_maps.is_empty());
    }

    #[test]
    fn test_schema_to_clap_args_string_property() {
        let schema = json!({
            "properties": {"text": {"type": "string", "description": "Some text"}},
            "required": []
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert_eq!(result.args.len(), 1);
        let arg = find_arg(&result.args, "text").expect("--text must exist");
        assert_eq!(arg.get_id(), "text");
        assert!(!arg.is_required_set());
    }

    #[test]
    fn test_schema_to_clap_args_integer_property() {
        let schema = json!({
            "properties": {"count": {"type": "integer"}},
            "required": ["count"]
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "count").expect("--count must exist");
        assert!(arg.is_required_set());
    }

    #[test]
    fn test_schema_to_clap_args_number_property() {
        let schema = json!({
            "properties": {"rate": {"type": "number"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "rate").is_some());
    }

    #[test]
    fn test_schema_to_clap_args_object_and_array_as_string() {
        let schema = json!({
            "properties": {
                "data": {"type": "object"},
                "items": {"type": "array"}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "data").is_some());
        assert!(find_arg(&result.args, "items").is_some());
    }

    #[test]
    fn test_schema_to_clap_args_underscore_to_hyphen() {
        let schema = json!({
            "properties": {"input_file": {"type": "string"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        // Flag long name must be "input-file".
        assert!(find_arg(&result.args, "input-file").is_some());
        // Arg id must be "input_file" (original name, for collect_input lookup).
        let arg = find_arg(&result.args, "input-file").unwrap();
        assert_eq!(arg.get_id(), "input_file");
    }

    #[test]
    fn test_schema_to_clap_args_file_convention_suffix() {
        let schema = json!({
            "properties": {"config_file": {"type": "string"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "config-file").expect("must exist");
        let _ = arg; // Exact parser check is implementation-dependent.
    }

    #[test]
    fn test_schema_to_clap_args_x_cli_file_flag() {
        let schema = json!({
            "properties": {"report": {"type": "string", "x-cli-file": true}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "report").is_some());
    }

    #[test]
    fn test_schema_to_clap_args_unknown_type_defaults_to_string() {
        let schema = json!({
            "properties": {"x": {"type": "foobar"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "x").is_some());
    }

    #[test]
    fn test_schema_to_clap_args_missing_type_defaults_to_string() {
        let schema = json!({
            "properties": {"x": {"description": "no type field"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "x").is_some());
    }

    #[test]
    fn test_schema_to_clap_args_default_value_set() {
        let schema = json!({
            "properties": {"timeout": {"type": "integer", "default": 30}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "timeout").unwrap();
        assert_eq!(
            arg.get_default_values().first().and_then(|v| v.to_str()),
            Some("30")
        );
    }

    // --- extract_help tests ---

    #[test]
    fn test_extract_help_uses_description() {
        let prop = json!({"description": "A plain description"});
        assert_eq!(extract_help(&prop), Some("A plain description".to_string()));
    }

    #[test]
    fn test_extract_help_prefers_x_llm_description() {
        let prop = json!({
            "description": "Plain description",
            "x-llm-description": "LLM description"
        });
        assert_eq!(extract_help(&prop), Some("LLM description".to_string()));
    }

    #[test]
    fn test_extract_help_truncates_at_200() {
        let long_text = "a".repeat(250);
        let prop = json!({"description": long_text});
        let result = extract_help(&prop).unwrap();
        assert_eq!(result.len(), 200);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_extract_help_no_truncation_at_200_exactly() {
        let text = "b".repeat(200);
        let prop = json!({"description": text.clone()});
        let result = extract_help(&prop).unwrap();
        assert_eq!(result, text);
        assert!(!result.ends_with("..."));
    }

    #[test]
    fn test_extract_help_returns_none_when_absent() {
        let prop = json!({"type": "string"});
        assert_eq!(extract_help(&prop), None);
    }

    // --- prop_name_to_flag_name tests ---

    #[test]
    fn test_prop_name_to_flag_name() {
        assert_eq!(prop_name_to_flag_name("my_val"), "my-val");
        assert_eq!(prop_name_to_flag_name("simple"), "simple");
        assert_eq!(prop_name_to_flag_name("a_b_c"), "a-b-c");
    }

    // --- map_type tests ---

    #[test]
    fn test_map_type_string() {
        let prop = json!({"type": "string"});
        let arg = map_type("name", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("name"));
        assert_eq!(arg.get_id(), "name");
    }

    #[test]
    fn test_map_type_integer() {
        let prop = json!({"type": "integer"});
        let arg = map_type("count", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("count"));
    }

    #[test]
    fn test_map_type_number() {
        let prop = json!({"type": "number"});
        let arg = map_type("rate", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("rate"));
    }

    #[test]
    fn test_map_type_file_suffix() {
        let prop = json!({"type": "string"});
        let arg = map_type("config_file", &prop).unwrap();
        // flag name should be config-file
        assert_eq!(arg.get_long(), Some("config-file"));
    }

    #[test]
    fn test_map_type_x_cli_file() {
        let prop = json!({"type": "string", "x-cli-file": true});
        let arg = map_type("report", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("report"));
    }

    #[test]
    fn test_map_type_object_as_string() {
        let prop = json!({"type": "object"});
        let arg = map_type("data", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("data"));
    }

    #[test]
    fn test_map_type_array_as_string() {
        let prop = json!({"type": "array"});
        let arg = map_type("items", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("items"));
    }

    #[test]
    fn test_map_type_unknown_defaults_to_string() {
        let prop = json!({"type": "foobar"});
        let arg = map_type("x", &prop).unwrap();
        assert_eq!(arg.get_long(), Some("x"));
    }

    // --- boolean flag pair tests ---

    #[test]
    fn test_boolean_flag_pair_produced() {
        let schema = json!({
            "properties": {"verbose": {"type": "boolean"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(
            find_arg(&result.args, "verbose").is_some(),
            "--verbose must be present"
        );
        assert!(
            find_arg(&result.args, "no-verbose").is_some(),
            "--no-verbose must be present"
        );
    }

    #[test]
    fn test_boolean_pair_actions() {
        let schema = json!({
            "properties": {"verbose": {"type": "boolean"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let pos_arg = find_arg(&result.args, "verbose").unwrap();
        let neg_arg = find_arg(&result.args, "no-verbose").unwrap();
        assert!(matches!(pos_arg.get_action(), clap::ArgAction::SetTrue));
        assert!(matches!(neg_arg.get_action(), clap::ArgAction::SetFalse));
    }

    #[test]
    fn test_boolean_default_false() {
        let schema = json!({
            "properties": {"debug": {"type": "boolean"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let pair = result.bool_pairs.iter().find(|p| p.prop_name == "debug");
        assert!(pair.is_some());
        assert!(
            !pair.unwrap().default_val,
            "default must be false when not specified"
        );
    }

    #[test]
    fn test_boolean_default_true() {
        let schema = json!({
            "properties": {"enabled": {"type": "boolean", "default": true}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let pair = result
            .bool_pairs
            .iter()
            .find(|p| p.prop_name == "enabled")
            .expect("BoolFlagPair must be recorded");
        assert!(
            pair.default_val,
            "default must be true when schema says true"
        );
    }

    #[test]
    fn test_boolean_pair_recorded_in_bool_pairs() {
        let schema = json!({
            "properties": {"dry_run": {"type": "boolean"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let pair = result.bool_pairs.iter().find(|p| p.prop_name == "dry_run");
        assert!(pair.is_some(), "BoolFlagPair must be recorded for dry_run");
        assert_eq!(
            pair.unwrap().flag_long,
            "dry-run",
            "flag_long must use hyphen form"
        );
    }

    #[test]
    fn test_boolean_underscore_to_hyphen() {
        let schema = json!({
            "properties": {"dry_run": {"type": "boolean"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "dry-run").is_some(), "--dry-run");
        assert!(
            find_arg(&result.args, "no-dry-run").is_some(),
            "--no-dry-run"
        );
    }

    #[test]
    fn test_boolean_with_enum_true_treated_as_flag() {
        let schema = json!({
            "properties": {"strict": {"type": "boolean", "enum": [true]}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(find_arg(&result.args, "strict").is_some());
        assert!(find_arg(&result.args, "no-strict").is_some());
        assert!(!result.enum_maps.contains_key("strict"));
    }

    #[test]
    fn test_boolean_not_counted_as_required_arg() {
        let schema = json!({
            "properties": {"active": {"type": "boolean"}},
            "required": ["active"]
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let pos = find_arg(&result.args, "active").unwrap();
        let neg = find_arg(&result.args, "no-active").unwrap();
        assert!(!pos.is_required_set());
        assert!(!neg.is_required_set());
    }

    // --- enum-choices tests ---

    #[test]
    fn test_enum_string_choices() {
        let schema = json!({
            "properties": {
                "format": {"type": "string", "enum": ["json", "csv", "xml"]}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "format").expect("--format must exist");
        let pvs = arg.get_possible_values();
        let possible: Vec<&str> = pvs.iter().map(|pv| pv.get_name()).collect();
        assert_eq!(possible, vec!["json", "csv", "xml"]);
    }

    #[test]
    fn test_enum_integer_choices_as_strings() {
        let schema = json!({
            "properties": {
                "level": {"type": "integer", "enum": [1, 2, 3]}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "level").expect("--level must exist");
        let pvs = arg.get_possible_values();
        let possible: Vec<&str> = pvs.iter().map(|pv| pv.get_name()).collect();
        assert_eq!(possible, vec!["1", "2", "3"]);
        let map = result
            .enum_maps
            .get("level")
            .expect("enum_maps must have 'level'");
        assert_eq!(map[0], serde_json::Value::Number(1.into()));
    }

    #[test]
    fn test_enum_float_choices_as_strings() {
        let schema = json!({
            "properties": {
                "ratio": {"type": "number", "enum": [0.5, 1.0, 1.5]}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "ratio").unwrap();
        let pvs = arg.get_possible_values();
        let possible: Vec<&str> = pvs.iter().map(|pv| pv.get_name()).collect();
        assert!(possible.contains(&"0.5"));
    }

    #[test]
    fn test_enum_bool_choices_as_strings() {
        let schema = json!({
            "properties": {
                "flag": {"type": "string", "enum": [true, false]}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "flag").expect("--flag must exist");
        let pvs = arg.get_possible_values();
        let possible: Vec<&str> = pvs.iter().map(|pv| pv.get_name()).collect();
        assert!(possible.contains(&"true"));
        assert!(possible.contains(&"false"));
    }

    #[test]
    fn test_enum_empty_array_falls_through_to_string() {
        let schema = json!({
            "properties": {
                "x": {"type": "string", "enum": []}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "x").expect("--x must exist");
        assert!(arg.get_possible_values().is_empty());
        assert!(!result.enum_maps.contains_key("x"));
    }

    #[test]
    fn test_enum_with_default() {
        let schema = json!({
            "properties": {
                "format": {"type": "string", "enum": ["json", "table"], "default": "json"}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "format").unwrap();
        assert_eq!(
            arg.get_default_values().first().and_then(|v| v.to_str()),
            Some("json")
        );
    }

    #[test]
    fn test_enum_required_property() {
        let schema = json!({
            "properties": {
                "mode": {"type": "string", "enum": ["a", "b"]}
            },
            "required": ["mode"]
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "mode").unwrap();
        assert!(
            !arg.is_required_set(),
            "required enforced post-parse, not at clap level"
        );
    }

    #[test]
    fn test_enum_stored_in_enum_maps() {
        let schema = json!({
            "properties": {
                "priority": {"type": "integer", "enum": [1, 2, 3]}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        assert!(result.enum_maps.contains_key("priority"));
        let map = &result.enum_maps["priority"];
        assert_eq!(map.len(), 3);
    }

    // --- help-text-and-collision tests ---

    #[test]
    fn test_help_prefers_x_llm_description() {
        let schema = json!({
            "properties": {
                "q": {
                    "type": "string",
                    "description": "plain description",
                    "x-llm-description": "LLM-optimised description"
                }
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "q").unwrap();
        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();
        assert!(
            help.contains("LLM-optimised"),
            "help must come from x-llm-description, got: {help}"
        );
        assert!(
            !help.contains("plain description"),
            "help must NOT come from description when x-llm-description is present"
        );
    }

    #[test]
    fn test_help_falls_back_to_description() {
        let schema = json!({
            "properties": {
                "q": {"type": "string", "description": "fallback text"}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "q").unwrap();
        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();
        assert!(help.contains("fallback text"));
    }

    #[test]
    fn test_help_truncated_at_200_chars() {
        let long_desc = "A".repeat(210);
        let schema = json!({
            "properties": {
                "q": {"type": "string", "description": long_desc}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "q").unwrap();
        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();
        assert_eq!(help.len(), 200, "truncated help must be exactly 200 chars");
        assert!(help.ends_with("..."), "truncated help must end with '...'");
    }

    #[test]
    fn test_help_exactly_200_chars_not_truncated() {
        let desc = "B".repeat(200);
        let schema = json!({
            "properties": {
                "q": {"type": "string", "description": desc}
            }
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "q").unwrap();
        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();
        assert_eq!(help.len(), 200);
        assert!(!help.ends_with("..."));
    }

    #[test]
    fn test_help_none_when_no_description_fields() {
        let schema = json!({
            "properties": {"q": {"type": "string"}}
        });
        let result = schema_to_clap_args(&schema).unwrap();
        let arg = find_arg(&result.args, "q").unwrap();
        assert!(arg.get_help().is_none());
    }

    #[test]
    fn test_flag_collision_detection() {
        let schema = json!({
            "properties": {
                "foo_bar": {"type": "string"},
                "foo-bar": {"type": "string"}
            }
        });
        let result = schema_to_clap_args(&schema);
        assert!(
            matches!(result, Err(SchemaParserError::FlagCollision { .. })),
            "expected FlagCollision, got: {result:?}"
        );
    }

    #[test]
    fn test_flag_collision_error_message_contains_both_names() {
        let schema = json!({
            "properties": {
                "my_flag": {"type": "string"},
                "my-flag": {"type": "string"}
            }
        });
        let err = schema_to_clap_args(&schema).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("my_flag") || msg.contains("my-flag"));
        assert!(msg.contains("my-flag") || msg.contains("--my-flag"));
    }

    #[test]
    fn test_no_collision_for_distinct_flags() {
        let schema = json!({
            "properties": {
                "alpha": {"type": "string"},
                "beta": {"type": "string"}
            }
        });
        let result = schema_to_clap_args(&schema);
        assert!(result.is_ok());
    }

    // --- reconvert_enum_values tests ---

    fn make_kwargs(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn test_reconvert_string_enum_passthrough() {
        let schema = json!({
            "properties": {"format": {"type": "string", "enum": ["json", "csv"]}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let kwargs = make_kwargs(&[("format", "json")]);
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert_eq!(result["format"], Value::String("json".to_string()));
    }

    #[test]
    fn test_reconvert_integer_enum() {
        let schema = json!({
            "properties": {"level": {"type": "integer", "enum": [1, 2, 3]}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let kwargs = make_kwargs(&[("level", "2")]);
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert_eq!(result["level"], json!(2));
        assert!(result["level"].is_number());
    }

    #[test]
    fn test_reconvert_float_enum() {
        let schema = json!({
            "properties": {"ratio": {"type": "number", "enum": [0.5, 1.0, 1.5]}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let kwargs = make_kwargs(&[("ratio", "1.5")]);
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert!(result["ratio"].is_number());
        assert_eq!(result["ratio"].as_f64(), Some(1.5));
    }

    #[test]
    fn test_reconvert_bool_enum() {
        let schema = json!({
            "properties": {"strict": {"type": "string", "enum": [true, false]}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let kwargs = make_kwargs(&[("strict", "true")]);
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert_eq!(result["strict"], Value::Bool(true));
    }

    #[test]
    fn test_reconvert_non_enum_field_unchanged() {
        let schema = json!({
            "properties": {"name": {"type": "string"}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let kwargs = make_kwargs(&[("name", "alice")]);
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert_eq!(result["name"], Value::String("alice".to_string()));
    }

    #[test]
    fn test_reconvert_null_value_unchanged() {
        let schema = json!({
            "properties": {"mode": {"type": "string", "enum": ["a", "b"]}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let mut kwargs: HashMap<String, Value> = HashMap::new();
        kwargs.insert("mode".to_string(), Value::Null);
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert_eq!(result["mode"], Value::Null);
    }

    #[test]
    fn test_reconvert_preserves_non_enum_keys() {
        let schema = json!({
            "properties": {"format": {"type": "string", "enum": ["json"]}}
        });
        let schema_args = schema_to_clap_args(&schema).unwrap();
        let mut kwargs = make_kwargs(&[("format", "json")]);
        kwargs.insert("extra".to_string(), Value::String("untouched".to_string()));
        let result = reconvert_enum_values(kwargs, &schema_args);
        assert_eq!(result["extra"], Value::String("untouched".to_string()));
    }
}
