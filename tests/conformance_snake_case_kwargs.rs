// Conformance — Algorithm C-SNAKE: snake_case multi-word kwargs flow.
//
// Cross-language fixture lives at
// `../apcore-cli/conformance/fixtures/snake-case-kwargs/cases.json` and is
// shared verbatim with the TypeScript and Python SDK runners.
//
// Targets the schema → clap → kwargs path used by `extract_cli_kwargs` in
// `src/cli.rs`. clap stores arg values under the snake_case `Arg::id`
// (set explicitly by `schema_to_clap_args`), so the Rust SDK is the parity
// reference for the TypeScript fix that reverse-maps commander's
// camelCase keys back to the schema's property names.
use std::collections::HashMap;
use std::path::PathBuf;

use apcore_cli::cli::reconcile_bool_pairs;
use apcore_cli::schema_to_clap_args;
use clap::Command;
use serde_json::Value;

fn spec_repo_root() -> PathBuf {
    if let Ok(p) = std::env::var("APCORE_CLI_SPEC_REPO") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir must have a parent")
        .join("apcore-cli")
}

fn fixture_path() -> PathBuf {
    spec_repo_root().join("conformance/fixtures/snake-case-kwargs/cases.json")
}

fn load_fixture() -> Value {
    let path = fixture_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("parse fixture")
}

/// Run a single case end-to-end: build clap args from the schema, parse the
/// argv, then mirror `extract_cli_kwargs` to produce the input dict.
fn run_case(input_schema: &Value, module_id: &str, args: &[String]) -> HashMap<String, Value> {
    let schema_args = schema_to_clap_args(input_schema, None).expect("schema_to_clap_args");

    let mut cmd = Command::new(module_id.to_string());
    for arg in schema_args.args.iter().cloned() {
        cmd = cmd.arg(arg);
    }

    // `try_get_matches_from` prepends argv[0]; mirror real CLI dispatch.
    let mut argv: Vec<String> = vec![module_id.to_string()];
    argv.extend(args.iter().cloned());
    let matches = cmd
        .try_get_matches_from(argv)
        .unwrap_or_else(|e| panic!("clap parse failed for args={args:?}: {e}"));

    let mut kwargs: HashMap<String, Value> = HashMap::new();

    // Re-derive args (consumed by Command above) so we can read their ids.
    let schema_args2 = schema_to_clap_args(input_schema, None).expect("schema_to_clap_args");
    for arg in &schema_args2.args {
        let id = arg.get_id().as_str().to_string();
        if id.starts_with("no-") {
            continue;
        }
        if let Ok(Some(val)) = matches.try_get_one::<String>(&id) {
            kwargs.insert(id, Value::String(val.clone()));
        }
    }

    let bool_vals = reconcile_bool_pairs(&matches, &schema_args2.bool_pairs);
    for (k, v) in bool_vals {
        kwargs.insert(k, v);
    }

    kwargs
}

#[test]
fn snake_case_kwargs_conformance() {
    let fixture = load_fixture();
    let module_id = fixture["module_id"].as_str().expect("module_id string");
    let input_schema = &fixture["input_schema"];
    let cases = fixture["test_cases"].as_array().expect("test_cases array");

    let mut failures: Vec<String> = vec![];
    for case in cases {
        let id = case["id"].as_str().expect("case id").to_string();
        let args: Vec<String> = case["args"]
            .as_array()
            .expect("case.args array")
            .iter()
            .map(|v| v.as_str().expect("arg string").to_string())
            .collect();
        let expected = case["expected_input"]
            .as_object()
            .expect("expected_input object");

        let actual = run_case(input_schema, module_id, &args);

        for (key, expected_val) in expected {
            match actual.get(key) {
                None => failures.push(format!("[{id}] missing key '{key}'; full input={actual:?}")),
                Some(got) if got != expected_val => failures.push(format!(
                    "[{id}] input['{key}'] = {got:?}, expected {expected_val:?}; \
                     full input={actual:?}"
                )),
                _ => {}
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "Algorithm C-SNAKE conformance failures ({}):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
