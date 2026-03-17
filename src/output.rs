// apcore-cli — TTY-adaptive output formatting.
// Protocol spec: FE-04 (format_module_list, format_module_detail,
//                        format_exec_result, resolve_format)

use serde_json::Value;
use std::io::IsTerminal;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const DESCRIPTION_TRUNCATE_LEN: usize = 80;

// ---------------------------------------------------------------------------
// resolve_format
// ---------------------------------------------------------------------------

/// Private inner: accepts explicit TTY state for testability.
pub(crate) fn resolve_format_inner(explicit_format: Option<&str>, is_tty: bool) -> &'static str {
    if let Some(fmt) = explicit_format {
        return match fmt {
            "json" => "json",
            "table" => "table",
            other => {
                // Unknown format: log a warning and fall back to json.
                // (Invalid values are caught by clap upstream; this is a safety net.)
                tracing::warn!("Unknown format '{}', defaulting to 'json'.", other);
                "json"
            }
        };
    }
    if is_tty { "table" } else { "json" }
}

/// Determine the output format to use.
///
/// Resolution order:
/// 1. `explicit_format` if `Some`.
/// 2. `"table"` when stdout is a TTY.
/// 3. `"json"` otherwise.
pub fn resolve_format(explicit_format: Option<&str>) -> &'static str {
    let is_tty = std::io::stdout().is_terminal();
    resolve_format_inner(explicit_format, is_tty)
}

// ---------------------------------------------------------------------------
// truncate
// ---------------------------------------------------------------------------

/// Truncate `text` to at most `max_length` characters.
///
/// If truncation occurs, the last 3 characters are replaced with `"..."`.
/// Uses char-boundary-safe truncation to handle Unicode correctly: byte length
/// is used for the boundary check (matching Python's `len()` on ASCII-dominant
/// module descriptions), but slicing respects char boundaries.
pub(crate) fn truncate(text: &str, max_length: usize) -> String {
    if text.len() <= max_length {
        return text.to_string();
    }
    let cutoff = max_length.saturating_sub(3);
    // Walk back from cutoff to find a valid char boundary.
    let mut end = cutoff;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

// ---------------------------------------------------------------------------
// format_module_list helpers
// ---------------------------------------------------------------------------

/// Extract a string field from a JSON module descriptor with fallback keys.
fn extract_str<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    for key in keys {
        if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
            return s;
        }
    }
    ""
}

/// Extract tags array from a JSON module descriptor. Returns empty Vec on missing/invalid.
fn extract_tags(v: &Value) -> Vec<String> {
    v.get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// format_module_list
// ---------------------------------------------------------------------------

/// Render a list of module descriptors as a table or JSON.
///
/// # Arguments
/// * `modules`      — slice of `serde_json::Value` objects (module descriptors)
/// * `format`       — `"table"` or `"json"`
/// * `filter_tags`  — AND-filter: only modules that have ALL listed tags are shown
///
/// Returns the formatted string ready for printing to stdout.
pub fn format_module_list(modules: &[Value], format: &str, filter_tags: &[&str]) -> String {
    use comfy_table::{ContentArrangement, Table};

    match format {
        "table" => {
            if modules.is_empty() {
                if !filter_tags.is_empty() {
                    return format!(
                        "No modules found matching tags: {}.",
                        filter_tags.join(", ")
                    );
                }
                return "No modules found.".to_string();
            }

            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec!["ID", "Description", "Tags"]);

            for m in modules {
                let id = extract_str(m, &["module_id", "id", "canonical_id"]);
                let desc_raw = extract_str(m, &["description"]);
                let desc = truncate(desc_raw, DESCRIPTION_TRUNCATE_LEN);
                let tags = extract_tags(m).join(", ");
                table.add_row(vec![id.to_string(), desc, tags]);
            }

            table.to_string()
        }
        "json" => {
            let result: Vec<serde_json::Value> = modules
                .iter()
                .map(|m| {
                    let id = extract_str(m, &["module_id", "id", "canonical_id"]);
                    let desc = extract_str(m, &["description"]);
                    let tags: Vec<serde_json::Value> = extract_tags(m)
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect();
                    serde_json::json!({
                        "id": id,
                        "description": desc,
                        "tags": tags,
                    })
                })
                .collect();

            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "[]".to_string())
        }
        unknown => {
            tracing::warn!(
                "Unknown format '{}' in format_module_list, using json.",
                unknown
            );
            format_module_list(modules, "json", filter_tags)
        }
    }
}

// ---------------------------------------------------------------------------
// format_module_detail
// ---------------------------------------------------------------------------

/// Render a minimal bordered panel heading. Returns a String with a box around `title`.
fn render_panel(title: &str) -> String {
    use comfy_table::Table;
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    table.add_row(vec![title]);
    table.to_string()
}

/// Render an optional section with a label and preformatted content.
/// Returns None if content is empty.
fn render_section(title: &str, content: &str) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    Some(format!("\n{}:\n{}", title, content))
}

/// Render a single module descriptor with its full schema.
///
/// # Arguments
/// * `module` — `serde_json::Value` module descriptor
/// * `format` — `"table"` or `"json"`
pub fn format_module_detail(module: &Value, format: &str) -> String {
    let id = extract_str(module, &["module_id", "id", "canonical_id"]);
    let description = extract_str(module, &["description"]);

    match format {
        "table" => {
            let mut parts: Vec<String> = Vec::new();

            // Header panel.
            parts.push(render_panel(&format!("Module: {}", id)));

            // Description.
            parts.push(format!("\nDescription:\n  {}", description));

            // Input schema.
            if let Some(input_schema) = module.get("input_schema").filter(|v| !v.is_null()) {
                let content = serde_json::to_string_pretty(input_schema)
                    .unwrap_or_else(|_| "{}".to_string());
                if let Some(section) = render_section("Input Schema", &content) {
                    parts.push(section);
                }
            }

            // Output schema.
            if let Some(output_schema) = module.get("output_schema").filter(|v| !v.is_null()) {
                let content = serde_json::to_string_pretty(output_schema)
                    .unwrap_or_else(|_| "{}".to_string());
                if let Some(section) = render_section("Output Schema", &content) {
                    parts.push(section);
                }
            }

            // Annotations.
            if let Some(ann) = module.get("annotations").and_then(|v| v.as_object()) {
                if !ann.is_empty() {
                    let content: String = ann
                        .iter()
                        .map(|(k, v)| {
                            let val = v.as_str().unwrap_or(&v.to_string()).to_string();
                            format!("  {}: {}", k, val)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Some(section) = render_section("Annotations", &content) {
                        parts.push(section);
                    }
                }
            }

            // Extension metadata (x- or x_ prefixed keys at the top level).
            let x_fields: Vec<(String, String)> = module
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter(|(k, _)| k.starts_with("x-") || k.starts_with("x_"))
                        .map(|(k, v)| {
                            let val = v.as_str().unwrap_or(&v.to_string()).to_string();
                            (k.clone(), val)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !x_fields.is_empty() {
                let content: String = x_fields
                    .iter()
                    .map(|(k, v)| format!("  {}: {}", k, v))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(section) = render_section("Extension Metadata", &content) {
                    parts.push(section);
                }
            }

            // Tags.
            let tags = extract_tags(module);
            if !tags.is_empty() {
                if let Some(section) = render_section("Tags", &format!("  {}", tags.join(", "))) {
                    parts.push(section);
                }
            }

            parts.join("\n")
        }
        "json" => {
            let mut result = serde_json::Map::new();
            result.insert("id".to_string(), serde_json::Value::String(id.to_string()));
            result.insert(
                "description".to_string(),
                serde_json::Value::String(description.to_string()),
            );

            // Optional fields: only include if present and non-null.
            for key in &["input_schema", "output_schema"] {
                if let Some(v) = module.get(*key).filter(|v| !v.is_null()) {
                    result.insert(key.to_string(), v.clone());
                }
            }

            if let Some(ann) = module
                .get("annotations")
                .filter(|v| !v.is_null() && v.as_object().map_or(false, |o| !o.is_empty()))
            {
                result.insert("annotations".to_string(), ann.clone());
            }

            let tags = extract_tags(module);
            if !tags.is_empty() {
                result.insert(
                    "tags".to_string(),
                    serde_json::Value::Array(
                        tags.into_iter().map(serde_json::Value::String).collect(),
                    ),
                );
            }

            // Extension metadata.
            if let Some(obj) = module.as_object() {
                for (k, v) in obj {
                    if k.starts_with("x-") || k.starts_with("x_") {
                        result.insert(k.clone(), v.clone());
                    }
                }
            }

            serde_json::to_string_pretty(&serde_json::Value::Object(result))
                .unwrap_or_else(|_| "{}".to_string())
        }
        unknown => {
            tracing::warn!(
                "Unknown format '{}' in format_module_detail, using json.",
                unknown
            );
            format_module_detail(module, "json")
        }
    }
}

// ---------------------------------------------------------------------------
// format_exec_result
// ---------------------------------------------------------------------------

/// Render a module execution result.
///
/// # Arguments
/// * `result` — `serde_json::Value` (the `output` field from the executor response)
/// * `format` — `"table"` or `"json"`
pub fn format_exec_result(result: &Value, format: &str) -> String {
    // TODO: table → key-value comfy-table
    //       json  → serde_json::to_string_pretty
    let _ = (result, format);
    todo!("format_exec_result")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- resolve_format_inner ---

    #[test]
    fn test_resolve_format_explicit_json_tty() {
        // Explicit format wins over TTY state.
        assert_eq!(resolve_format_inner(Some("json"), true), "json");
    }

    #[test]
    fn test_resolve_format_explicit_table_non_tty() {
        // Explicit format wins over non-TTY state.
        assert_eq!(resolve_format_inner(Some("table"), false), "table");
    }

    #[test]
    fn test_resolve_format_none_tty() {
        // No explicit format + TTY → "table".
        assert_eq!(resolve_format_inner(None, true), "table");
    }

    #[test]
    fn test_resolve_format_none_non_tty() {
        // No explicit format + non-TTY → "json".
        assert_eq!(resolve_format_inner(None, false), "json");
    }

    // --- truncate ---

    #[test]
    fn test_truncate_short_string() {
        let s = "hello";
        assert_eq!(truncate(s, 80), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        let s = "a".repeat(80);
        assert_eq!(truncate(&s, 80), s);
    }

    #[test]
    fn test_truncate_over_limit() {
        let s = "a".repeat(100);
        let result = truncate(&s, 80);
        assert_eq!(result.len(), 80);
        assert!(result.ends_with("..."));
        assert_eq!(&result[..77], &"a".repeat(77));
    }

    #[test]
    fn test_truncate_exactly_81_chars() {
        let s = "b".repeat(81);
        let result = truncate(&s, 80);
        assert_eq!(result.len(), 80);
        assert!(result.ends_with("..."));
    }

    // --- format_module_list ---

    #[test]
    fn test_format_module_list_json_two_modules() {
        let modules = vec![
            json!({"module_id": "math.add", "description": "Add numbers", "tags": ["math"]}),
            json!({"module_id": "text.upper", "description": "Uppercase", "tags": []}),
        ];
        let output = format_module_list(&modules, "json", &[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
        let arr = parsed.as_array().expect("must be array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], "math.add");
        assert_eq!(arr[1]["id"], "text.upper");
    }

    #[test]
    fn test_format_module_list_json_empty() {
        let output = format_module_list(&[], "json", &[]);
        assert_eq!(output.trim(), "[]");
    }

    #[test]
    fn test_format_module_list_table_two_modules() {
        let modules = vec![
            json!({"module_id": "math.add", "description": "Add numbers", "tags": ["math"]}),
        ];
        let output = format_module_list(&modules, "table", &[]);
        assert!(output.contains("math.add"), "table must contain module ID");
        assert!(output.contains("Add numbers"), "table must contain description");
    }

    #[test]
    fn test_format_module_list_table_columns() {
        let modules = vec![
            json!({"module_id": "math.add", "description": "Add numbers", "tags": []}),
        ];
        let output = format_module_list(&modules, "table", &[]);
        assert!(output.contains("ID"), "table must have ID column");
        assert!(output.contains("Description"), "table must have Description column");
        assert!(output.contains("Tags"), "table must have Tags column");
    }

    #[test]
    fn test_format_module_list_table_empty_no_tags() {
        let output = format_module_list(&[], "table", &[]);
        assert_eq!(output.trim(), "No modules found.");
    }

    #[test]
    fn test_format_module_list_table_empty_with_filter_tags() {
        let output = format_module_list(&[], "table", &["math", "text"]);
        assert!(
            output.contains("No modules found matching tags:"),
            "must contain tag-filter message"
        );
        assert!(output.contains("math"), "must contain tag name");
        assert!(output.contains("text"), "must contain tag name");
    }

    #[test]
    fn test_format_module_list_table_description_truncated() {
        let long_desc = "a".repeat(100);
        let modules = vec![
            json!({"module_id": "x.y", "description": long_desc, "tags": []}),
        ];
        let output = format_module_list(&modules, "table", &[]);
        assert!(output.contains("..."), "long description must be truncated with '...'");
        assert!(!output.contains(&"a".repeat(100)), "full description must not appear");
    }

    #[test]
    fn test_format_module_list_json_tags_present() {
        let modules = vec![
            json!({"module_id": "a.b", "description": "desc", "tags": ["x", "y"]}),
        ];
        let output = format_module_list(&modules, "json", &[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let tags = parsed[0]["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], "x");
    }

    // Placeholder tests for future tasks (kept to avoid removing stubs needed by other tasks)
    #[test]
    fn test_format_exec_result_json() {
        // TODO: verify execution result JSON round-trips correctly.
        assert!(false, "not implemented");
    }

    #[test]
    fn test_format_exec_result_table() {
        // TODO: verify table output contains result key-value pairs.
        assert!(false, "not implemented");
    }

    // --- format_module_detail ---

    #[test]
    fn test_format_module_detail_json_full() {
        let module = json!({
            "module_id": "math.add",
            "description": "Add two numbers",
            "input_schema": {"type": "object", "properties": {"a": {"type": "integer"}}},
            "output_schema": {"type": "object", "properties": {"result": {"type": "integer"}}},
            "tags": ["math"],
            "annotations": {"author": "test"}
        });
        let output = format_module_detail(&module, "json");
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
        assert_eq!(parsed["id"], "math.add");
        assert_eq!(parsed["description"], "Add two numbers");
        assert!(parsed.get("input_schema").is_some(), "input_schema must be present");
        assert!(parsed.get("output_schema").is_some(), "output_schema must be present");
        let tags = parsed["tags"].as_array().unwrap();
        assert_eq!(tags[0], "math");
    }

    #[test]
    fn test_format_module_detail_json_no_output_schema() {
        let module = json!({
            "module_id": "text.upper",
            "description": "Uppercase",
        });
        let output = format_module_detail(&module, "json");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("output_schema").is_none(), "output_schema must be absent when not set");
    }

    #[test]
    fn test_format_module_detail_json_no_none_fields() {
        let module = json!({
            "module_id": "a.b",
            "description": "desc",
            "input_schema": null,
            "output_schema": null,
            "tags": null,
        });
        let output = format_module_detail(&module, "json");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("input_schema").is_none(), "null input_schema must be absent");
        assert!(parsed.get("tags").is_none(), "null tags must be absent");
    }

    #[test]
    fn test_format_module_detail_table_contains_description() {
        let module = json!({
            "module_id": "math.add",
            "description": "Add two numbers",
        });
        let output = format_module_detail(&module, "table");
        assert!(output.contains("Add two numbers"), "table must contain description");
    }

    #[test]
    fn test_format_module_detail_table_contains_module_id() {
        let module = json!({
            "module_id": "math.add",
            "description": "desc",
        });
        let output = format_module_detail(&module, "table");
        assert!(output.contains("math.add"), "table must contain module ID");
    }

    #[test]
    fn test_format_module_detail_table_input_schema_section() {
        let module = json!({
            "module_id": "math.add",
            "description": "desc",
            "input_schema": {"type": "object"}
        });
        let output = format_module_detail(&module, "table");
        assert!(output.contains("Input Schema"), "table must contain Input Schema section");
    }

    #[test]
    fn test_format_module_detail_table_no_output_schema_section_when_absent() {
        let module = json!({
            "module_id": "text.upper",
            "description": "desc",
        });
        let output = format_module_detail(&module, "table");
        assert!(
            !output.contains("Output Schema"),
            "Output Schema section must be absent when not set"
        );
    }

    #[test]
    fn test_format_module_detail_table_tags_section() {
        let module = json!({
            "module_id": "math.add",
            "description": "desc",
            "tags": ["math", "arithmetic"]
        });
        let output = format_module_detail(&module, "table");
        assert!(output.contains("Tags"), "table must contain Tags section");
        assert!(output.contains("math"), "table must contain tag value");
    }

    #[test]
    fn test_format_module_detail_table_annotations_section() {
        let module = json!({
            "module_id": "a.b",
            "description": "desc",
            "annotations": {"author": "alice", "version": "1.0"}
        });
        let output = format_module_detail(&module, "table");
        assert!(output.contains("Annotations"), "table must contain Annotations section");
        assert!(output.contains("author"), "table must contain annotation key");
        assert!(output.contains("alice"), "table must contain annotation value");
    }

    #[test]
    fn test_format_module_detail_table_extension_metadata() {
        let module = json!({
            "module_id": "a.b",
            "description": "desc",
            "x-category": "utility"
        });
        let output = format_module_detail(&module, "table");
        assert!(output.contains("Extension Metadata"), "must contain Extension Metadata section");
        assert!(output.contains("x-category"), "must contain x- key");
        assert!(output.contains("utility"), "must contain x- value");
    }
}
