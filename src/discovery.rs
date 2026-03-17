// apcore-cli — Discovery subcommands (list + describe).
// Protocol spec: FE-04

use std::sync::Arc;

use clap::{Arg, ArgAction, Command};
use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// DiscoveryError
// ---------------------------------------------------------------------------

/// Errors produced by discovery command handlers.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("module '{0}' not found")]
    ModuleNotFound(String),

    #[error("invalid module id: {0}")]
    InvalidModuleId(String),

    #[error("invalid tag format: '{0}'. Tags must match [a-z][a-z0-9_-]*.")]
    InvalidTag(String),
}

// ---------------------------------------------------------------------------
// RegistryProvider trait
// ---------------------------------------------------------------------------

/// Minimal registry interface used by discovery commands.
///
/// The real `apcore::Registry` implements this trait via a thin adaptor
/// (added in the `core-dispatcher` feature). Tests use `MockRegistry`.
pub trait RegistryProvider: Send + Sync {
    /// Return all module IDs in the registry.
    fn list(&self) -> Vec<String>;

    /// Return the JSON descriptor for a single module, or `None` if not found.
    fn get_definition(&self, id: &str) -> Option<Value>;
}

// ---------------------------------------------------------------------------
// validate_tag
// ---------------------------------------------------------------------------

/// Validate a tag string against the pattern `^[a-z][a-z0-9_-]*$`.
///
/// Returns `true` if valid, `false` otherwise. Does not exit the process.
pub fn validate_tag(tag: &str) -> bool {
    let mut chars = tag.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// validate_module_id (local, mirrors cli::validate_module_id rules)
// ---------------------------------------------------------------------------

fn validate_module_id_discovery(id: &str) -> bool {
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    if id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return false;
    }
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

// ---------------------------------------------------------------------------
// module_has_all_tags helper
// ---------------------------------------------------------------------------

fn module_has_all_tags(module: &Value, tags: &[&str]) -> bool {
    let mod_tags: Vec<&str> = module
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    tags.iter().all(|required| mod_tags.contains(required))
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

/// Execute the `list` subcommand logic.
///
/// Returns `Ok(String)` with the formatted output on success.
/// Returns `Err(DiscoveryError)` on invalid tag format.
///
/// Exit code mapping for the caller: `DiscoveryError::InvalidTag` → exit 2.
pub fn cmd_list(
    registry: &dyn RegistryProvider,
    tags: &[&str],
    explicit_format: Option<&str>,
) -> Result<String, DiscoveryError> {
    // Validate all tag formats before filtering.
    for tag in tags {
        if !validate_tag(tag) {
            return Err(DiscoveryError::InvalidTag(tag.to_string()));
        }
    }

    // Collect all module definitions.
    let mut modules: Vec<Value> = registry
        .list()
        .into_iter()
        .filter_map(|id| registry.get_definition(&id))
        .collect();

    // Apply AND tag filter if any tags were specified.
    if !tags.is_empty() {
        modules.retain(|m| module_has_all_tags(m, tags));
    }

    let fmt = crate::output::resolve_format(explicit_format);
    Ok(crate::output::format_module_list(&modules, fmt, tags))
}

// ---------------------------------------------------------------------------
// cmd_describe
// ---------------------------------------------------------------------------

/// Execute the `describe` subcommand logic.
///
/// Returns `Ok(String)` with the formatted output on success.
/// Returns `Err(DiscoveryError)` on invalid module ID or module not found.
///
/// Exit code mapping for the caller:
/// - `DiscoveryError::InvalidModuleId` → exit 2
/// - `DiscoveryError::ModuleNotFound`  → exit 44
pub fn cmd_describe(
    registry: &dyn RegistryProvider,
    module_id: &str,
    explicit_format: Option<&str>,
) -> Result<String, DiscoveryError> {
    // Validate module ID format.
    if !validate_module_id_discovery(module_id) {
        return Err(DiscoveryError::InvalidModuleId(module_id.to_string()));
    }

    let module = registry
        .get_definition(module_id)
        .ok_or_else(|| DiscoveryError::ModuleNotFound(module_id.to_string()))?;

    let fmt = crate::output::resolve_format(explicit_format);
    Ok(crate::output::format_module_detail(&module, fmt))
}

// ---------------------------------------------------------------------------
// register_discovery_commands
// ---------------------------------------------------------------------------

/// Attach `list` and `describe` subcommands to the given root command.
///
/// Returns the root command with the subcommands added. Follows the clap v4
/// builder idiom (commands are consumed and returned, not mutated in-place).
pub fn register_discovery_commands(
    cli: Command,
    _registry: Arc<dyn RegistryProvider>,
) -> Command {
    cli.subcommand(list_command())
        .subcommand(describe_command())
}

// ---------------------------------------------------------------------------
// list_command / describe_command builders
// ---------------------------------------------------------------------------

fn list_command() -> Command {
    Command::new("list")
        .about("List available modules in the registry")
        .arg(
            Arg::new("tag")
                .long("tag")
                .action(ArgAction::Append)
                .value_name("TAG")
                .help("Filter modules by tag (AND logic). Repeatable."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(clap::builder::PossibleValuesParser::new(["table", "json"]))
                .value_name("FORMAT")
                .help("Output format. Default: table (TTY) or json (non-TTY)."),
        )
}

fn describe_command() -> Command {
    Command::new("describe")
        .about("Show metadata, schema, and annotations for a module")
        .arg(
            Arg::new("module_id")
                .required(true)
                .value_name("MODULE_ID")
                .help("Canonical module identifier (e.g. math.add)"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(clap::builder::PossibleValuesParser::new(["table", "json"]))
                .value_name("FORMAT")
                .help("Output format. Default: table (TTY) or json (non-TTY)."),
        )
}

// ---------------------------------------------------------------------------
// MockRegistry — public for integration tests
// ---------------------------------------------------------------------------

/// Test helper: in-memory registry backed by a Vec of JSON module descriptors.
#[doc(hidden)]
pub struct MockRegistry {
    modules: Vec<Value>,
}

#[doc(hidden)]
impl MockRegistry {
    pub fn new(modules: Vec<Value>) -> Self {
        Self { modules }
    }
}

impl RegistryProvider for MockRegistry {
    fn list(&self) -> Vec<String> {
        self.modules
            .iter()
            .filter_map(|m| m.get("module_id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect()
    }

    fn get_definition(&self, id: &str) -> Option<Value> {
        self.modules
            .iter()
            .find(|m| m.get("module_id").and_then(|v| v.as_str()) == Some(id))
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// mock_module helper — public for integration tests
// ---------------------------------------------------------------------------

/// Test helper: build a minimal module descriptor JSON value.
#[doc(hidden)]
pub fn mock_module(id: &str, description: &str, tags: &[&str]) -> Value {
    serde_json::json!({
        "module_id": id,
        "description": description,
        "tags": tags,
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- validate_tag ---

    #[test]
    fn test_validate_tag_valid_simple() {
        assert!(validate_tag("math"), "single lowercase word must be valid");
    }

    #[test]
    fn test_validate_tag_valid_with_digits_and_dash() {
        assert!(validate_tag("ml-v2"), "digits and dash must be valid");
    }

    #[test]
    fn test_validate_tag_valid_with_underscore() {
        assert!(validate_tag("core_util"), "underscore must be valid");
    }

    #[test]
    fn test_validate_tag_invalid_uppercase() {
        assert!(!validate_tag("Math"), "uppercase start must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_starts_with_digit() {
        assert!(!validate_tag("1tag"), "digit start must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_special_chars() {
        assert!(!validate_tag("invalid!"), "special chars must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_empty() {
        assert!(!validate_tag(""), "empty string must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_space() {
        assert!(!validate_tag("has space"), "space must be invalid");
    }

    // --- RegistryProvider / MockRegistry ---

    #[test]
    fn test_mock_registry_list_returns_ids() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("text.upper", "Uppercase text", &["text"]),
        ]);
        let ids = registry.list();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"math.add".to_string()));
    }

    #[test]
    fn test_mock_registry_get_definition_found() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math"]),
        ]);
        let def = registry.get_definition("math.add");
        assert!(def.is_some());
        assert_eq!(def.unwrap()["module_id"], "math.add");
    }

    #[test]
    fn test_mock_registry_get_definition_not_found() {
        let registry = MockRegistry::new(vec![]);
        assert!(registry.get_definition("non.existent").is_none());
    }

    // --- cmd_list ---

    #[test]
    fn test_cmd_list_all_modules_no_filter() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("text.upper", "Uppercase text", &["text"]),
        ]);
        let output = cmd_list(&registry, &[], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn test_cmd_list_empty_registry_table() {
        let registry = MockRegistry::new(vec![]);
        let output = cmd_list(&registry, &[], Some("table")).unwrap();
        assert_eq!(output.trim(), "No modules found.");
    }

    #[test]
    fn test_cmd_list_empty_registry_json() {
        let registry = MockRegistry::new(vec![]);
        let output = cmd_list(&registry, &[], Some("json")).unwrap();
        assert_eq!(output.trim(), "[]");
    }

    #[test]
    fn test_cmd_list_tag_filter_single_match() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("text.upper", "Uppercase text", &["text"]),
        ]);
        let output = cmd_list(&registry, &["math"], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "math.add");
    }

    #[test]
    fn test_cmd_list_tag_filter_and_semantics() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("math.mul", "Multiply", &["math"]),
        ]);
        // Only math.add has BOTH "math" AND "core".
        let output = cmd_list(&registry, &["math", "core"], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "math.add");
    }

    #[test]
    fn test_cmd_list_tag_filter_no_match_table() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math"]),
        ]);
        let output = cmd_list(&registry, &["nonexistent"], Some("table")).unwrap();
        assert!(output.contains("No modules found matching tags:"));
        assert!(output.contains("nonexistent"));
    }

    #[test]
    fn test_cmd_list_tag_filter_no_match_json() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math"]),
        ]);
        let output = cmd_list(&registry, &["nonexistent"], Some("json")).unwrap();
        assert_eq!(output.trim(), "[]");
    }

    #[test]
    fn test_cmd_list_invalid_tag_format_returns_error() {
        let registry = MockRegistry::new(vec![]);
        let result = cmd_list(&registry, &["INVALID!"], Some("json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            DiscoveryError::InvalidTag(tag) => assert_eq!(tag, "INVALID!"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_cmd_list_description_truncated_in_table() {
        let long_desc = "x".repeat(100);
        let registry = MockRegistry::new(vec![
            mock_module("a.b", &long_desc, &[]),
        ]);
        let output = cmd_list(&registry, &[], Some("table")).unwrap();
        assert!(output.contains("..."), "long description must be truncated");
        assert!(!output.contains(&"x".repeat(100)), "full description must not appear");
    }

    #[test]
    fn test_cmd_list_json_contains_id_description_tags() {
        let registry = MockRegistry::new(vec![
            mock_module("a.b", "Desc", &["x", "y"]),
        ]);
        let output = cmd_list(&registry, &[], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let entry = &parsed[0];
        assert!(entry.get("id").is_some());
        assert!(entry.get("description").is_some());
        assert!(entry.get("tags").is_some());
    }

    // --- cmd_describe ---

    #[test]
    fn test_cmd_describe_valid_module_json() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add two numbers", &["math", "core"]),
        ]);
        let output = cmd_describe(&registry, "math.add", Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["id"], "math.add");
        assert_eq!(parsed["description"], "Add two numbers");
    }

    #[test]
    fn test_cmd_describe_valid_module_table() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add two numbers", &["math"]),
        ]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(output.contains("math.add"), "table must contain module id");
        assert!(output.contains("Add two numbers"), "table must contain description");
    }

    #[test]
    fn test_cmd_describe_not_found_returns_error() {
        let registry = MockRegistry::new(vec![]);
        let result = cmd_describe(&registry, "non.existent", Some("json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            DiscoveryError::ModuleNotFound(id) => assert_eq!(id, "non.existent"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_cmd_describe_invalid_id_returns_error() {
        let registry = MockRegistry::new(vec![]);
        let result = cmd_describe(&registry, "INVALID!ID", Some("json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            DiscoveryError::InvalidModuleId(_) => {}
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_cmd_describe_no_output_schema_table_omits_section() {
        // Module without output_schema: section must be absent from table output.
        let registry = MockRegistry::new(vec![
            serde_json::json!({
                "module_id": "math.add",
                "description": "Add numbers",
                "input_schema": {"type": "object"},
                "tags": ["math"]
                // note: no output_schema key
            }),
        ]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(!output.contains("Output Schema:"), "output_schema section must be absent");
    }

    #[test]
    fn test_cmd_describe_no_annotations_table_omits_section() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math"]),
        ]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(!output.contains("Annotations:"), "annotations section must be absent");
    }

    #[test]
    fn test_cmd_describe_with_annotations_table_shows_section() {
        let registry = MockRegistry::new(vec![
            serde_json::json!({
                "module_id": "math.add",
                "description": "Add numbers",
                "annotations": {"readonly": true},
                "tags": []
            }),
        ]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(output.contains("Annotations:"), "annotations section must be present");
        assert!(output.contains("readonly"), "annotation key must appear");
    }

    #[test]
    fn test_cmd_describe_json_omits_null_fields() {
        // Module with no input_schema, output_schema, annotations.
        let registry = MockRegistry::new(vec![
            mock_module("a.b", "Desc", &[]),
        ]);
        let output = cmd_describe(&registry, "a.b", Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("input_schema").is_none());
        assert!(parsed.get("output_schema").is_none());
        assert!(parsed.get("annotations").is_none());
    }

    #[test]
    fn test_cmd_describe_json_includes_all_fields() {
        let registry = MockRegistry::new(vec![
            serde_json::json!({
                "module_id": "math.add",
                "description": "Add two numbers",
                "input_schema": {"type": "object", "properties": {"a": {"type": "integer"}}},
                "output_schema": {"type": "object", "properties": {"result": {"type": "integer"}}},
                "annotations": {"readonly": false},
                "tags": ["math", "core"]
            }),
        ]);
        let output = cmd_describe(&registry, "math.add", Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("input_schema").is_some());
        assert!(parsed.get("output_schema").is_some());
        assert!(parsed.get("annotations").is_some());
        assert!(parsed.get("tags").is_some());
    }

    #[test]
    fn test_cmd_describe_with_x_fields_table_shows_extension_section() {
        let registry = MockRegistry::new(vec![
            serde_json::json!({
                "module_id": "a.b",
                "description": "Desc",
                "x-custom": "custom-value",
                "tags": []
            }),
        ]);
        let output = cmd_describe(&registry, "a.b", Some("table")).unwrap();
        assert!(
            output.contains("Extension Metadata:") || output.contains("x-custom"),
            "x-fields must appear in table output"
        );
    }

    // --- register_discovery_commands ---

    #[test]
    fn test_register_discovery_commands_adds_list() {
        use std::sync::Arc;
        let registry = Arc::new(MockRegistry::new(vec![]));
        let root = Command::new("apcore-cli");
        let cmd = register_discovery_commands(root, registry);
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(names.contains(&"list"), "must have 'list' subcommand, got {names:?}");
    }

    #[test]
    fn test_register_discovery_commands_adds_describe() {
        use std::sync::Arc;
        let registry = Arc::new(MockRegistry::new(vec![]));
        let root = Command::new("apcore-cli");
        let cmd = register_discovery_commands(root, registry);
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"describe"),
            "must have 'describe' subcommand, got {names:?}"
        );
    }

    #[test]
    fn test_list_command_with_tag_filter() {
        let cmd = list_command();
        let arg_names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(arg_names.contains(&"tag"), "list must have --tag flag");
    }

    #[test]
    fn test_describe_command_module_not_found() {
        // Verify module_id positional arg is present.
        let cmd = describe_command();
        let positionals: Vec<&str> = cmd
            .get_positionals()
            .filter_map(|a| a.get_id().as_str().into())
            .collect();
        assert!(
            positionals.contains(&"module_id"),
            "describe must have module_id positional, got {positionals:?}"
        );
    }
}
