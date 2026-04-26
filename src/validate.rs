// apcore-cli -- Standalone validate command (FE-11 / F1: Dry-Run).
// Runs preflight checks without executing the module.

use clap::{Arg, Command};
use serde_json::Value;
use std::io::IsTerminal;

// ---------------------------------------------------------------------------
// Preflight result formatting
// ---------------------------------------------------------------------------

/// Check-name to exit code mapping for the first failed check.
///
/// `pub(crate)` so cli.rs's dispatch_module dry-run path (D9-004) can share
/// the same exit-code mapping the standalone `validate` subcommand uses.
pub(crate) fn first_failed_exit_code(checks: &[Value]) -> i32 {
    let check_to_exit = |check: &str| -> i32 {
        match check {
            "module_id" => crate::EXIT_INVALID_INPUT,
            "module_lookup" => crate::EXIT_MODULE_NOT_FOUND,
            "call_chain" => crate::EXIT_MODULE_EXECUTE_ERROR,
            "acl" => crate::EXIT_ACL_DENIED,
            "schema" => crate::EXIT_SCHEMA_VALIDATION_ERROR,
            "approval" => crate::EXIT_APPROVAL_DENIED,
            "module_preflight" => crate::EXIT_MODULE_EXECUTE_ERROR,
            _ => crate::EXIT_MODULE_EXECUTE_ERROR,
        }
    };

    for c in checks {
        let passed = c.get("passed").and_then(|v| v.as_bool()).unwrap_or(true);
        if !passed {
            let check = c.get("check").and_then(|v| v.as_str()).unwrap_or("");
            return check_to_exit(check);
        }
    }
    crate::EXIT_MODULE_EXECUTE_ERROR
}

/// Format and print a preflight result (from executor.validate).
pub fn format_preflight_result(result: &Value, format: Option<&str>) {
    let fmt = crate::output::resolve_format(format);
    let valid = result
        .get("valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let requires_approval = result
        .get("requires_approval")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let checks = result
        .get("checks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if fmt == "json" || !std::io::stdout().is_terminal() {
        let mut payload = serde_json::Map::new();
        payload.insert("valid".to_string(), Value::Bool(valid));
        payload.insert(
            "requires_approval".to_string(),
            Value::Bool(requires_approval),
        );
        let checks_json: Vec<Value> = checks
            .iter()
            .map(|c| {
                let mut entry = serde_json::Map::new();
                if let Some(check) = c.get("check") {
                    entry.insert("check".to_string(), check.clone());
                }
                if let Some(passed) = c.get("passed") {
                    entry.insert("passed".to_string(), passed.clone());
                }
                if let Some(error) = c.get("error") {
                    if !error.is_null() {
                        entry.insert("error".to_string(), error.clone());
                    }
                }
                if let Some(warnings) = c.get("warnings") {
                    if let Some(arr) = warnings.as_array() {
                        if !arr.is_empty() {
                            entry.insert("warnings".to_string(), warnings.clone());
                        }
                    }
                }
                Value::Object(entry)
            })
            .collect();
        payload.insert("checks".to_string(), Value::Array(checks_json));
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(payload))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        // TTY table format
        for c in &checks {
            let passed = c.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
            let check = c.get("check").and_then(|v| v.as_str()).unwrap_or("?");
            let has_warnings = c
                .get("warnings")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty());
            // Spec symbols: v=passed, !=warning, x=failed, o=skipped
            let sym = if passed && has_warnings {
                "!"
            } else if passed {
                "v"
            } else {
                "x"
            };
            let error = c.get("error");
            let detail = if let Some(err) = error {
                if err.is_null() {
                    if passed && !has_warnings {
                        " OK".to_string()
                    } else if !passed {
                        " Skipped".to_string()
                    } else {
                        String::new()
                    }
                } else if let Some(s) = err.as_str() {
                    format!(" {s}")
                } else {
                    format!(" {err}")
                }
            } else if passed && !has_warnings {
                " OK".to_string()
            } else if !passed {
                " Skipped".to_string()
            } else {
                String::new()
            };
            println!("  {sym} {check:<20}{detail}");

            if let Some(warnings) = c.get("warnings").and_then(|v| v.as_array()) {
                for w in warnings {
                    let wstr = w.as_str().unwrap_or("?");
                    println!("    Warning: {wstr}");
                }
            }
        }

        let error_count = checks
            .iter()
            .filter(|c| !c.get("passed").and_then(|v| v.as_bool()).unwrap_or(true))
            .count();
        let warning_count: usize = checks
            .iter()
            .map(|c| {
                c.get("warnings")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0)
            })
            .sum();
        let tag = if valid { "PASS" } else { "FAIL" };
        println!("\nResult: {tag} ({error_count} error(s), {warning_count} warning(s))");
    }
}

// ---------------------------------------------------------------------------
// Command builder
// ---------------------------------------------------------------------------

/// Build the `validate` clap subcommand.
pub fn validate_command() -> Command {
    Command::new("validate")
        .about("Run preflight checks without executing a module")
        .arg(
            Arg::new("module_id")
                .required(true)
                .value_name("MODULE_ID")
                .help("Module ID to validate."),
        )
        .arg(
            Arg::new("input")
                .long("input")
                .value_name("SOURCE")
                .help("JSON input file or '-' for stdin."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

/// Register the validate subcommand on the root command.
pub fn register_validate_command(cli: Command) -> Command {
    cli.subcommand(validate_command())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Build a preflight result for `module_def` against `input`.
///
/// Calls `system.validate` via the apcore executor when available; on failure
/// (module not registered, internal error) constructs a synthetic preflight
/// JSON shape with the same `{ checks: [...], valid, requires_approval }`
/// schema so callers can format and exit uniformly.
///
/// Used by both [`dispatch_validate`] (the standalone `validate` subcommand)
/// and the `--dry-run` branch of `dispatch_module` in cli.rs (D9-004 — was
/// previously two parallel implementations). Caller is responsible for
/// running `validate_module_id` and `get_module_descriptor` lookups before
/// calling this — the returned preflight assumes the module exists and the
/// id has the right shape.
pub async fn build_preflight_result(
    apcore_executor: &apcore::Executor,
    module_def: &apcore::registry::registry::ModuleDescriptor,
    input: &Value,
) -> Value {
    let preflight_input = serde_json::json!({
        "module_id": module_def.module_id,
        "input": input,
    });

    match apcore_executor
        .call("system.validate", preflight_input, None, None)
        .await
    {
        Ok(preflight) => preflight,
        Err(e) => {
            tracing::debug!(
                "system.validate call failed: {e}; falling back to basic schema validation"
            );
            // Synthetic preflight with the same shape system.validate emits.
            // module_id and module_lookup are passed because the caller has
            // already validated those.
            let merged: std::collections::HashMap<String, Value> = match input.as_object() {
                Some(obj) => obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                None => std::collections::HashMap::new(),
            };
            let schema_passed = if let Some(schema_obj) = module_def.input_schema.as_object() {
                if schema_obj.contains_key("properties") {
                    crate::cli::validate_against_schema(&merged, &module_def.input_schema).is_ok()
                } else {
                    true
                }
            } else {
                true
            };

            let checks = vec![
                serde_json::json!({"check": "module_id", "passed": true}),
                serde_json::json!({"check": "module_lookup", "passed": true}),
                serde_json::json!({"check": "schema", "passed": schema_passed}),
            ];

            let requires_approval = module_def
                .annotations
                .as_ref()
                .map(|a| a.requires_approval)
                .unwrap_or(false);

            serde_json::json!({
                "valid": schema_passed,
                "requires_approval": requires_approval,
                "checks": checks,
            })
        }
    }
}

/// Dispatch the `validate` subcommand.
///
/// Calls `executor.validate()` (preflight) and prints the result.
pub async fn dispatch_validate(
    matches: &clap::ArgMatches,
    registry: &std::sync::Arc<dyn crate::discovery::RegistryProvider>,
    apcore_executor: &apcore::Executor,
) {
    let module_id = matches
        .get_one::<String>("module_id")
        .expect("module_id is required");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());

    // Validate module ID.
    if let Err(_e) = crate::cli::validate_module_id(module_id) {
        eprintln!("Error: Invalid module ID format: '{module_id}'.");
        std::process::exit(crate::EXIT_INVALID_INPUT);
    }

    // Check module exists.
    if registry.get_module_descriptor(module_id).is_none() {
        eprintln!("Error: Module '{module_id}' not found.");
        std::process::exit(crate::EXIT_MODULE_NOT_FOUND);
    }

    // Collect input if provided.
    let stdin_flag = matches.get_one::<String>("input").map(|s| s.as_str());
    let merged =
        match crate::cli::collect_input(stdin_flag, std::collections::HashMap::new(), false) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(crate::EXIT_INVALID_INPUT);
            }
        };

    let input_value = serde_json::to_value(&merged).unwrap_or(Value::Object(Default::default()));

    let module_def = match registry.get_module_descriptor(module_id) {
        Some(d) => d,
        None => {
            // Already exited above; defensive guard against caller skipping
            // the existence check.
            eprintln!("Error: Module '{module_id}' not found.");
            std::process::exit(crate::EXIT_MODULE_NOT_FOUND);
        }
    };

    let preflight = build_preflight_result(apcore_executor, &module_def, &input_value).await;
    format_preflight_result(&preflight, format);

    let valid = preflight
        .get("valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if valid {
        std::process::exit(crate::EXIT_SUCCESS);
    } else {
        let checks = preflight
            .get("checks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        std::process::exit(first_failed_exit_code(&checks));
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_command_builder() {
        let cmd = validate_command();
        assert_eq!(cmd.get_name(), "validate");
        let args: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert!(args.contains(&"module_id"));
    }

    #[test]
    fn test_register_validate_command() {
        let root = clap::Command::new("test");
        let root = register_validate_command(root);
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"validate"));
    }

    #[test]
    fn test_first_failed_exit_code_module_lookup() {
        let checks = vec![
            serde_json::json!({"check": "module_id", "passed": true}),
            serde_json::json!({
                "check": "module_lookup",
                "passed": false,
                "error": "not found",
            }),
        ];
        assert_eq!(first_failed_exit_code(&checks), 44);
    }

    #[test]
    fn test_first_failed_exit_code_all_pass() {
        let checks = vec![
            serde_json::json!({"check": "module_id", "passed": true}),
            serde_json::json!({"check": "schema", "passed": true}),
        ];
        // All passed, falls through to default.
        assert_eq!(first_failed_exit_code(&checks), 1);
    }

    #[test]
    fn test_first_failed_exit_code_schema() {
        let checks = vec![serde_json::json!({
            "check": "schema",
            "passed": false,
            "error": "missing field",
        })];
        assert_eq!(first_failed_exit_code(&checks), 45);
    }

    #[test]
    fn test_first_failed_exit_code_acl() {
        let checks = vec![serde_json::json!({
            "check": "acl",
            "passed": false,
        })];
        assert_eq!(first_failed_exit_code(&checks), 77);
    }
}
