// apcore-cli -- FE-15a OpenAPI import integration tests.
//
// Covers the in-scope T-OAPI-* verification matrix from
// `apcore-cli/docs/features/openapi-import.md` section 9: T-OAPI-01..22 and
// 24..27. T-OAPI-23 is withdrawn (the `--writer native` flag was removed
// because no toolkit source writer can resolve an OpenAPI `target`, see spec
// section 4.4), and T-OAPI-30..40 belong to the deferred FE-15b.
//
// The commands are driven end-to-end through the real binary, so the exit
// codes and stream routing asserted here are what a user's shell would see.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use apcore_cli::openapi_cmd::{planned_paths, sanitize_binding_filename, BINDING_SUFFIX};
use apcore_cli::openapi_source::{detect_proxy_hazards, load_openapi_source};
use apcore_toolkit::{BindingLoader, OpenAPIScanner, ScanOptions};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Four operations, exercising every rendering branch the matrix asks about:
/// an `operationId` (case preserved), a path-derived ID, a deprecated
/// operation, a `POST` carrying `in: query` parameters (a proxy hazard), an
/// operation with no 2xx response, and an external `$ref`.
const PETSTORE_YAML: &str = r#"
openapi: "3.1.0"
info:
  title: Petstore
  version: "1.0.0"
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
      description: "SUPERSECRETSCHEME"
paths:
  /pets:
    get:
      operationId: listPets
      summary: List all pets
      tags: [pets]
      parameters:
        - name: limit
          in: query
          schema: {type: integer}
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: {type: object}
    post:
      operationId: createPets
      summary: Create a pet
      tags: [pets]
      parameters:
        - name: dry_run
          in: query
          schema: {type: boolean}
        - name: notify
          in: query
          schema: {type: boolean}
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: {type: object}
  /pets/{petId}:
    get:
      operationId: showPetById
      summary: Info for a specific pet
      tags: [pets]
      parameters:
        - name: petId
          in: path
          required: true
          schema: {type: string}
      responses:
        "404":
          description: not found
    delete:
      tags: [pets]
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                $ref: "./common.yaml#/Error"
  /legacy:
    get:
      operationId: legacyThing
      deprecated: true
      responses:
        "200":
          description: ok
"#;

/// The same document as JSON, so content sniffing can be measured rather than
/// assumed (T-OAPI-02).
fn petstore_json() -> String {
    let value: Value = serde_yaml_ng::from_str(PETSTORE_YAML).expect("fixture parses as YAML");
    serde_json::to_string_pretty(&value).expect("fixture serializes as JSON")
}

/// A `deprecated: "false"` string, which is not the boolean `true` and must
/// therefore not be treated as deprecated (T-OAPI-09).
const STRINGY_DEPRECATED: &str = r#"
openapi: "3.0.3"
info: {title: Stringy, version: "2.0.0"}
paths:
  /thing:
    get:
      operationId: getThing
      deprecated: "false"
      responses:
        "200": {description: ok}
"#;

const SWAGGER_2: &str = r#"
swagger: "2.0"
info: {title: Old, version: "1.0.0"}
paths: {}
"#;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        // The binary refuses to start without an extensions directory.
        std::fs::create_dir(dir.path().join("extensions")).expect("mkdir extensions");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&path, contents).expect("write fixture");
        path
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_apcore-cli"))
            .current_dir(self.dir.path())
            .args(args)
            .env("APCORE_CLI_AUDIT_DISABLE", "1")
            .output()
            .expect("failed to spawn apcore-cli")
    }

    /// A project holding the petstore fixture, ready to scan.
    fn petstore() -> (Self, String) {
        let project = Self::new();
        project.write("petstore.yaml", PETSTORE_YAML);
        (project, "./petstore.yaml".to_string())
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn json_stdout(out: &Output) -> Value {
    serde_json::from_str(&stdout(out))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", stdout(out)))
}

fn module_ids(payload: &Value) -> Vec<String> {
    payload["modules"]
        .as_array()
        .expect("modules array")
        .iter()
        .map(|m| m["module_id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Scan a document in-process, for the assertions that are about the scan
/// result rather than about the command surface.
async fn scan_fixture(
    text: &str,
    options: ScanOptions,
) -> (Value, Vec<apcore_toolkit::ScannedModule>) {
    let spec: Value = serde_yaml_ng::from_str(text).expect("fixture parses");
    let modules = OpenAPIScanner::new()
        .scan(&spec, &options)
        .await
        .expect("fixture scans");
    (spec, modules)
}

// ---------------------------------------------------------------------------
// T-OAPI-01 .. T-OAPI-05 -- scanning and naming
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_01_scan_yields_one_module_per_operation() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    // 5 operations: GET /pets, POST /pets, GET+DELETE /pets/{petId}, GET /legacy.
    assert_eq!(module_ids(&payload).len(), 5, "{:?}", module_ids(&payload));
    assert_eq!(payload["openapi"], "3.1.0");
    assert_eq!(payload["title"], "Petstore");
    assert_eq!(payload["version"], "1.0.0");
}

#[test]
fn t_oapi_02_json_source_is_parsed_by_content_sniffing() {
    let project = Project::new();
    project.write("petstore.yaml", PETSTORE_YAML);
    // Deliberately a `.txt` extension: detection is content sniffing, not
    // file extension (spec 4.1).
    project.write("petstore.txt", &petstore_json());

    let from_yaml = project.run(&[
        "apcli",
        "openapi",
        "scan",
        "./petstore.yaml",
        "--format",
        "json",
    ]);
    let from_json = project.run(&[
        "apcli",
        "openapi",
        "scan",
        "./petstore.txt",
        "--format",
        "json",
    ]);
    assert_eq!(
        from_json.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&from_json)
    );
    assert_eq!(
        module_ids(&json_stdout(&from_yaml)),
        module_ids(&json_stdout(&from_json)),
        "a JSON document must produce the same modules as its YAML twin"
    );
}

#[test]
fn t_oapi_03_operation_id_ids_are_the_toolkits_verbatim() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    let ids = module_ids(&json_stdout(&out));
    // Case preserved; the CLI must not kebab-case, lowercase, or otherwise
    // post-process what `derive_module_id` returned (spec 1.2).
    assert!(ids.contains(&"listPets".to_string()), "{ids:?}");
    assert!(ids.contains(&"createPets".to_string()), "{ids:?}");
    assert!(ids.contains(&"showPetById".to_string()), "{ids:?}");
    for id in &ids {
        assert_eq!(
            *id,
            apcore_toolkit::derive_module_id(
                "/ignored",
                "get",
                &serde_json::json!({"operationId": id})
            ),
            "the CLI must return derive_module_id's output unchanged"
        );
    }
}

#[test]
fn t_oapi_04_operations_without_an_operation_id_use_the_path_algorithm() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    let ids = module_ids(&json_stdout(&out));
    let derived =
        apcore_toolkit::derive_module_id("/pets/{petId}", "delete", &serde_json::json!({}));
    assert!(
        ids.contains(&derived),
        "the DELETE with no operationId must use the path-and-method algorithm \
         ({derived}); got {ids:?}"
    );
}

#[test]
fn t_oapi_05_prefix_is_applied_to_every_id() {
    let (project, source) = Project::petstore();
    let out = project.run(&[
        "apcli", "openapi", "scan", &source, "--prefix", "api", "--format", "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let ids = module_ids(&json_stdout(&out));
    assert!(!ids.is_empty());
    for id in &ids {
        assert!(id.starts_with("api."), "unprefixed id: {id}");
    }
}

#[test]
fn t_oapi_05b_prefix_is_applied_before_filtering() {
    // The prefix is applied before filtering and dedup, so `--include` must
    // match against the prefixed form.
    let (project, source) = Project::petstore();
    let out = project.run(&[
        "apcli",
        "openapi",
        "scan",
        &source,
        "--prefix",
        "api",
        "--include",
        "^api\\.",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(module_ids(&json_stdout(&out)).len(), 5);
}

// ---------------------------------------------------------------------------
// T-OAPI-06 .. T-OAPI-09 -- filtering
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_06_include_keeps_only_matching_ids() {
    let (project, source) = Project::petstore();
    let out = project.run(&[
        "apcli",
        "openapi",
        "scan",
        &source,
        "--include",
        "^listPets$",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(module_ids(&json_stdout(&out)), vec!["listPets".to_string()]);
}

#[test]
fn t_oapi_07_invalid_regex_exits_2() {
    let (project, source) = Project::petstore();
    let out = project.run(&[
        "apcli",
        "openapi",
        "scan",
        &source,
        "--exclude",
        "[unclosed",
        "--format",
        "json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an invalid regex is a usage error; stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("Invalid regex for --exclude"),
        "the message must name the flag; stderr: {}",
        stderr(&out)
    );
}

#[test]
fn t_oapi_08_no_deprecated_omits_deprecated_operations() {
    let (project, source) = Project::petstore();
    let with = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    assert!(module_ids(&json_stdout(&with)).contains(&"legacyThing".to_string()));

    let without = project.run(&[
        "apcli",
        "openapi",
        "scan",
        &source,
        "--no-deprecated",
        "--format",
        "json",
    ]);
    assert_eq!(
        without.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&without)
    );
    let ids = module_ids(&json_stdout(&without));
    assert!(!ids.contains(&"legacyThing".to_string()), "{ids:?}");
    assert_eq!(ids.len(), 4);
}

#[test]
fn t_oapi_09_string_deprecated_is_not_deprecated() {
    let project = Project::new();
    project.write("stringy.yaml", STRINGY_DEPRECATED);
    let out = project.run(&[
        "apcli",
        "openapi",
        "scan",
        "./stringy.yaml",
        "--no-deprecated",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(
        module_ids(&json_stdout(&out)),
        vec!["getThing".to_string()],
        "`deprecated: \"false\"` is a string, not the boolean true"
    );
}

// ---------------------------------------------------------------------------
// T-OAPI-10 .. T-OAPI-12 -- warnings and refusal
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_10_missing_2xx_response_warns_but_keeps_the_module() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a partial document is a successful scan"
    );
    let payload = json_stdout(&out);
    let show = payload["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .find(|m| m["module_id"] == "showPetById")
        .expect("showPetById is present despite the warning");
    let warnings: Vec<&str> = show["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("2xx")),
        "the no-2xx warning must be surfaced, not swallowed: {warnings:?}"
    );
}

#[test]
fn t_oapi_10b_table_output_renders_the_warning_block() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "table"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let rendered = stdout(&out);
    assert!(
        rendered.contains("5 operations from ./petstore.yaml"),
        "{rendered}"
    );
    assert!(rendered.contains("warnings:"), "{rendered}");
    assert!(rendered.contains("showPetById"), "{rendered}");
    assert!(rendered.contains("GET /pets"), "{rendered}");
}

#[test]
fn t_oapi_11_external_ref_is_warned_about_and_not_fetched() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    let payload = json_stdout(&out);
    let all_warnings: Vec<String> = payload["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .flat_map(|m| {
            m["warnings"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|w| w.as_str().map(str::to_string))
        })
        .collect();
    assert!(
        all_warnings.iter().any(|w| w.contains("common.yaml")),
        "the external $ref must be named in a warning: {all_warnings:?}"
    );
    // And nothing tried to read it -- the file does not exist, yet the scan
    // succeeded.
    assert!(!project.path().join("common.yaml").exists());
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn t_oapi_12_swagger_2_exits_47_with_the_toolkit_message() {
    let project = Project::new();
    project.write("swagger.yaml", SWAGGER_2);
    let out = project.run(&["apcli", "openapi", "scan", "./swagger.yaml"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("swagger"),
        "the message must name swagger: {err}"
    );
    assert!(
        err.contains("3.0") && err.contains("3.1"),
        "the message must name the supported versions: {err}"
    );
}

#[test]
fn missing_source_exits_47_with_the_documented_message() {
    let project = Project::new();
    let out = project.run(&["apcli", "openapi", "scan", "./nope.yaml"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Cannot read OpenAPI source './nope.yaml': "),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn malformed_document_exits_47_with_the_parse_message() {
    let project = Project::new();
    project.write("bad.yaml", "openapi: 3.1.0\n  bad indent: [\n");
    let out = project.run(&["apcli", "openapi", "scan", "./bad.yaml"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Cannot parse OpenAPI source './bad.yaml': "),
        "stderr: {}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// T-OAPI-13 / T-OAPI-14 -- output formats
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_13_json_carries_per_module_warnings_and_top_level_hazards() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    let payload = json_stdout(&out);
    assert!(payload["modules"].is_array());
    for m in payload["modules"].as_array().expect("modules") {
        assert!(
            m["warnings"].is_array(),
            "every module carries its own warnings array: {m}"
        );
        assert!(
            m.get("hazards").is_none(),
            "hazards are a top-level key, not a module field: {m}"
        );
    }
    let hazards = payload["hazards"]
        .as_array()
        .expect("top-level hazards key");
    assert_eq!(hazards.len(), 1, "{hazards:?}");
    assert_eq!(hazards[0]["module_id"], "createPets");
}

#[test]
fn t_oapi_14_markdown_and_skill_render_through_the_toolkit() {
    let (project, source) = Project::petstore();
    for style in ["markdown", "skill"] {
        let out = project.run(&["apcli", "openapi", "scan", &source, "--format", style]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "[{style}] stderr: {}",
            stderr(&out)
        );
        let rendered = stdout(&out);
        assert!(rendered.contains("listPets"), "[{style}] {rendered}");
        // Diagnostics stay off the rendered document, on stderr.
        assert!(
            stderr(&out).contains("warnings:"),
            "[{style}] warnings must still be surfaced: {}",
            stderr(&out)
        );
    }
    // The `skill` style is the markdown body plus YAML frontmatter.
    let skill = project.run(&["apcli", "openapi", "scan", &source, "--format", "skill"]);
    assert!(stdout(&skill).starts_with("---"), "{}", stdout(&skill));
}

#[test]
fn csv_yaml_and_jsonl_formats_render() {
    let (project, source) = Project::petstore();
    let csv = project.run(&["apcli", "openapi", "scan", &source, "--format", "csv"]);
    assert_eq!(csv.status.code(), Some(0), "stderr: {}", stderr(&csv));
    assert!(stdout(&csv).starts_with("module_id,"), "{}", stdout(&csv));

    let jsonl = project.run(&["apcli", "openapi", "scan", &source, "--format", "jsonl"]);
    assert_eq!(jsonl.status.code(), Some(0));
    assert_eq!(stdout(&jsonl).trim().lines().count(), 5);

    let yaml = project.run(&["apcli", "openapi", "scan", &source, "--format", "yaml"]);
    assert_eq!(yaml.status.code(), Some(0));
    let parsed: Value = serde_yaml_ng::from_str(&stdout(&yaml)).expect("valid YAML");
    assert_eq!(parsed["hazards"].as_array().map(Vec::len), Some(1));
}

// ---------------------------------------------------------------------------
// T-OAPI-16 / T-OAPI-17 -- proxy-hazard detection
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_16_post_with_query_parameters_is_reported_as_a_hazard() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "table"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "hazards never change the exit code in FE-15a"
    );
    let rendered = stdout(&out);
    assert!(
        rendered.contains("cannot be proxied by FE-15b"),
        "the hazard block must be rendered inline: {rendered}"
    );
    assert!(rendered.contains("createPets"), "{rendered}");
    assert!(rendered.contains("POST"), "{rendered}");
    assert!(
        rendered.contains("dry_run") && rendered.contains("notify"),
        "the offending parameter names must be named: {rendered}"
    );
}

#[test]
fn hazard_block_does_not_reference_a_nonexistent_flag() {
    // An early spec draft's sample output referenced `--explain-hazards`,
    // which appears in no command signature. The hazard block is rendered
    // inline instead; no such flag exists.
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "scan", &source, "--format", "table"]);
    assert!(
        !stdout(&out).contains("--explain-hazards"),
        "{}",
        stdout(&out)
    );
    let rejected = project.run(&["apcli", "openapi", "scan", &source, "--explain-hazards"]);
    assert_eq!(rejected.status.code(), Some(2), "no such flag exists");
}

#[tokio::test]
async fn t_oapi_17_get_with_query_parameters_is_not_a_hazard() {
    // The petstore's GET /pets declares an `in: query` limit. Query is the
    // correct encoding for GET, so it must not be reported.
    let (spec, modules) = scan_fixture(PETSTORE_YAML, ScanOptions::new()).await;
    let hazards = detect_proxy_hazards(&spec, &modules);
    assert_eq!(hazards.len(), 1, "{hazards:?}");
    assert_eq!(hazards[0].module_id, "createPets");
    assert!(
        !hazards.iter().any(|h| h.module_id == "listPets"),
        "a GET with query parameters is correctly encoded: {hazards:?}"
    );
}

// ---------------------------------------------------------------------------
// T-OAPI-15 -- HTTP support availability (Rust form)
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_15_http_sources_are_linkable_in_this_build() {
    // `load_spec` sits behind apcore-toolkit's `http-proxy` feature, which
    // this crate declares unconditionally, so the spec's "HTTP support
    // missing" branch has no live code path in Rust. What is testable is that
    // an http(s) source reaches the network layer and reports a *fetch*
    // failure rather than a missing-symbol or unsupported-scheme error.
    let project = Project::new();
    // Reserved TEST-NET-1 address with a sub-second timeout: no connection is
    // possible, and no real host is contacted.
    let out = project.run(&[
        "apcli",
        "openapi",
        "scan",
        "http://192.0.2.1:9/openapi.json",
        "--openapi-timeout",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("Cannot read OpenAPI source 'http://192.0.2.1:9/openapi.json'"),
        "an http source must reach the fetch path: {err}"
    );
    assert!(
        !err.contains("http-proxy"),
        "the feature is enabled in this build, so the missing-feature message \
         must not appear: {err}"
    );
}

#[test]
fn openapi_timeout_is_expressed_in_seconds_and_validated() {
    let (project, source) = Project::petstore();
    let ok = project.run(&[
        "apcli",
        "openapi",
        "scan",
        &source,
        "--openapi-timeout",
        "5",
        "--format",
        "json",
    ]);
    assert_eq!(ok.status.code(), Some(0), "stderr: {}", stderr(&ok));

    let bad = project.run(&[
        "apcli",
        "openapi",
        "scan",
        &source,
        "--openapi-timeout",
        "nonsense",
    ]);
    assert_eq!(bad.status.code(), Some(2), "stderr: {}", stderr(&bad));
    assert!(
        stderr(&bad).contains("seconds"),
        "the message must state the unit: {}",
        stderr(&bad)
    );
}

// ---------------------------------------------------------------------------
// T-OAPI-18 .. T-OAPI-22 -- generate
// ---------------------------------------------------------------------------

fn generated_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

#[test]
fn t_oapi_18_generate_writes_one_binding_per_module() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let files = generated_files(&project.path().join("out"));
    assert_eq!(files.len(), 5, "{files:?}");
    for name in &files {
        assert!(name.ends_with(BINDING_SUFFIX), "{name}");
    }
    assert!(
        files.contains(&format!("createPets{BINDING_SUFFIX}")),
        "{files:?}"
    );
}

#[test]
fn t_oapi_19_dry_run_lists_paths_and_creates_nothing() {
    let (project, source) = Project::petstore();
    let out = project.run(&[
        "apcli",
        "openapi",
        "generate",
        &source,
        "-o",
        "./out",
        "--dry-run",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let listed = stdout(&out);
    assert_eq!(listed.trim().lines().count(), 5, "{listed}");
    assert!(
        listed.contains(&format!("createPets{BINDING_SUFFIX}")),
        "{listed}"
    );
    assert!(
        !project.path().join("out").exists(),
        "--dry-run must not create the output directory"
    );
}

#[test]
fn t_oapi_20_artifact_carries_an_intact_routing_contract() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));

    let path = project
        .path()
        .join("out")
        .join(format!("createPets{BINDING_SUFFIX}"));
    let text = std::fs::read_to_string(&path).expect("read artifact");
    let doc: Value = serde_yaml_ng::from_str(&text).expect("artifact is valid YAML");
    let binding = &doc["bindings"][0];

    // `target` is a route descriptor, NOT an import path (spec 4.5).
    assert_eq!(binding["target"], "POST /pets");
    assert_eq!(binding["metadata"]["http_method"], "POST");
    assert_eq!(
        binding["metadata"]["http_method"].as_str().unwrap(),
        binding["metadata"]["http_method"]
            .as_str()
            .unwrap()
            .to_uppercase(),
        "http_method must be uppercase"
    );
    let url_path = binding["metadata"]["url_path"].as_str().expect("url_path");
    assert!(url_path.starts_with('/'), "{url_path}");
    assert_eq!(url_path, "/pets");
    assert_eq!(binding["metadata"]["openapi"]["spec_version"], "3.1.0");
    assert_eq!(binding["metadata"]["openapi"]["operation_id"], "createPets");

    // Braces are retained on a templated path.
    let templated = generated_files(&project.path().join("out"))
        .into_iter()
        .find(|n| n.starts_with("showPetById"))
        .expect("showPetById artifact");
    let templated_doc: Value = serde_yaml_ng::from_str(
        &std::fs::read_to_string(project.path().join("out").join(templated)).expect("read"),
    )
    .expect("valid YAML");
    assert_eq!(
        templated_doc["bindings"][0]["metadata"]["url_path"],
        "/pets/{petId}"
    );

    // And the routing keys survive a full round-trip through BindingLoader.
    let loaded = BindingLoader::new()
        .load(&path, /*strict*/ true, /*recursive*/ false)
        .expect("binding loads");
    let reloaded = loaded
        .iter()
        .find(|m| m.module_id == "createPets")
        .expect("createPets round-trips");
    assert_eq!(
        reloaded.metadata.get("http_method").and_then(Value::as_str),
        Some("POST"),
        "FE-15b depends entirely on this key surviving"
    );
    assert_eq!(
        reloaded.metadata.get("url_path").and_then(Value::as_str),
        Some("/pets")
    );
}

#[test]
fn t_oapi_21_existing_file_is_skipped_without_force() {
    let (project, source) = Project::petstore();
    let target = format!("out/createPets{BINDING_SUFFIX}");
    project.write(&target, "PRE-EXISTING\n");

    let out = project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a skipped file is not an error; stderr: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("WARNING") && stderr(&out).contains("createPets"),
        "a WARNING must name the skipped file: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join(&target)).expect("read"),
        "PRE-EXISTING\n",
        "the existing file must be unchanged"
    );
    // Every other module still gets written.
    assert_eq!(generated_files(&project.path().join("out")).len(), 5);
}

#[test]
fn t_oapi_22_force_overwrites_an_existing_file() {
    let (project, source) = Project::petstore();
    let target = format!("out/createPets{BINDING_SUFFIX}");
    project.write(&target, "PRE-EXISTING\n");

    let out = project.run(&[
        "apcli", "openapi", "generate", &source, "-o", "./out", "--force",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let text = std::fs::read_to_string(project.path().join(&target)).expect("read");
    assert_ne!(text, "PRE-EXISTING\n");
    assert!(text.contains("createPets"), "{text}");
}

// ---------------------------------------------------------------------------
// T-OAPI-24 / T-OAPI-25 -- credentials never reach disk
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_24_header_values_are_absent_from_every_generated_file() {
    let (project, source) = Project::petstore();
    let out = project.run(&[
        "apcli",
        "openapi",
        "generate",
        &source,
        "-o",
        "./out",
        "--header",
        "Authorization: Bearer SUPERSECRETTOKEN",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    for name in generated_files(&project.path().join("out")) {
        let text = std::fs::read_to_string(project.path().join("out").join(&name)).expect("read");
        assert!(
            !text.contains("SUPERSECRETTOKEN"),
            "credential leaked into {name}"
        );
        assert!(
            !text.contains("Authorization"),
            "header name leaked into {name}"
        );
    }
}

#[test]
fn t_oapi_25_security_schemes_are_not_copied_into_artifacts() {
    let (project, source) = Project::petstore();
    let out = project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    for name in generated_files(&project.path().join("out")) {
        let text = std::fs::read_to_string(project.path().join("out").join(&name)).expect("read");
        assert!(
            !text.contains("SUPERSECRETSCHEME"),
            "securitySchemes material leaked into {name}"
        );
        assert!(
            !text.contains("securitySchemes"),
            "securitySchemes leaked into {name}"
        );
    }
}

#[test]
fn no_base_url_is_written() {
    // Spec 4.4: a base URL in the artifact would be metadata nothing in this
    // release consumes. It arrives with FE-15b, where a dispatcher exists.
    let (project, source) = Project::petstore();
    project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    let text = std::fs::read_to_string(
        project
            .path()
            .join("out")
            .join(format!("createPets{BINDING_SUFFIX}")),
    )
    .expect("read");
    let doc: Value = serde_yaml_ng::from_str(&text).expect("valid YAML");
    assert!(
        doc["bindings"][0]["metadata"]["openapi"]
            .get("base_url")
            .is_none(),
        "no base_url key may be written in FE-15a: {text}"
    );
}

// ---------------------------------------------------------------------------
// T-OAPI-26 / T-OAPI-27
// ---------------------------------------------------------------------------

#[test]
fn t_oapi_26_generate_reports_the_same_hazards_as_scan() {
    let (project, source) = Project::petstore();
    let scanned = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    let scan_hazards: Vec<String> = json_stdout(&scanned)["hazards"]
        .as_array()
        .expect("hazards")
        .iter()
        .map(|h| h["module_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(scan_hazards, vec!["createPets".to_string()]);

    let generated = project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    let err = stderr(&generated);
    assert!(
        err.contains("cannot be proxied by FE-15b"),
        "generate must report hazards too: {err}"
    );
    for id in &scan_hazards {
        assert!(
            err.contains(id),
            "hazard '{id}' missing from generate: {err}"
        );
    }
}

#[test]
fn t_oapi_27_neither_command_touches_the_registry() {
    // The scratch project has an empty extensions directory, so no module is
    // registered and no system module is available. Both commands must still
    // succeed -- they need neither registry nor executor (spec 4.7).
    let (project, source) = Project::petstore();
    let scanned = project.run(&["apcli", "openapi", "scan", &source, "--format", "json"]);
    assert_eq!(
        scanned.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&scanned)
    );
    let generated = project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    assert_eq!(
        generated.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&generated)
    );

    // And `apcli list` on the same project finds nothing, confirming the
    // registry really is empty for the run above.
    let listed = project.run(&["apcli", "list", "--format", "json"]);
    assert_eq!(listed.status.code(), Some(0), "stderr: {}", stderr(&listed));
}

// ---------------------------------------------------------------------------
// Registration (spec 4.7) and helper contracts
// ---------------------------------------------------------------------------

#[test]
fn openapi_is_registered_under_the_apcli_group() {
    let group = apcore_cli::register_apcli_subcommands(
        clap::Command::new("apcli"),
        &apcore_cli::ApcliGroup::from_yaml(None, /*registry_injected*/ false),
        "apcore-cli",
    );
    let openapi = group
        .get_subcommands()
        .find(|c| c.get_name() == "openapi")
        .expect("openapi must be registered under apcli");
    let subs: Vec<&str> = openapi.get_subcommands().map(|c| c.get_name()).collect();
    assert_eq!(subs, vec!["scan", "generate"]);
}

#[test]
fn openapi_is_not_always_registered_and_is_not_a_system_command() {
    assert!(!apcore_cli::APCLI_ALWAYS_REGISTERED.contains(&"openapi"));
    assert!(!apcore_cli::SYSTEM_COMMANDS.contains(&"openapi"));
}

#[test]
fn there_is_no_root_level_openapi_entry_point() {
    // Root-level shims were retired in v0.8; `<cli> apcli openapi <sub>` is
    // the sole path (spec 4.7).
    let project = Project::new();
    let out = project.run(&["--help"]);
    let rendered = stdout(&out);
    assert!(
        !rendered
            .lines()
            .any(|l| l.trim_start().starts_with("openapi ")),
        "openapi must not appear as a root-level command: {rendered}"
    );
}

#[test]
fn planned_paths_match_what_the_writer_actually_produced() {
    // The CLI mirrors YAMLWriter's filename derivation so it can answer
    // --dry-run and decide the skip-without---force case. If the mirror drifts
    // from the writer, --dry-run lies and --force stops protecting anything.
    let (project, source) = Project::petstore();
    let dry = project.run(&[
        "apcli",
        "openapi",
        "generate",
        &source,
        "-o",
        "./out",
        "--dry-run",
    ]);
    let mut predicted: Vec<String> = stdout(&dry)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            Path::new(l)
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .to_string()
        })
        .collect();
    predicted.sort();

    project.run(&["apcli", "openapi", "generate", &source, "-o", "./out"]);
    assert_eq!(
        predicted,
        generated_files(&project.path().join("out")),
        "planned_paths must agree with YAMLWriter"
    );
}

#[test]
fn planned_paths_and_sanitizer_are_pure() {
    let module = apcore_toolkit::ScannedModule::new(
        "a/b".to_string(),
        String::new(),
        serde_json::json!({}),
        serde_json::json!({}),
        vec![],
        "GET /x".to_string(),
    );
    assert_eq!(sanitize_binding_filename("a/b"), "a_b");
    assert_eq!(
        planned_paths(std::slice::from_ref(&module), "/out"),
        vec![PathBuf::from(format!("/out/a_b{BINDING_SUFFIX}"))]
    );
}

#[tokio::test]
async fn load_openapi_source_takes_the_source_verbatim() {
    // No candidate-path probing: a wrong path must produce an honest failure
    // rather than a surprising success against a different document.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("openapi.json"), "{}").expect("write");
    let wrong = dir.path().join("api");
    let err = load_openapi_source(wrong.to_str().unwrap(), &Default::default(), 1.0)
        .await
        .expect_err("no probing");
    assert!(err.to_string().starts_with("Cannot read OpenAPI source '"));
}
