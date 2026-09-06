// apcore-cli -- `apcli openapi` command group (FE-15a sections 4.2 and 4.4).
//
// Two commands, both document-to-artifact only:
//
// * `scan`     -- read a document and show what it would produce.
// * `generate` -- materialize the scan as `.binding.yaml` files or
//                 host-language source.
//
// Neither registers a module, builds an executor, or issues a request to the
// described API. `scan` of a local file performs no network I/O at all; `scan`
// of an `http(s)://` source fetches exactly one document, the one named on the
// command line.
//
// FE-15a does NOT make an API callable. Passing generated binding files to
// `--binding` does not yet produce working commands -- see FE-15 section 8.1.
// The help text says so, because stating that plainly is part of the
// deliverable.

use std::path::{Path, PathBuf};

use apcore_toolkit::{
    format_modules, module_to_dict, FormatOutput, ModuleStyle, OpenAPIScanner, ScanOptions,
    ScannedModule, ScannerError, WriteResult, YAMLWriter,
};
use clap::{Arg, ArgAction, Command};
use serde_json::{Map, Value};

use crate::openapi_source::{
    detect_proxy_hazards, load_openapi_source, parse_headers, Hazard, DEFAULT_OPENAPI_TIMEOUT_SECS,
};
use crate::{EXIT_CONFIG_NOT_FOUND, EXIT_INVALID_INPUT, EXIT_MODULE_EXECUTE_ERROR, EXIT_SUCCESS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Suffix written by `YAMLWriter`.
pub const BINDING_SUFFIX: &str = ".binding.yaml";

/// Warning shown on every `scan` / `generate` invocation's help text.
const NOT_YET_EXECUTABLE: &str =
    "FE-15a writes artifacts only. Passing the generated files to --binding does \
     NOT yet produce working commands.";

// ---------------------------------------------------------------------------
// Command builder
// ---------------------------------------------------------------------------

fn scan_option_args(cmd: Command) -> Command {
    // These map one-to-one onto `OpenAPIScanner::scan` options and are
    // forwarded verbatim. The hooks (transform_operation, derive_module_id,
    // transform_module) are deliberately NOT exposed: overriding derivation
    // hands back the cross-SDK naming guarantee, which is not something a
    // command-line flag should be able to do silently (spec 4.2).
    cmd.arg(
        Arg::new("include")
            .long("include")
            .value_name("REGEX")
            .help("Keep only module IDs matching this regex."),
    )
    .arg(
        Arg::new("exclude")
            .long("exclude")
            .value_name("REGEX")
            .help("Drop module IDs matching this regex."),
    )
    .arg(
        Arg::new("prefix")
            .long("prefix")
            .value_name("PREFIX")
            .help("Prepend '<PREFIX>.' to every derived module ID."),
    )
    .arg(
        Arg::new("no-deprecated")
            .long("no-deprecated")
            .action(ArgAction::SetTrue)
            .help("Omit operations marked deprecated: true."),
    )
    .arg(
        Arg::new("header")
            .long("header")
            .action(ArgAction::Append)
            .value_name("K: V")
            .help(
                "Request header for an http(s) source. Repeatable. \
                 Never written to a generated file.",
            ),
    )
    .arg(
        Arg::new("openapi-timeout")
            .long("openapi-timeout")
            .value_name("SECS")
            .help("Fetch timeout in seconds (default: 30)."),
    )
}

/// Build the `openapi` clap subcommand group.
///
/// **API divergence (matching `register_pipeline_command`):** the spec writes
/// this registrar as `register_openapi_command(apcli_group)`; the Rust
/// registrar table in `lib.rs` is a `FnOnce(Command) -> Command` table, so the
/// builder takes and returns the parent command.
pub fn openapi_command() -> Command {
    let scan = scan_option_args(
        Command::new("scan")
            .about("Show the modules an OpenAPI document would produce")
            .long_about(format!(
                "Read an OpenAPI 3.0/3.1 document and render the modules it would \
                 produce. Nothing is written and no module is registered.\n\n{NOT_YET_EXECUTABLE}"
            ))
            .arg(
                Arg::new("source")
                    .required(true)
                    .value_name("SOURCE")
                    .help("Local path or http(s):// URL, taken verbatim."),
            )
            .arg(
                Arg::new("format")
                    .long("format")
                    .value_parser(["table", "json", "csv", "yaml", "jsonl", "markdown", "skill"])
                    .value_name("FORMAT")
                    .help("Output format."),
            ),
    );

    // There is deliberately NO `--writer` flag (spec 4.4). An earlier draft
    // offered `--writer native`, mapping to the toolkit's `RustWriter` on the
    // `apcli init` precedent that each SDK scaffolds in its own language. It
    // cannot work, for the same reason `RegistryWriter` cannot (spec 4.5):
    // every toolkit source writer resolves `target` as a
    // `module.path:callable` import path, while an OpenAPI-derived `target` is
    // always a route descriptor such as "GET /pets". The flag could never
    // succeed for any input this command can produce, so it is absent rather
    // than present-and-always-failing. Emitting genuine host-language source
    // for an OpenAPI operation means emitting an HTTP proxy implementation,
    // which belongs with FE-15b.
    let generate = scan_option_args(
        Command::new("generate")
            .about("Write the scanned modules to disk as .binding.yaml artifacts")
            .long_about(format!(
                "Materialize an OpenAPI document as <id>.binding.yaml files through the \
                 toolkit's YAMLWriter. This is the cross-language artifact: the same \
                 document produces comparable output from every SDK.\n\n{NOT_YET_EXECUTABLE}"
            ))
            .arg(
                Arg::new("source")
                    .required(true)
                    .value_name("SOURCE")
                    .help("Local path or http(s):// URL, taken verbatim."),
            )
            .arg(
                Arg::new("output")
                    .long("output")
                    .short('o')
                    .required(true)
                    .value_name("DIR")
                    .help("Output directory."),
            )
            .arg(
                Arg::new("dry-run")
                    .long("dry-run")
                    .action(ArgAction::SetTrue)
                    .help("List the paths that would be written; create nothing."),
            )
            .arg(
                Arg::new("force")
                    .long("force")
                    .action(ArgAction::SetTrue)
                    .help("Overwrite existing files instead of skipping them."),
            ),
    );

    Command::new("openapi")
        .about("Import an OpenAPI document as apcore module artifacts")
        .subcommand(scan)
        .subcommand(generate)
}

/// Attach the `openapi` subcommand group to the given command.
pub fn register_openapi_command(cli: Command) -> Command {
    cli.subcommand(openapi_command())
}

// ---------------------------------------------------------------------------
// Planned output paths
// ---------------------------------------------------------------------------

/// Mirror of `YAMLWriter`'s filename sanitization: everything outside
/// `[a-zA-Z0-9._-]` becomes `_`, then runs of two or more dots collapse to a
/// single `_` (path-traversal prevention).
///
/// Duplicated here rather than imported because the toolkit keeps it private,
/// and the CLI needs the filename *before* the writer runs -- to answer
/// `--dry-run` and to decide whether an existing file should be skipped
/// without `--force`. It derives no module ID and makes no routing decision.
#[must_use]
pub fn sanitize_binding_filename(module_id: &str) -> String {
    let safe: String = module_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Collapse each run of 2+ dots into one '_'.
    let mut out = String::with_capacity(safe.len());
    let mut dot_run = 0usize;
    for c in safe.chars() {
        if c == '.' {
            dot_run += 1;
            continue;
        }
        flush_dot_run(&mut out, dot_run);
        dot_run = 0;
        out.push(c);
    }
    flush_dot_run(&mut out, dot_run);
    out
}

fn flush_dot_run(out: &mut String, run: usize) {
    match run {
        0 => {}
        1 => out.push('.'),
        _ => out.push('_'),
    }
}

/// The path each module would be written to, in writer order.
///
/// Reproduces `YAMLWriter`'s in-batch collision suffixing
/// (`foo_1.binding.yaml`) so `--dry-run` can report real paths and the
/// skip-without-`--force` decision can be made before the writer runs.
#[must_use]
pub fn planned_paths(modules: &[ScannedModule], output_dir: &str) -> Vec<PathBuf> {
    let root = Path::new(output_dir);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    modules
        .iter()
        .map(|m| {
            let safe = sanitize_binding_filename(&m.module_id);
            let mut name = format!("{safe}{BINDING_SUFFIX}");
            let mut counter = 0u32;
            while seen.contains(&name) {
                counter += 1;
                name = format!("{safe}_{counter}{BINDING_SUFFIX}");
            }
            seen.insert(name.clone());
            root.join(name)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Scan rendering
// ---------------------------------------------------------------------------

/// Machine payload for `scan`. Each module carries its own `warnings` array
/// (via the toolkit's `module_to_dict`); hazards sit under a **top-level**
/// `hazards` key because they are a statement about a *future* execution path
/// rather than about the scan that just ran (spec 4.3).
#[must_use]
pub fn scan_payload(
    source: &str,
    spec: &Value,
    modules: &[ScannedModule],
    hazards: &[Hazard],
) -> Value {
    serde_json::json!({
        "source": source,
        "openapi": spec.get("openapi").cloned().unwrap_or(Value::Null),
        "title": spec.pointer("/info/title").cloned().unwrap_or(Value::Null),
        "version": spec.pointer("/info/version").cloned().unwrap_or(Value::Null),
        "modules": modules.iter().map(module_to_dict).collect::<Vec<_>>(),
        "hazards": hazards.iter().map(Hazard::to_json).collect::<Vec<_>>(),
    })
}

fn scan_header(source: &str, spec: &Value, count: usize) -> String {
    let version = spec.get("openapi").and_then(Value::as_str).unwrap_or("?");
    let title = spec
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("untitled");
    let doc_version = spec
        .pointer("/info/version")
        .and_then(Value::as_str)
        .unwrap_or("");
    let plural = if count == 1 {
        "operation"
    } else {
        "operations"
    };
    format!("{count} {plural} from {source} (OpenAPI {version}, {title} {doc_version})")
        .trim_end()
        .to_string()
}

/// The warnings and hazards blocks, or an empty string when there are none.
///
/// Warnings MUST be rendered, not dropped: the scanner is a
/// degrade-with-warning design, and the warning is the only signal that a
/// module's flags are incomplete (spec 4.2).
#[must_use]
pub fn render_diagnostics(modules: &[ScannedModule], hazards: &[Hazard]) -> String {
    let mut out = String::new();

    let warned: Vec<(&str, &String)> = modules
        .iter()
        .flat_map(|m| m.warnings.iter().map(move |w| (m.module_id.as_str(), w)))
        .collect();
    if !warned.is_empty() {
        let plural = if warned.len() == 1 {
            "warning"
        } else {
            "warnings"
        };
        out.push_str(&format!("\n{} {plural}:\n", warned.len()));
        for (id, warning) in warned {
            out.push_str(&format!("  {id}  {warning}\n"));
        }
    }

    if !hazards.is_empty() {
        let plural = if hazards.len() == 1 {
            "operation"
        } else {
            "operations"
        };
        out.push_str(&format!(
            "\n{} {plural} cannot be proxied by FE-15b:\n",
            hazards.len()
        ));
        for hazard in hazards {
            out.push_str(&format!("  {}  {}\n", hazard.module_id, hazard.summary()));
        }
    }

    out
}

fn render_scan_table(source: &str, spec: &Value, modules: &[ScannedModule]) -> String {
    use comfy_table::{ContentArrangement, Table};

    let header = scan_header(source, spec, modules.len());
    if modules.is_empty() {
        return header;
    }
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["Module ID", "Route", "Description", "Tags"]);
    for m in modules {
        table.add_row(vec![
            m.module_id.clone(),
            // `target` is the scanner's route descriptor ("GET /pets"), a
            // documented deviation from target's usual module.path:callable
            // meaning -- see spec 4.5.
            m.target.clone(),
            m.description.clone(),
            m.tags.join(", "),
        ]);
    }
    format!("{header}\n\n{table}")
}

fn module_rows(modules: &[ScannedModule]) -> Vec<Map<String, Value>> {
    modules
        .iter()
        .filter_map(|m| module_to_dict(m).as_object().cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the `openapi` subcommand group. Never returns -- every arm exits.
pub async fn dispatch_openapi(matches: &clap::ArgMatches) {
    match matches.subcommand() {
        Some(("scan", sub_m)) => dispatch_scan(sub_m).await,
        Some(("generate", sub_m)) => dispatch_generate(sub_m).await,
        _ => {
            eprintln!("Error: Usage: openapi <scan|generate>");
            std::process::exit(EXIT_INVALID_INPUT);
        }
    }
}

struct ScanInputs {
    source: String,
    spec: Value,
    modules: Vec<ScannedModule>,
    hazards: Vec<Hazard>,
}

/// Load, scan, and detect hazards. Exits on every failure so both commands
/// share one error contract.
async fn run_scan(sub_m: &clap::ArgMatches) -> ScanInputs {
    let source = sub_m
        .get_one::<String>("source")
        .expect("source is required")
        .clone();

    let raw_headers: Vec<String> = sub_m
        .get_many::<String>("header")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();
    let headers = parse_headers(&raw_headers).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(EXIT_INVALID_INPUT);
    });

    let timeout = match sub_m.get_one::<String>("openapi-timeout") {
        Some(raw) => match raw.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            _ => {
                eprintln!(
                    "Error: Invalid --openapi-timeout value '{raw}': expected seconds as a positive number."
                );
                std::process::exit(EXIT_INVALID_INPUT);
            }
        },
        None => DEFAULT_OPENAPI_TIMEOUT_SECS,
    };

    let spec = load_openapi_source(&source, &headers, timeout)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(e.exit_code());
        });

    let options = ScanOptions {
        include: sub_m.get_one::<String>("include").cloned(),
        exclude: sub_m.get_one::<String>("exclude").cloned(),
        base_path_prefix: sub_m.get_one::<String>("prefix").cloned(),
        include_deprecated: !sub_m.get_flag("no-deprecated"),
        transform_operation: None,
        derive_module_id: None,
        transform_module: None,
    };

    let modules = OpenAPIScanner::new()
        .scan(&spec, &options)
        .await
        .unwrap_or_else(|e| {
            match e {
                // The toolkit message names the offending `openapi` value and
                // states that Swagger 2.0 is unsupported; reproduce it
                // verbatim (spec 4.1 error table).
                ScannerError::InvalidSpec(msg) => {
                    eprintln!("Error: {msg}");
                    std::process::exit(EXIT_CONFIG_NOT_FOUND);
                }
                ScannerError::Pattern(err) => {
                    let flag = if sub_m.get_one::<String>("include").is_some() {
                        "include"
                    } else {
                        "exclude"
                    };
                    eprintln!("Error: Invalid regex for --{flag}: {err}");
                    std::process::exit(EXIT_INVALID_INPUT);
                }
            }
        });

    let hazards = detect_proxy_hazards(&spec, &modules);
    ScanInputs {
        source,
        spec,
        modules,
        hazards,
    }
}

async fn dispatch_scan(sub_m: &clap::ArgMatches) {
    let scan = run_scan(sub_m).await;
    let explicit = sub_m.get_one::<String>("format").map(String::as_str);
    let fmt = crate::output::resolve_format(explicit);

    match fmt {
        "json" => {
            let payload = scan_payload(&scan.source, &scan.spec, &scan.modules, &scan.hazards);
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
            );
        }
        "yaml" => {
            let payload = scan_payload(&scan.source, &scan.spec, &scan.modules, &scan.hazards);
            println!(
                "{}",
                serde_yaml_ng::to_string(&payload)
                    .map(|s| s.trim_end().to_string())
                    .unwrap_or_default()
            );
        }
        "csv" | "jsonl" => {
            let rows = module_rows(&scan.modules);
            let rendered = if fmt == "csv" {
                apcore_toolkit::format_csv(&rows, false)
                    .trim_end_matches("\r\n")
                    .to_string()
            } else {
                apcore_toolkit::format_jsonl(&rows)
                    .trim_end_matches('\n')
                    .to_string()
            };
            println!("{rendered}");
            // csv/jsonl are flat row formats with nowhere to put a top-level
            // `hazards` key, so hazards go to stderr rather than being dropped.
            emit_diagnostics_to_stderr(&[], &scan.hazards);
        }
        "markdown" | "skill" => {
            // Rendered through the toolkit wholesale -- `scan()` returns
            // `ScannedModule` values, which is exactly the type
            // `format_modules` accepts, so no adaptation layer is needed
            // (spec 4.2 / T-OAPI-14).
            let style = if fmt == "skill" {
                ModuleStyle::Skill
            } else {
                ModuleStyle::Markdown
            };
            match format_modules(&scan.modules, style, None, true) {
                FormatOutput::Text(s) => println!("{s}"),
                other => println!("{other:?}"),
            }
            // Keep the rendered document clean; diagnostics still surface.
            emit_diagnostics_to_stderr(&scan.modules, &scan.hazards);
        }
        _ => {
            print!(
                "{}",
                render_scan_table(&scan.source, &scan.spec, &scan.modules)
            );
            println!();
            let diagnostics = render_diagnostics(&scan.modules, &scan.hazards);
            if !diagnostics.is_empty() {
                print!("{diagnostics}");
            }
        }
    }

    // Exit 0 even when warnings or hazards are present -- a
    // partially-understood document is a successful scan (spec 4.2).
    std::process::exit(EXIT_SUCCESS);
}

fn emit_diagnostics_to_stderr(modules: &[ScannedModule], hazards: &[Hazard]) {
    let diagnostics = render_diagnostics(modules, hazards);
    if !diagnostics.is_empty() {
        eprint!("{diagnostics}");
    }
}

async fn dispatch_generate(sub_m: &clap::ArgMatches) {
    let scan = run_scan(sub_m).await;
    let output_dir = sub_m
        .get_one::<String>("output")
        .expect("--output is required")
        .clone();
    let dry_run = sub_m.get_flag("dry-run");
    let force = sub_m.get_flag("force");

    let planned = planned_paths(&scan.modules, &output_dir);

    if dry_run {
        for path in &planned {
            println!("{}", path.display());
        }
        report_generate_diagnostics(&scan);
        std::process::exit(EXIT_SUCCESS);
    }

    // Without --force an existing file is skipped with a warning and the
    // command still exits 0, matching `apcli init`'s non-destructive default
    // (spec 4.4).
    let mut to_write: Vec<ScannedModule> = Vec::new();
    let mut skipped = 0usize;
    for (module, path) in scan.modules.iter().zip(planned.iter()) {
        if !force && path.exists() {
            eprintln!(
                "WARNING: {} exists; skipping (use --force to overwrite).",
                path.display()
            );
            skipped += 1;
            continue;
        }
        to_write.push(module.clone());
    }

    let results = write_modules(&to_write, &output_dir);
    for result in &results {
        match (&result.path, result.verified) {
            (Some(path), true) => println!("{path}"),
            (Some(path), false) => eprintln!(
                "WARNING: {path} written but not verified: {}",
                result.verification_error.as_deref().unwrap_or("unknown")
            ),
            (None, _) => println!("{}", result.module_id),
        }
    }
    if skipped > 0 {
        eprintln!("{skipped} file(s) skipped; re-run with --force to overwrite.");
    }
    report_generate_diagnostics(&scan);
    std::process::exit(EXIT_SUCCESS);
}

fn report_generate_diagnostics(scan: &ScanInputs) {
    // `generate` reports the same hazard set as `scan` on the same document
    // (spec 4.3 / T-OAPI-26). Diagnostics go to stderr so the written-path
    // listing on stdout stays machine-readable.
    emit_diagnostics_to_stderr(&scan.modules, &scan.hazards);
}

fn write_modules(modules: &[ScannedModule], output_dir: &str) -> Vec<WriteResult> {
    if modules.is_empty() {
        return Vec::new();
    }
    // `verify: true` runs the toolkit's YAMLVerifier over each written file,
    // so a truncated or unparseable artifact is reported rather than assumed
    // good.
    YAMLWriter
        .write(modules, output_dir, false, true, None)
        .unwrap_or_else(|e| {
            // An unwritable output directory is an OS-level error, propagated
            // (spec 6, exit 1).
            eprintln!("Error: {e}");
            std::process::exit(EXIT_MODULE_EXECUTE_ERROR);
        })
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
            "desc".to_string(),
            json!({"type": "object"}),
            json!({"type": "object"}),
            vec!["pets".to_string()],
            format!("{method} {path}"),
        );
        m.metadata
            .insert("http_method".to_string(), Value::from(method));
        m.metadata.insert("url_path".to_string(), Value::from(path));
        m
    }

    // ----- command surface -----

    #[test]
    fn openapi_group_registers_scan_and_generate() {
        let cmd = openapi_command();
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert_eq!(names, vec!["scan", "generate"]);
    }

    #[test]
    fn register_openapi_command_attaches_the_group() {
        let cmd = register_openapi_command(Command::new("apcli"));
        assert!(cmd.get_subcommands().any(|c| c.get_name() == "openapi"));
    }

    #[test]
    fn scan_forwards_every_documented_option() {
        let scan = openapi_command()
            .get_subcommands()
            .find(|c| c.get_name() == "scan")
            .expect("scan")
            .clone();
        let ids: Vec<String> = scan
            .get_arguments()
            .map(|a| a.get_id().to_string())
            .collect();
        for expected in [
            "source",
            "include",
            "exclude",
            "prefix",
            "no-deprecated",
            "header",
            "openapi-timeout",
            "format",
        ] {
            assert!(
                ids.contains(&expected.to_string()),
                "missing {expected}: {ids:?}"
            );
        }
    }

    #[test]
    fn generate_requires_an_output_directory() {
        let cmd = openapi_command();
        let parsed = cmd.try_get_matches_from(vec!["openapi", "generate", "./spec.yaml"]);
        assert!(parsed.is_err(), "-o/--output must be required");
    }

    #[test]
    fn generate_offers_no_writer_flag() {
        // Spec 4.4: every toolkit source writer resolves `target` as a
        // `module.path:callable` import path, while an OpenAPI-derived target
        // is always a route descriptor. A `--writer native` could therefore
        // never succeed for any input this command can produce, so it is
        // absent rather than present-and-always-failing.
        let generate = openapi_command()
            .get_subcommands()
            .find(|c| c.get_name() == "generate")
            .expect("generate")
            .clone();
        let ids: Vec<String> = generate
            .get_arguments()
            .map(|a| a.get_id().to_string())
            .collect();
        assert!(!ids.contains(&"writer".to_string()), "got {ids:?}");

        let parsed = openapi_command().try_get_matches_from(vec![
            "openapi", "generate", "./s.yaml", "-o", "./out", "--writer", "native",
        ]);
        assert!(parsed.is_err(), "--writer must not be accepted");
    }

    #[test]
    fn help_states_that_artifacts_are_not_yet_executable() {
        let scan = openapi_command()
            .get_subcommands()
            .find(|c| c.get_name() == "scan")
            .expect("scan")
            .clone();
        let long = scan
            .get_long_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(long.contains("--binding"), "{long}");
        assert!(long.contains("NOT yet"), "{long}");
    }

    // ----- filename planning -----

    #[test]
    fn binding_filenames_mirror_the_toolkit_sanitizer() {
        assert_eq!(
            sanitize_binding_filename("pets.petid.get"),
            "pets.petid.get"
        );
        assert_eq!(sanitize_binding_filename("a/b"), "a_b");
        // "../evil": '/' -> '_' gives ".._evil", then the ".." run collapses
        // to a single '_' -> "__evil". No path component survives, which is
        // the traversal guard the toolkit's two-step sanitizer exists for.
        assert_eq!(sanitize_binding_filename("../evil"), "__evil");
        assert_eq!(sanitize_binding_filename("a..b"), "a_b");
        assert_eq!(sanitize_binding_filename("a...b"), "a_b");
        assert_eq!(sanitize_binding_filename("Pet-1_2"), "Pet-1_2");
    }

    #[test]
    fn planned_yaml_paths_suffix_in_batch_collisions() {
        let modules = vec![module("a/b", "GET", "/x"), module("a:b", "GET", "/y")];
        let paths = planned_paths(&modules, "/out");
        assert_eq!(paths[0], PathBuf::from("/out/a_b.binding.yaml"));
        assert_eq!(paths[1], PathBuf::from("/out/a_b_1.binding.yaml"));
    }

    #[test]
    fn planned_paths_use_the_binding_suffix() {
        let modules = vec![module("pets.petid.get", "GET", "/pets/{petId}")];
        let paths = planned_paths(&modules, "/out");
        assert_eq!(paths[0], PathBuf::from("/out/pets.petid.get.binding.yaml"));
    }

    // ----- rendering -----

    #[test]
    fn scan_payload_carries_warnings_per_module_and_hazards_at_the_top() {
        let spec = json!({
            "openapi": "3.1.0",
            "info": {"title": "Petstore", "version": "1.0.0"},
            "paths": {}
        });
        let mut m = module("createPets", "POST", "/pets");
        m.warnings
            .push("no 2xx response defined; output_schema is empty".to_string());
        let hazards = vec![Hazard {
            module_id: "createPets".to_string(),
            http_method: "POST".to_string(),
            url_path: "/pets".to_string(),
            parameters: vec!["q".to_string()],
        }];
        let payload = scan_payload("./petstore.yaml", &spec, &[m], &hazards);
        assert_eq!(payload["source"], "./petstore.yaml");
        assert_eq!(payload["openapi"], "3.1.0");
        assert_eq!(payload["title"], "Petstore");
        assert_eq!(
            payload["modules"][0]["warnings"][0],
            "no 2xx response defined; output_schema is empty"
        );
        assert_eq!(payload["hazards"][0]["module_id"], "createPets");
        assert!(
            payload["modules"][0].get("hazards").is_none(),
            "hazards are a top-level key, not a module field"
        );
    }

    #[test]
    fn scan_table_names_the_document_and_the_routes() {
        let spec = json!({
            "openapi": "3.1.0",
            "info": {"title": "Petstore", "version": "1.0.0"},
            "paths": {}
        });
        let modules = vec![module("listPets", "GET", "/pets")];
        let out = render_scan_table("./petstore.yaml", &spec, &modules);
        assert!(out.starts_with("1 operation from ./petstore.yaml (OpenAPI 3.1.0, Petstore 1.0.0)"));
        assert!(out.contains("GET /pets"), "{out}");
        assert!(out.contains("Module ID"), "{out}");
    }

    #[test]
    fn diagnostics_render_warnings_and_hazards_separately() {
        let mut m = module("showPetById", "GET", "/pets/{petId}");
        m.warnings.push("no 2xx response defined".to_string());
        let hazards = vec![Hazard {
            module_id: "createPets".to_string(),
            http_method: "POST".to_string(),
            url_path: "/pets".to_string(),
            parameters: vec!["a".to_string(), "b".to_string()],
        }];
        let out = render_diagnostics(&[m], &hazards);
        assert!(out.contains("1 warning:"), "{out}");
        assert!(
            out.contains("showPetById  no 2xx response defined"),
            "{out}"
        );
        assert!(
            out.contains("1 operation cannot be proxied by FE-15b:"),
            "{out}"
        );
        assert!(
            out.contains("POST with 2 'in: query' parameters: a, b"),
            "{out}"
        );
    }

    #[test]
    fn diagnostics_are_empty_for_a_clean_scan() {
        assert_eq!(render_diagnostics(&[module("m", "GET", "/x")], &[]), "");
    }
}
