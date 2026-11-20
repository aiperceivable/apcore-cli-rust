use std::collections::HashMap;
use std::path::PathBuf;

use apcore_cli::detect_effective_proxy_hazards;
use apcore_toolkit::{OpenAPIScanner, ScanOptions, ScannedModule};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Fixture {
    version: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    module: FixtureModule,
    expected: Vec<Value>,
}

#[derive(Deserialize)]
struct FixtureModule {
    module_id: String,
    input_schema: Value,
    metadata: HashMap<String, Value>,
}

fn fixture_path(filename: &str) -> PathBuf {
    let root = std::env::var_os("APCORE_CLI_SPEC_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../apcore-cli"));
    root.join("conformance/fixtures").join(filename)
}

#[test]
fn test_effective_proxy_hazards_match_shared_wire_contract() {
    let path = fixture_path("openapi_effective_hazards.json");
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
    let fixture: Fixture = serde_json::from_str(&data).expect("parse fixture");
    assert_eq!(fixture.version, 1);
    assert!(!fixture.cases.is_empty());

    for case in fixture.cases {
        let mut module = ScannedModule::new(
            case.module.module_id,
            String::new(),
            case.module.input_schema,
            json!({}),
            vec![],
            String::new(),
        );
        module.metadata = case.module.metadata;
        let mut actual: Vec<Value> = detect_effective_proxy_hazards(&[module])
            .into_iter()
            .map(|hazard| serde_json::to_value(hazard).expect("serialize diagnostic"))
            .collect();
        for (actual, expected) in actual.iter_mut().zip(&case.expected) {
            if expected["kind"]["code"] == "invalid_effective_inputs"
                && expected["kind"].get("detail").is_none()
            {
                let detail = actual["kind"].as_object_mut().unwrap().remove("detail");
                assert!(
                    detail
                        .as_ref()
                        .and_then(Value::as_str)
                        .is_some_and(|detail| !detail.trim().is_empty()),
                    "{} must report an invalid metadata detail",
                    case.id
                );
            }
        }
        assert_eq!(actual, case.expected, "{}", case.id);
    }
}

#[derive(Deserialize)]
struct DocumentFixture {
    version: u32,
    cases: Vec<DocumentCase>,
}

#[derive(Deserialize)]
struct DocumentCase {
    id: String,
    document: Value,
    include: Option<String>,
    expected: DocumentExpected,
}

#[derive(Deserialize)]
struct DocumentExpected {
    modules: Vec<DocumentModule>,
    diagnostics: Vec<Value>,
}

#[derive(Deserialize)]
struct DocumentModule {
    id: String,
    properties: Option<Value>,
    required: Option<Value>,
    output_schema: Option<Value>,
}

#[tokio::test]
async fn test_effective_openapi_documents_match_shared_contract() {
    let path = fixture_path("openapi_effective_documents.json");
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
    let fixture: DocumentFixture = serde_json::from_str(&data).expect("parse fixture");
    assert_eq!(fixture.version, 1);
    assert!(!fixture.cases.is_empty());

    for case in fixture.cases {
        let mut options = ScanOptions::new();
        options.include = case.include;
        let mut modules = OpenAPIScanner::new()
            .scan_with_effective_parameters(&case.document, &options)
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", case.id));
        modules.sort_by(|left, right| left.module_id.cmp(&right.module_id));
        assert_eq!(modules.len(), case.expected.modules.len(), "{}", case.id);
        for (module, expected) in modules.iter().zip(case.expected.modules) {
            assert_eq!(module.module_id, expected.id, "{}", case.id);
            if let Some(properties) = expected.properties {
                assert_eq!(module.input_schema["properties"], properties, "{}", case.id);
            }
            if let Some(required) = expected.required {
                assert_eq!(module.input_schema["required"], required, "{}", case.id);
            }
            if let Some(expected_output) = expected.output_schema {
                assert_eq!(module.output_schema, expected_output, "{}", case.id);
            }
        }
        let actual = serde_json::to_value(detect_effective_proxy_hazards(&modules)).unwrap();
        assert_eq!(actual, json!(case.expected.diagnostics), "{}", case.id);
    }
}
