// apcore-cli — TTY-adaptive output formatting.
// Protocol spec: FE-04 (format_module_list, format_module_detail,
//                        format_exec_result, resolve_format)

use serde_json::Value;
use std::io::IsTerminal;

/// Adapt a registry-style JSON Value (module descriptor) to the toolkit's
/// `ScannedModule` so the surface formatters can render it.
///
/// Both shapes share most fields (`module_id`, `description`,
/// `input_schema`, `output_schema`, `tags`, `annotations`, `examples`,
/// `metadata`); the toolkit additionally needs `target` (set to ""),
/// `version` (defaulted), and `display` (sourced from `metadata.display`
/// when present).
pub(crate) fn descriptor_to_scanned(m: &Value) -> apcore_toolkit::ScannedModule {
    use apcore_toolkit::ScannedModule;

    let module_id = extract_str(m, &["module_id", "id", "canonical_id", "name"]).to_string();
    let description = extract_str(m, &["description"]).to_string();
    let input_schema = m
        .get("input_schema")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let output_schema = m
        .get("output_schema")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let tags = extract_tags(m);

    let mut sm = ScannedModule::new(
        module_id,
        description,
        input_schema,
        output_schema,
        tags,
        String::new(),
    );

    if let Some(metadata_obj) = m.get("metadata").and_then(|v| v.as_object()) {
        for (k, v) in metadata_obj {
            sm.metadata.insert(k.clone(), v.clone());
        }
        if let Some(display) = metadata_obj.get("display") {
            if !display.is_null() {
                sm.display = Some(display.clone());
            }
        }
    }

    if let Some(ann) = m.get("annotations") {
        if let Ok(parsed) = serde_json::from_value::<apcore::module::ModuleAnnotations>(ann.clone())
        {
            sm.annotations = Some(parsed);
        }
    }

    sm
}

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
            "csv" => "csv",
            "yaml" => "yaml",
            "jsonl" => "jsonl",
            "markdown" => "markdown",
            "skill" => "skill",
            other => {
                // Unknown format: log a warning and fall back to json.
                // (Invalid values are caught by clap upstream; this is a safety net.)
                tracing::warn!("Unknown format '{}', defaulting to 'json'.", other);
                "json"
            }
        };
    }
    if is_tty {
        "table"
    } else {
        "json"
    }
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

/// Coerce a JSON `Value` into the row shape expected by the toolkit's
/// tabular formatters: a slice of `Map<String, Value>`. Returns `None` for
/// shapes that don't map to tabular (scalars, empty arrays, arrays of
/// non-objects).
fn rows_for_tabular(value: &Value) -> Option<Vec<serde_json::Map<String, Value>>> {
    match value {
        Value::Null => None,
        Value::Object(obj) => Some(vec![obj.clone()]),
        Value::Array(arr) => {
            if arr.is_empty() {
                return None;
            }
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                match item {
                    Value::Object(obj) => out.push(obj.clone()),
                    _ => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// format_module_list
// ---------------------------------------------------------------------------

/// Count a module descriptor's dependencies, for the `--deps` column/field.
///
/// Reads the top-level `"dependencies"` array `ModuleDescriptor.dependencies`
/// serializes to (parity with Python's `getattr(m, "dependencies", None) or
/// []`, which reads the same top-level attribute, not a nested
/// `metadata.dependencies`). Missing or non-array values count as zero.
fn dependency_count(m: &Value) -> usize {
    m.get("dependencies")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Additive display options for [`format_module_list_with_options`] (FE-11
/// F7 dependency column, FE-12 exposure column/filter-echo).
///
/// A separate parameter object plus a separate entry point, rather than
/// widening [`format_module_list`]'s own signature: that function has ~18
/// existing call sites (several of them tests, in this file and in
/// `tests/test_output.rs` / `tests/test_integration.rs`) which must keep
/// compiling unchanged.
#[derive(Default)]
pub struct ListDisplayOptions<'a> {
    /// Add a right-justified "Deps" column (table) / `dependency_count`
    /// field (json) with each module's dependency count.
    pub show_deps: bool,
    /// When `Some`, add a centered "Exposure" column (table: "✓"/"—") /
    /// `exposed` field (json), computed via `filter.is_exposed(module_id)`.
    pub exposure_filter: Option<&'a crate::exposure::ExposureFilter>,
}

/// Render a list of module descriptors as a table or JSON.
///
/// # Arguments
/// * `modules`      — slice of `serde_json::Value` objects (module descriptors)
/// * `format`       — `"table"` or `"json"`
/// * `filter_tags`  — AND-filter: only modules that have ALL listed tags are shown
///
/// Returns the formatted string ready for printing to stdout.
///
/// Thin wrapper over [`format_module_list_with_options`] with both optional
/// columns off, preserving this function's exact historical behavior and
/// signature.
pub fn format_module_list(modules: &[Value], format: &str, filter_tags: &[&str]) -> String {
    format_module_list_with_options(modules, format, filter_tags, &ListDisplayOptions::default())
}

/// [`format_module_list`], additionally able to show the FE-11 `--deps` and
/// FE-12 `--exposure all` columns per `opts`.
pub fn format_module_list_with_options(
    modules: &[Value],
    format: &str,
    filter_tags: &[&str],
    opts: &ListDisplayOptions<'_>,
) -> String {
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
            let mut headers = vec!["ID", "Description", "Tags"];
            if opts.show_deps {
                headers.push("Deps");
            }
            if opts.exposure_filter.is_some() {
                headers.push("Exposure");
            }
            table.set_header(headers);

            for m in modules {
                let id = extract_str(m, &["module_id", "id", "canonical_id", "name"]);
                let desc_raw = extract_str(m, &["description"]);
                let desc = truncate(desc_raw, DESCRIPTION_TRUNCATE_LEN);
                let tags = extract_tags(m).join(", ");
                let mut row = vec![id.to_string(), desc, tags];
                if opts.show_deps {
                    row.push(dependency_count(m).to_string());
                }
                if let Some(filter) = opts.exposure_filter {
                    row.push(
                        if filter.is_exposed(id) {
                            "\u{2713}"
                        } else {
                            "\u{2014}"
                        }
                        .to_string(),
                    );
                }
                table.add_row(row);
            }

            table.to_string()
        }
        "json" => {
            let result: Vec<serde_json::Value> = modules
                .iter()
                .map(|m| {
                    let id = extract_str(m, &["module_id", "id", "canonical_id", "name"]);
                    let desc = extract_str(m, &["description"]);
                    let tags: Vec<serde_json::Value> = extract_tags(m)
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect();
                    let mut entry = serde_json::json!({
                        "id": id,
                        "description": desc,
                        "tags": tags,
                    });
                    if opts.show_deps {
                        entry["dependency_count"] = serde_json::json!(dependency_count(m));
                    }
                    if let Some(filter) = opts.exposure_filter {
                        entry["exposed"] = serde_json::json!(filter.is_exposed(id));
                    }
                    entry
                })
                .collect();

            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "[]".to_string())
        }
        "markdown" | "skill" => {
            use apcore_toolkit::{format_modules, FormatOutput, ModuleStyle};
            let style = if format == "skill" {
                ModuleStyle::Skill
            } else {
                ModuleStyle::Markdown
            };
            let scanned: Vec<_> = modules.iter().map(descriptor_to_scanned).collect();
            match format_modules(&scanned, style, None, true) {
                FormatOutput::Text(s) => s,
                other => format!("{:?}", other),
            }
        }
        unknown => {
            tracing::warn!(
                "Unknown format '{}' in format_module_list, using json.",
                unknown
            );
            format_module_list_with_options(modules, "json", filter_tags, opts)
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
    let id = extract_str(module, &["module_id", "id", "canonical_id", "name"]);
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
                let content =
                    serde_json::to_string_pretty(input_schema).unwrap_or_else(|_| "{}".to_string());
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
                .filter(|v| !v.is_null() && v.as_object().is_some_and(|o| !o.is_empty()))
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
        "markdown" | "skill" => {
            use apcore_toolkit::{
                format_module as toolkit_format_module, FormatOutput, ModuleStyle,
            };
            let style = if format == "skill" {
                ModuleStyle::Skill
            } else {
                ModuleStyle::Markdown
            };
            let scanned = descriptor_to_scanned(module);
            match toolkit_format_module(&scanned, style, true) {
                FormatOutput::Text(s) => s,
                other => format!("{:?}", other),
            }
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

/// Apply field selection to a JSON object.
///
/// `fields` is a comma-separated list of dot-paths (e.g. `"status,data.count"`).
/// Returns a new object containing only the selected fields.
fn apply_field_selection(result: &Value, fields: &str) -> Value {
    if let Some(obj) = result.as_object() {
        let mut selected = serde_json::Map::new();
        for field in fields.split(',') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let mut val: &Value = &Value::Object(obj.clone());
            for part in field.split('.') {
                if let Some(next) = val.get(part) {
                    val = next;
                } else {
                    val = &Value::Null;
                    break;
                }
            }
            selected.insert(field.to_string(), val.clone());
        }
        Value::Object(selected)
    } else {
        result.clone()
    }
}

/// Render a module execution result.
///
/// # Arguments
/// * `result` — `serde_json::Value` (the `output` field from the executor response)
/// * `format` — `"table"`, `"json"`, `"csv"`, `"yaml"`, or `"jsonl"`
/// * `fields` — optional comma-separated dot-paths to select from the result
pub fn format_exec_result(result: &Value, format: &str, fields: Option<&str>) -> String {
    use comfy_table::{ContentArrangement, Table};

    let result = if let Some(f) = fields {
        apply_field_selection(result, f)
    } else {
        result.clone()
    };

    match &result {
        Value::Null => String::new(),

        Value::String(s) => s.clone(),

        _ if format == "csv" => {
            // Delegate to apcore-toolkit for byte-equivalent cross-SDK output.
            // Toolkit's format_csv: header = union of keys across all rows
            // (fixes the prior single-row-keys data-loss bug).
            match rows_for_tabular(&result) {
                Some(rows) => {
                    // Trim trailing CRLF for compatibility with the existing
                    // caller convention (which appends its own newline).
                    apcore_toolkit::format_csv(&rows, false)
                        .trim_end_matches("\r\n")
                        .to_string()
                }
                None => serde_json::to_string(&result).unwrap_or_default(),
            }
        }

        _ if format == "yaml" => serde_yaml_ng::to_string(&result)
            .map(|s| s.trim_end().to_string())
            .unwrap_or_else(|_| {
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "null".to_string())
            }),

        _ if format == "jsonl" => match rows_for_tabular(&result) {
            Some(rows) => apcore_toolkit::format_jsonl(&rows)
                .trim_end_matches('\n')
                .to_string(),
            None => serde_json::to_string(&result).unwrap_or_default(),
        },

        Value::Object(_) if format == "table" => {
            let obj = result.as_object().unwrap();
            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec!["Key", "Value"]);
            for (k, v) in obj {
                let val_str = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                table.add_row(vec![k.clone(), val_str]);
            }
            table.to_string()
        }

        Value::Object(_) | Value::Array(_) => {
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "null".to_string())
        }

        // Number, Bool -- convert to display string.
        other => other.to_string(),
    }
}

/// What (if anything) should be written to stdout for an exec result.
///
/// Pure decision split out of [`print_exec_result`] so the empty-vs-non-empty
/// branch — the exact thing the null-result bug got wrong — is unit
/// testable without capturing real stdout: `format_exec_result` returns `""`
/// for `Value::Null`, and printing that via an unconditional `println!` would
/// still emit a stray newline byte. Returns `None` when nothing should be
/// printed.
fn exec_result_to_print(result: &Value, format: &str, fields: Option<&str>) -> Option<String> {
    let formatted = format_exec_result(result, format, fields);
    if formatted.is_empty() {
        None
    } else {
        Some(formatted)
    }
}

/// Print a module execution result to stdout.
///
/// This is the single interface every call site should use instead of
/// `println!("{}", format_exec_result(...))` — a `Value::Null` result formats
/// to `""`, and an unconditional `println!` around that still writes one
/// newline byte, breaking the documented byte-for-byte stdout equivalence
/// with the Python and TypeScript SDKs (both write zero bytes for a null exec
/// result). Centralizing the print here means a new call site can't
/// reintroduce that bug.
pub fn print_exec_result(result: &Value, format: &str, fields: Option<&str>) {
    if let Some(s) = exec_result_to_print(result, format, fields) {
        println!("{s}");
    }
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
        let modules =
            vec![json!({"module_id": "math.add", "description": "Add numbers", "tags": ["math"]})];
        let output = format_module_list(&modules, "table", &[]);
        assert!(output.contains("math.add"), "table must contain module ID");
        assert!(
            output.contains("Add numbers"),
            "table must contain description"
        );
    }

    #[test]
    fn test_format_module_list_table_columns() {
        let modules =
            vec![json!({"module_id": "math.add", "description": "Add numbers", "tags": []})];
        let output = format_module_list(&modules, "table", &[]);
        assert!(output.contains("ID"), "table must have ID column");
        assert!(
            output.contains("Description"),
            "table must have Description column"
        );
        assert!(output.contains("Tags"), "table must have Tags column");
    }

    // --- format_module_list_with_options (issues #6 / #8) ---

    #[test]
    fn test_format_module_list_with_options_deps_column_table() {
        let modules = vec![json!({
            "module_id": "a.b",
            "description": "Desc",
            "tags": [],
            "dependencies": ["x", "y"]
        })];
        let opts = ListDisplayOptions {
            show_deps: true,
            exposure_filter: None,
        };
        let output = format_module_list_with_options(&modules, "table", &[], &opts);
        assert!(
            output.contains("Deps"),
            "table must have Deps column header"
        );
        assert!(
            output.contains('2'),
            "table must show the dependency count, got:\n{output}"
        );
    }

    #[test]
    fn test_format_module_list_default_wrapper_has_no_deps_column() {
        // format_module_list (the pre-existing, unchanged signature) must
        // never show the new optional column -- proves the wrapper is a
        // true no-op delegate, not a behavior change for the ~18 existing
        // call sites.
        let modules = vec![json!({
            "module_id": "a.b",
            "description": "Desc",
            "tags": [],
            "dependencies": ["x", "y", "z"]
        })];
        let output = format_module_list(&modules, "table", &[]);
        assert!(!output.contains("Deps"), "got:\n{output}");
    }

    #[test]
    fn test_format_module_list_with_options_exposure_column_table() {
        let modules = vec![
            json!({"module_id": "admin.users", "description": "Admin", "tags": []}),
            json!({"module_id": "public.ping", "description": "Ping", "tags": []}),
        ];
        let filter = crate::exposure::ExposureFilter::new("exclude", &[], &["admin.*".to_string()]);
        let opts = ListDisplayOptions {
            show_deps: false,
            exposure_filter: Some(&filter),
        };
        let output = format_module_list_with_options(&modules, "table", &[], &opts);
        assert!(
            output.contains("Exposure"),
            "table must have Exposure column header, got:\n{output}"
        );
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
        let modules = vec![json!({"module_id": "x.y", "description": long_desc, "tags": []})];
        let output = format_module_list(&modules, "table", &[]);
        assert!(
            output.contains("..."),
            "long description must be truncated with '...'"
        );
        assert!(
            !output.contains(&"a".repeat(100)),
            "full description must not appear"
        );
    }

    #[test]
    fn test_format_module_list_json_tags_present() {
        let modules = vec![json!({"module_id": "a.b", "description": "desc", "tags": ["x", "y"]})];
        let output = format_module_list(&modules, "json", &[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let tags = parsed[0]["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], "x");
    }

    // --- format_exec_result ---

    #[test]
    fn test_format_exec_result_null_returns_empty() {
        let output = format_exec_result(&Value::Null, "json", None);
        assert_eq!(output, "", "Null result must produce empty string");
    }

    #[test]
    fn test_format_exec_result_string_plain() {
        let result = json!("hello world");
        let output = format_exec_result(&result, "json", None);
        assert_eq!(output, "hello world");
    }

    #[test]
    fn test_format_exec_result_string_table_mode_also_plain() {
        // Strings are always printed raw, regardless of format.
        let result = json!("hello");
        let output = format_exec_result(&result, "table", None);
        assert_eq!(output, "hello");
    }

    #[test]
    fn test_format_exec_result_object_json_mode() {
        let result = json!({"sum": 42, "status": "ok"});
        let output = format_exec_result(&result, "json", None);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
        assert_eq!(parsed["sum"], 42);
        assert_eq!(parsed["status"], "ok");
    }

    #[test]
    fn test_format_exec_result_object_table_mode() {
        let result = json!({"key": "value", "count": 3});
        let output = format_exec_result(&result, "table", None);
        // Table must contain both keys and their values.
        assert!(output.contains("key"), "table must contain 'key'");
        assert!(output.contains("value"), "table must contain 'value'");
        assert!(output.contains("count"), "table must contain 'count'");
        assert!(output.contains('3'), "table must contain '3'");
    }

    #[test]
    fn test_format_exec_result_array_is_json() {
        let result = json!([1, 2, 3]);
        let output = format_exec_result(&result, "json", None);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("must be valid JSON");
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_format_exec_result_array_table_mode_is_json() {
        // Arrays always render as JSON, even in table mode.
        let result = json!([{"a": 1}, {"b": 2}]);
        let output = format_exec_result(&result, "table", None);
        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("array must produce JSON");
        assert!(parsed.is_array());
    }

    #[test]
    fn test_format_exec_result_number_scalar() {
        let result = json!(42);
        let output = format_exec_result(&result, "json", None);
        assert_eq!(output, "42");
    }

    #[test]
    fn test_format_exec_result_bool_scalar() {
        let result = json!(true);
        let output = format_exec_result(&result, "json", None);
        assert_eq!(output, "true");
    }

    #[test]
    fn test_format_exec_result_float_scalar() {
        let result = json!(3.15);
        let output = format_exec_result(&result, "json", None);
        assert!(output.starts_with("3.15"), "float must stringify correctly");
    }

    // --- exec_result_to_print / print_exec_result ---
    //
    // Regression: a `Value::Null` result formats to "", but a call site that
    // unconditionally wraps `format_exec_result`'s return in `println!` still
    // emits a stray newline byte for that case (see src/cli.rs call sites).
    // `exec_result_to_print` is the pure decision `print_exec_result` acts
    // on, tested directly here since capturing real stdout from `println!`
    // is awkward at the unit level.

    #[test]
    fn test_exec_result_to_print_null_is_none() {
        assert_eq!(
            exec_result_to_print(&Value::Null, "json", None),
            None,
            "a null result must produce nothing to print, not an empty-but-Some string"
        );
    }

    #[test]
    fn test_exec_result_to_print_object_is_some() {
        let result = json!({"sum": 42});
        let printed = exec_result_to_print(&result, "json", None);
        assert!(printed.is_some(), "a non-null result must be printed");
        assert!(printed.unwrap().contains("42"));
    }

    #[test]
    fn test_exec_result_to_print_empty_string_result_is_none() {
        // An empty-string result also formats to "" (the String arm of
        // format_exec_result returns the string verbatim), so it must be
        // treated the same as Null: nothing printed, not a bare newline.
        let result = json!("");
        assert_eq!(exec_result_to_print(&result, "json", None), None);
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
        assert!(
            parsed.get("input_schema").is_some(),
            "input_schema must be present"
        );
        assert!(
            parsed.get("output_schema").is_some(),
            "output_schema must be present"
        );
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
        assert!(
            parsed.get("output_schema").is_none(),
            "output_schema must be absent when not set"
        );
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
        assert!(
            parsed.get("input_schema").is_none(),
            "null input_schema must be absent"
        );
        assert!(parsed.get("tags").is_none(), "null tags must be absent");
    }

    #[test]
    fn test_format_module_detail_table_contains_description() {
        let module = json!({
            "module_id": "math.add",
            "description": "Add two numbers",
        });
        let output = format_module_detail(&module, "table");
        assert!(
            output.contains("Add two numbers"),
            "table must contain description"
        );
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
        assert!(
            output.contains("Input Schema"),
            "table must contain Input Schema section"
        );
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
        assert!(
            output.contains("Annotations"),
            "table must contain Annotations section"
        );
        assert!(
            output.contains("author"),
            "table must contain annotation key"
        );
        assert!(
            output.contains("alice"),
            "table must contain annotation value"
        );
    }

    #[test]
    fn test_format_module_detail_table_extension_metadata() {
        let module = json!({
            "module_id": "a.b",
            "description": "desc",
            "x-category": "utility"
        });
        let output = format_module_detail(&module, "table");
        assert!(
            output.contains("Extension Metadata"),
            "must contain Extension Metadata section"
        );
        assert!(output.contains("x-category"), "must contain x- key");
        assert!(output.contains("utility"), "must contain x- value");
    }

    // ---------------------------------------------------------------------
    // markdown / skill — toolkit delegation (issue #20)
    // ---------------------------------------------------------------------

    fn fixture_module() -> Value {
        json!({
            "module_id": "math.add",
            "description": "Add two numbers and return the sum",
            "tags": ["math"],
            "input_schema": {
                "type": "object",
                "properties": {
                    "a": {"type": "integer", "description": "First operand"},
                    "b": {"type": "integer", "description": "Second operand"}
                },
                "required": ["a", "b"]
            },
            "output_schema": {
                "type": "object",
                "properties": {"sum": {"type": "integer"}},
                "required": ["sum"]
            }
        })
    }

    #[test]
    fn test_format_module_list_markdown_matches_toolkit() {
        use apcore_toolkit::{format_modules, FormatOutput, ModuleStyle};
        let modules = vec![fixture_module()];
        let scanned: Vec<_> = modules.iter().map(descriptor_to_scanned).collect();
        let expected = match format_modules(&scanned, ModuleStyle::Markdown, None, true) {
            FormatOutput::Text(s) => s,
            _ => panic!("expected text"),
        };
        let got = format_module_list(&modules, "markdown", &[]);
        assert_eq!(got, expected);
    }

    #[test]
    fn test_format_module_list_skill_matches_toolkit() {
        use apcore_toolkit::{format_modules, FormatOutput, ModuleStyle};
        let modules = vec![fixture_module()];
        let scanned: Vec<_> = modules.iter().map(descriptor_to_scanned).collect();
        let expected = match format_modules(&scanned, ModuleStyle::Skill, None, true) {
            FormatOutput::Text(s) => s,
            _ => panic!("expected text"),
        };
        let got = format_module_list(&modules, "skill", &[]);
        assert_eq!(got, expected);
    }

    #[test]
    fn test_format_module_detail_markdown_matches_toolkit() {
        use apcore_toolkit::{format_module as toolkit_fmt, FormatOutput, ModuleStyle};
        let m = fixture_module();
        let scanned = descriptor_to_scanned(&m);
        let expected = match toolkit_fmt(&scanned, ModuleStyle::Markdown, true) {
            FormatOutput::Text(s) => s,
            _ => panic!("expected text"),
        };
        let got = format_module_detail(&m, "markdown");
        assert_eq!(got, expected);
    }

    #[test]
    fn test_format_module_detail_skill_emits_yaml_frontmatter() {
        let m = fixture_module();
        let got = format_module_detail(&m, "skill");
        assert!(
            got.starts_with("---\n"),
            "skill output must start with YAML --- delimiter"
        );
        let lines: Vec<&str> = got.split('\n').collect();
        assert!(lines.len() > 3);
        assert!(lines[1].starts_with("name: math.add"));
        assert!(lines[2].starts_with("description:"));
        assert_eq!(lines[3], "---");
    }

    #[test]
    fn test_format_module_detail_skill_matches_toolkit() {
        use apcore_toolkit::{format_module as toolkit_fmt, FormatOutput, ModuleStyle};
        let m = fixture_module();
        let scanned = descriptor_to_scanned(&m);
        let expected = match toolkit_fmt(&scanned, ModuleStyle::Skill, true) {
            FormatOutput::Text(s) => s,
            _ => panic!("expected text"),
        };
        let got = format_module_detail(&m, "skill");
        assert_eq!(got, expected);
    }
}
