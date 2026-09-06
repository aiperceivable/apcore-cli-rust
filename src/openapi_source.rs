// apcore-cli -- OpenAPI document loading and proxy-hazard detection
// (FE-15a sections 4.1 and 4.3).
//
// The CLI is an adapter, not a second implementation (spec section 1.2):
// module-ID derivation, schema extraction and the two-key execution contract
// (`http_method` / `url_path`) all belong to apcore-toolkit and are consumed,
// never re-derived, here.

use std::collections::HashMap;

use apcore_toolkit::{load_spec_with_options, LoadSpecError, LoadSpecOptions, ScannedModule};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default fetch timeout, in **seconds**.
///
/// `load_spec`'s timeout is seconds in Python and Rust and milliseconds in
/// TypeScript, and `HTTPProxyRegistryWriter` defaults to 60s where `load_spec`
/// defaults to 30s. The CLI exposes a single `--openapi-timeout SECS` in
/// seconds in all three SDKs and converts at the call boundary, so a user does
/// not have to know which SDK they are running (spec section 4.1).
pub const DEFAULT_OPENAPI_TIMEOUT_SECS: f64 = 30.0;

/// HTTP methods whose non-path inputs `HTTPProxyRegistryWriter` sends as a
/// JSON body, and which therefore misroute a declared query parameter.
pub const BODY_METHODS: &[&str] = &["post", "put", "patch"];

/// Message used when a build cannot reach the HTTP path.
///
/// In this SDK `apcore-toolkit` is declared as
/// `features = ["http-proxy"]`, so `load_spec` is always linked and this
/// message has no live code path -- it exists so the string is pinned by a
/// test and stays available if the feature is ever made optional again
/// (spec section 4.6 / T-OAPI-15).
pub const HTTP_SUPPORT_MISSING: &str =
    "HTTP sources require the apcore-toolkit 'http-proxy' feature; \
     rebuild with `apcore-toolkit = { features = [\"http-proxy\"] }`.";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure loading an OpenAPI document. Every variant exits 47
/// ([`crate::EXIT_CONFIG_NOT_FOUND`]) per the FE-15a section 6 error table.
#[derive(Debug, thiserror::Error)]
pub enum OpenApiSourceError {
    // `source` is reserved by `thiserror` for the error-source field, so the
    // document locator is spelled `source_name` here. The rendered message
    // still reads "source", which is the spec's wording.
    /// The source could not be read or fetched.
    #[error("Cannot read OpenAPI source '{source_name}': {detail}")]
    Unreadable { source_name: String, detail: String },
    /// The document was retrieved but is not valid JSON or YAML.
    #[error("Cannot parse OpenAPI source '{source_name}': {detail}")]
    Unparseable { source_name: String, detail: String },
}

impl OpenApiSourceError {
    /// Every load failure is a configuration fault, not an execution one.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        crate::EXIT_CONFIG_NOT_FOUND
    }
}

/// Failure parsing a `--header "Key: Value"` value.
#[derive(Debug, thiserror::Error)]
#[error("Invalid --header value '{0}': expected \"Key: Value\".")]
pub struct HeaderParseError(pub String);

// ---------------------------------------------------------------------------
// Header parsing
// ---------------------------------------------------------------------------

/// Parse repeated `--header "Key: Value"` values into a header map.
///
/// Splits on the **first** colon so header values may themselves contain
/// colons (`Authorization: Bearer a:b`).
pub fn parse_headers(headers: &[String]) -> Result<HashMap<String, String>, HeaderParseError> {
    let mut out = HashMap::new();
    for raw in headers {
        let Some((name, value)) = raw.split_once(':') else {
            return Err(HeaderParseError(raw.clone()));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(HeaderParseError(raw.clone()));
        }
        out.insert(name.to_string(), value.trim().to_string());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// load_openapi_source
// ---------------------------------------------------------------------------

/// Load and parse an OpenAPI document from a local path or `http(s)://` URL.
///
/// The source is taken **verbatim**. The CLI MUST NOT probe candidate paths
/// (`/openapi.json`, `/v3/api-docs`, ...): a wrong URL must produce an honest
/// 404 rather than a surprising success against a different document. Format
/// detection is content sniffing, not file extension. All of this is
/// `load_spec`'s behaviour and is delegated to it (spec section 4.1).
///
/// `headers` are ignored for local files, and exist only for this fetch --
/// they are never copied into a generated artifact (spec section 4.4).
///
/// # Errors
///
/// [`OpenApiSourceError`], whose variants carry the two spec-mandated message
/// shapes. Every one exits 47.
pub async fn load_openapi_source(
    source: &str,
    headers: &HashMap<String, String>,
    timeout_secs: f64,
) -> Result<Value, OpenApiSourceError> {
    let options = LoadSpecOptions {
        headers: if headers.is_empty() {
            None
        } else {
            Some(headers.clone())
        },
        auth_header_factory: None,
        timeout_secs,
    };
    load_spec_with_options(source, &options)
        .await
        .map_err(|e| map_load_error(source, e))
}

fn map_load_error(source: &str, err: LoadSpecError) -> OpenApiSourceError {
    match err {
        LoadSpecError::Io(e) => OpenApiSourceError::Unreadable {
            source_name: source.to_string(),
            detail: e.to_string(),
        },
        LoadSpecError::Http(e) => {
            let detail = match e.status() {
                Some(status) => format!("HTTP {}", status.as_u16()),
                None => e.to_string(),
            };
            OpenApiSourceError::Unreadable {
                source_name: source.to_string(),
                detail,
            }
        }
        LoadSpecError::Json(e) => OpenApiSourceError::Unparseable {
            source_name: source.to_string(),
            detail: e.to_string(),
        },
        LoadSpecError::Yaml { error, .. } => OpenApiSourceError::Unparseable {
            source_name: source.to_string(),
            detail: error.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// detect_proxy_hazards
// ---------------------------------------------------------------------------

/// One operation whose parameters FE-15b's proxy writer would misroute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hazard {
    /// The module ID the scanner derived for the affected operation.
    pub module_id: String,
    /// Uppercase HTTP method.
    pub http_method: String,
    /// The document's path template, braces retained.
    pub url_path: String,
    /// Names of the `in: query` parameters declared on a body method.
    pub parameters: Vec<String>,
}

impl Hazard {
    /// JSON form, used under the top-level `hazards` key of machine formats.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "module_id": self.module_id,
            "http_method": self.http_method,
            "url_path": self.url_path,
            "parameters": self.parameters,
        })
    }

    /// One-line human summary, e.g. `POST with 2 'in: query' parameters`.
    #[must_use]
    pub fn summary(&self) -> String {
        let plural = if self.parameters.len() == 1 {
            "parameter"
        } else {
            "parameters"
        };
        format!(
            "{} with {} 'in: query' {plural}: {}",
            self.http_method,
            self.parameters.len(),
            self.parameters.join(", ")
        )
    }
}

/// Report operations that FE-15b would be unable to proxy correctly.
///
/// `HTTPProxyRegistryWriter` decides body-versus-query by HTTP method alone:
/// `POST` / `PUT` / `PATCH` send every non-path input as a JSON body,
/// everything else as a query string. It has no parameter-location
/// information to do otherwise, because `OpenAPIScanner` deliberately does not
/// record one. The consequence is that a query parameter declared on a body
/// method **would be sent in the request body**, and the failure is silent.
///
/// FE-15a cannot fix that, but it can make it visible: the CLI holds the raw
/// document, which still carries `parameters[].in`. This is a diagnostic, not
/// a routing decision -- no toolkit logic is duplicated (spec section 4.3).
///
/// Never raises. A malformed `parameters` entry yields no hazard rather than
/// an error.
#[must_use]
pub fn detect_proxy_hazards(spec: &Value, modules: &[ScannedModule]) -> Vec<Hazard> {
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut hazards = Vec::new();
    for (path, path_item) in paths {
        let Some(operations) = path_item.as_object() else {
            continue;
        };
        for (method, operation) in operations {
            let lowered = method.to_ascii_lowercase();
            if !BODY_METHODS.contains(&lowered.as_str()) {
                // Query is the correct encoding for GET / DELETE / HEAD /
                // OPTIONS / TRACE, so those are never hazardous.
                continue;
            }
            let query_params = query_parameter_names(operation);
            if query_params.is_empty() {
                continue;
            }
            let http_method = lowered.to_ascii_uppercase();
            for module_id in correlate_modules(modules, &http_method, path) {
                hazards.push(Hazard {
                    module_id,
                    http_method: http_method.clone(),
                    url_path: path.clone(),
                    parameters: query_params.clone(),
                });
            }
        }
    }
    hazards.sort_by(|a, b| a.module_id.cmp(&b.module_id));
    hazards
}

/// Names of the operation's `in: query` parameters, in document order.
///
/// A `$ref`'d parameter carries no `in` key at this level and is skipped: the
/// toolkit refuses external `$ref`s and this diagnostic must not become a
/// resolver.
fn query_parameter_names(operation: &Value) -> Vec<String> {
    let Some(parameters) = operation.get("parameters").and_then(Value::as_array) else {
        return Vec::new();
    };
    parameters
        .iter()
        .filter(|p| p.get("in").and_then(Value::as_str) == Some("query"))
        .filter_map(|p| p.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Module IDs whose routing metadata points at this method and path.
///
/// Correlation reads the toolkit's own `http_method` / `url_path` metadata
/// rather than re-deriving an ID, so a `--prefix` or a deduplication suffix
/// applied by the scanner is honoured automatically.
fn correlate_modules(modules: &[ScannedModule], http_method: &str, path: &str) -> Vec<String> {
    modules
        .iter()
        .filter(|m| {
            m.metadata.get("http_method").and_then(Value::as_str) == Some(http_method)
                && m.metadata.get("url_path").and_then(Value::as_str) == Some(path)
        })
        .map(|m| m.module_id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn module(id: &str, method: &str, path: &str) -> ScannedModule {
        let mut m = ScannedModule::new(
            id.to_string(),
            String::new(),
            json!({"type": "object"}),
            json!({"type": "object"}),
            vec![],
            format!("{method} {path}"),
        );
        m.metadata
            .insert("http_method".to_string(), Value::from(method));
        m.metadata.insert("url_path".to_string(), Value::from(path));
        m
    }

    // ----- parse_headers -----

    #[test]
    fn headers_split_on_the_first_colon() {
        let parsed = parse_headers(&["Authorization: Bearer a:b".to_string()]).expect("parses");
        assert_eq!(
            parsed.get("Authorization").map(String::as_str),
            Some("Bearer a:b")
        );
    }

    #[test]
    fn header_without_a_colon_is_rejected() {
        assert!(parse_headers(&["nonsense".to_string()]).is_err());
        assert!(parse_headers(&[": value".to_string()]).is_err());
    }

    #[test]
    fn empty_header_list_yields_an_empty_map() {
        assert!(parse_headers(&[]).expect("parses").is_empty());
    }

    // ----- load_openapi_source -----

    #[tokio::test]
    async fn missing_local_file_is_unreadable() {
        let err = load_openapi_source("/nonexistent/openapi.yaml", &HashMap::new(), 1.0)
            .await
            .expect_err("missing file");
        assert!(
            err.to_string()
                .starts_with("Cannot read OpenAPI source '/nonexistent/openapi.yaml': "),
            "unexpected message: {err}"
        );
        assert_eq!(err.exit_code(), crate::EXIT_CONFIG_NOT_FOUND);
    }

    #[tokio::test]
    async fn malformed_yaml_is_unparseable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.yaml");
        std::fs::write(&path, "openapi: 3.1.0\n  bad indent: [\n").expect("write");
        let err = load_openapi_source(path.to_str().unwrap(), &HashMap::new(), 1.0)
            .await
            .expect_err("malformed YAML");
        assert!(
            err.to_string().starts_with("Cannot parse OpenAPI source '"),
            "unexpected message: {err}"
        );
    }

    #[tokio::test]
    async fn json_and_yaml_sources_parse_to_the_same_document() {
        let dir = tempfile::tempdir().expect("tempdir");
        let yaml = dir.path().join("spec.yaml");
        let json_path = dir.path().join("spec.json");
        std::fs::write(
            &yaml,
            "openapi: 3.1.0\ninfo:\n  title: T\n  version: '1'\npaths: {}\n",
        )
        .expect("write");
        std::fs::write(
            &json_path,
            r#"{"openapi":"3.1.0","info":{"title":"T","version":"1"},"paths":{}}"#,
        )
        .expect("write");
        let from_yaml = load_openapi_source(yaml.to_str().unwrap(), &HashMap::new(), 1.0)
            .await
            .expect("yaml parses");
        let from_json = load_openapi_source(json_path.to_str().unwrap(), &HashMap::new(), 1.0)
            .await
            .expect("json parses by content sniffing");
        assert_eq!(from_yaml, from_json);
    }

    // ----- detect_proxy_hazards -----

    #[test]
    fn post_with_query_parameters_is_a_hazard() {
        let spec = json!({
            "openapi": "3.1.0",
            "paths": {
                "/pets": {
                    "post": {
                        "operationId": "createPets",
                        "parameters": [
                            {"name": "dry_run", "in": "query"},
                            {"name": "trace", "in": "query"},
                            {"name": "X-Tenant", "in": "header"}
                        ]
                    }
                }
            }
        });
        let modules = vec![module("createPets", "POST", "/pets")];
        let hazards = detect_proxy_hazards(&spec, &modules);
        assert_eq!(hazards.len(), 1);
        assert_eq!(hazards[0].module_id, "createPets");
        assert_eq!(hazards[0].http_method, "POST");
        assert_eq!(hazards[0].url_path, "/pets");
        assert_eq!(hazards[0].parameters, vec!["dry_run", "trace"]);
        assert!(hazards[0].summary().contains("2 'in: query' parameters"));
    }

    #[test]
    fn get_with_query_parameters_is_not_a_hazard() {
        // Query is the correct encoding for GET, so nothing is reported.
        let spec = json!({
            "paths": {
                "/pets": {
                    "get": {"parameters": [{"name": "limit", "in": "query"}]}
                }
            }
        });
        let modules = vec![module("listPets", "GET", "/pets")];
        assert!(detect_proxy_hazards(&spec, &modules).is_empty());
    }

    #[test]
    fn put_and_patch_are_body_methods_too() {
        let spec = json!({
            "paths": {
                "/pets/{id}": {
                    "put": {"parameters": [{"name": "force", "in": "query"}]},
                    "patch": {"parameters": [{"name": "force", "in": "query"}]}
                }
            }
        });
        let modules = vec![
            module("replacePet", "PUT", "/pets/{id}"),
            module("patchPet", "PATCH", "/pets/{id}"),
        ];
        let hazards = detect_proxy_hazards(&spec, &modules);
        assert_eq!(hazards.len(), 2);
        assert_eq!(hazards[0].module_id, "patchPet");
        assert_eq!(hazards[1].module_id, "replacePet");
    }

    #[test]
    fn path_and_body_parameters_are_not_hazards() {
        let spec = json!({
            "paths": {
                "/pets/{id}": {
                    "post": {"parameters": [
                        {"name": "id", "in": "path"},
                        {"name": "X-Trace", "in": "header"},
                        {"name": "session", "in": "cookie"}
                    ]}
                }
            }
        });
        let modules = vec![module("createPet", "POST", "/pets/{id}")];
        assert!(detect_proxy_hazards(&spec, &modules).is_empty());
    }

    #[test]
    fn malformed_documents_yield_no_hazard_and_no_panic() {
        for spec in [
            json!({}),
            json!({"paths": "not-an-object"}),
            json!({"paths": {"/x": 7}}),
            json!({"paths": {"/x": {"post": {"parameters": "nope"}}}}),
            json!({"paths": {"/x": {"post": {"parameters": [7, null, {"in": "query"}]}}}}),
            json!({"paths": {"/x": {"post": {"parameters": [{"$ref": "#/c/p/Q"}]}}}}),
        ] {
            assert!(
                detect_proxy_hazards(&spec, &[module("m", "POST", "/x")]).is_empty(),
                "spec must not produce a hazard: {spec}"
            );
        }
    }

    #[test]
    fn hazards_correlate_by_routing_metadata_not_by_re_derivation() {
        // A `--prefix api` scan produces `api.createPets`; correlation reads
        // metadata, so the prefixed ID is reported verbatim.
        let spec = json!({
            "paths": {"/pets": {"post": {"parameters": [{"name": "q", "in": "query"}]}}}
        });
        let modules = vec![module("api.createPets", "POST", "/pets")];
        let hazards = detect_proxy_hazards(&spec, &modules);
        assert_eq!(hazards.len(), 1);
        assert_eq!(hazards[0].module_id, "api.createPets");
    }

    #[test]
    fn hazard_json_shape() {
        let h = Hazard {
            module_id: "createPets".to_string(),
            http_method: "POST".to_string(),
            url_path: "/pets".to_string(),
            parameters: vec!["q".to_string()],
        };
        let v = h.to_json();
        assert_eq!(v["module_id"], "createPets");
        assert_eq!(v["http_method"], "POST");
        assert_eq!(v["url_path"], "/pets");
        assert_eq!(v["parameters"][0], "q");
    }

    // ----- HTTP feature availability (T-OAPI-15, Rust form) -----

    #[test]
    fn http_sources_are_always_linkable_in_this_sdk() {
        // `load_spec` sits behind apcore-toolkit's `http-proxy` feature, which
        // this crate declares unconditionally, so the "HTTP support missing"
        // branch of the spec's error table has no live code path here. The
        // *feature* declaration is the thing to pin.
        //
        // The version floor deliberately does NOT appear in the assertion. An
        // earlier form matched the whole dependency line verbatim, including
        // `version = ">=0.11.0"`, so every routine floor bump failed a test
        // about HTTP support -- a false alarm that says nothing about whether
        // `load_spec` is linkable. The floor is Cargo's business; the feature
        // is this test's.
        let manifest = include_str!("../Cargo.toml");
        let declaration = manifest
            .lines()
            .find(|line| line.starts_with("apcore-toolkit = "))
            .expect("apcore-toolkit must be a direct dependency");
        assert!(
            declaration.contains(r#"features = ["http-proxy"]"#),
            "apcore-toolkit must declare the http-proxy feature: load_spec is gated on it \
             (found: {declaration})"
        );
        assert!(HTTP_SUPPORT_MISSING.contains("http-proxy"));
    }
}
