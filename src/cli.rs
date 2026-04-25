// apcore-cli — Core CLI dispatcher.
// Protocol spec: FE-01 (build_module_command, collect_input,
//                        validate_module_id, set_audit_logger, dispatch_module)

use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use thiserror::Error;

use crate::security::AuditLogger;

// NOTE: LazyModuleGroup, GroupedModuleGroup, ModuleExecutor trait, and
// ApCoreExecutorAdapter were deleted per audit findings D9-001..004. They
// were Python Click-hierarchy ports that did not fit clap's model. Actual
// dispatch is handled directly by dispatch_module() below; the dispatcher
// calls the concrete `apcore::Executor` without a trait-object indirection.

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors produced by CLI dispatch operations.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("invalid module id: {0}")]
    InvalidModuleId(String),

    #[error("reserved module id: '{0}' conflicts with a built-in command name")]
    ReservedModuleId(String),

    #[error("stdin read error: {0}")]
    StdinRead(String),

    #[error("json parse error: {0}")]
    JsonParse(String),

    #[error("input too large (limit {limit} bytes, got {actual} bytes)")]
    InputTooLarge { limit: usize, actual: usize },

    #[error("expected JSON object, got a different type")]
    NotAnObject,

    /// Schema $ref resolution failed (circular, missing target, max depth).
    /// Routed to `EXIT_SCHEMA_CIRCULAR_REF` (48) by `cli_error_exit_code`.
    #[error("schema $ref resolution failed for module '{module_id}': {source}")]
    SchemaRefResolution {
        module_id: String,
        source: crate::ref_resolver::RefResolverError,
    },
}

impl CliError {
    /// Map a `CliError` to the protocol-spec exit code so callers don't have
    /// to switch on the variant inline.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::SchemaRefResolution { .. } => crate::EXIT_SCHEMA_CIRCULAR_REF,
            _ => crate::EXIT_INVALID_INPUT,
        }
    }
}

// ---------------------------------------------------------------------------
// Global verbose help flag (controls built-in option visibility in help)
// ---------------------------------------------------------------------------

/// Whether --verbose was passed (controls help detail level).
static VERBOSE_HELP: AtomicBool = AtomicBool::new(false);

/// Set the verbose help flag. When false, built-in options are hidden
/// from help.
pub fn set_verbose_help(verbose: bool) {
    VERBOSE_HELP.store(verbose, Ordering::Relaxed);
}

/// Check the verbose help flag.
pub fn is_verbose_help() -> bool {
    VERBOSE_HELP.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Global docs URL (shown in help and man pages)
// ---------------------------------------------------------------------------

/// Base URL for online documentation. `None` means no link shown.
static DOCS_URL: Mutex<Option<String>> = Mutex::new(None);

/// Set the base URL for online documentation links in help and man
/// pages. Pass `None` to disable. Command-level help appends
/// `/commands/{name}` automatically.
///
/// # Example
/// ```
/// apcore_cli::cli::set_docs_url(Some("https://docs.apcore.dev/cli".into()));
/// ```
pub fn set_docs_url(url: Option<String>) {
    if let Ok(mut guard) = DOCS_URL.lock() {
        *guard = url;
    }
}

/// Get the current docs URL (if set).
pub fn get_docs_url() -> Option<String> {
    match DOCS_URL.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Global audit logger (module-level singleton, set once at startup)
// ---------------------------------------------------------------------------

static AUDIT_LOGGER: Mutex<Option<AuditLogger>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Global executable map (module name -> script path, set once at startup)
// ---------------------------------------------------------------------------

static EXECUTABLES: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();

/// Store the executable map built during module discovery.
///
/// Must be called once before any `dispatch_module` invocation.
pub fn set_executables(map: HashMap<String, PathBuf>) {
    let _ = EXECUTABLES.set(map);
}

/// Set (or clear) the global audit logger used by all module commands.
///
/// Pass `None` to disable auditing. Typically called once during CLI
/// initialisation, before any commands are dispatched.
pub fn set_audit_logger(audit_logger: Option<AuditLogger>) {
    match AUDIT_LOGGER.lock() {
        Ok(mut guard) => {
            *guard = audit_logger;
        }
        Err(_poisoned) => {
            tracing::warn!("AUDIT_LOGGER mutex poisoned — audit logger not updated");
        }
    }
}

/// Centralised audit-log entry point. Acquires the global lock once, calls
/// `log_execution`, and silently no-ops when the logger is disabled or the
/// mutex is poisoned. Both helpers below delegate here so every dispatch_module
/// exit path goes through the same shape — closes the gap where the standard
/// Err path called `log_execution` but the stream/trace Err paths did not
/// (review #7).
fn audit_log_entry(module_id: &str, input: &Value, status: &str, exit_code: i32, duration_ms: u64) {
    if let Ok(guard) = AUDIT_LOGGER.lock() {
        if let Some(logger) = guard.as_ref() {
            logger.log_execution(module_id, input, status, exit_code, duration_ms);
        }
    }
}

/// Record a successful module execution. Exit code is 0.
fn audit_success(module_id: &str, input: &Value, duration_ms: u64) {
    audit_log_entry(module_id, input, "success", 0, duration_ms);
}

/// Record a failed module execution. `exit_code` is the protocol-spec code
/// returned to the caller.
fn audit_error(module_id: &str, input: &Value, exit_code: i32, duration_ms: u64) {
    audit_log_entry(module_id, input, "error", exit_code, duration_ms);
}

// ---------------------------------------------------------------------------
// exec_command — clap subcommand builder for `exec`
// ---------------------------------------------------------------------------

/// Add the standard dispatch flags (--input, --yes, --large-input, --format,
/// --sandbox) to a clap Command. Used by both `exec_command()` and the external
/// subcommand re-parser in main.rs.
pub fn add_dispatch_flags(cmd: clap::Command) -> clap::Command {
    use clap::{Arg, ArgAction};
    let hide = !is_verbose_help();
    cmd.arg(
        Arg::new("input")
            .long("input")
            .value_name("SOURCE")
            .help(
                "Read JSON input from a file path, \
                 or use '-' to read from stdin pipe",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("yes")
            .long("yes")
            .short('y')
            .action(ArgAction::SetTrue)
            .help(
                "Skip interactive approval prompts \
                 (for scripts and CI)",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("large-input")
            .long("large-input")
            .action(ArgAction::SetTrue)
            .help(
                "Allow stdin input larger than 10MB \
                 (default limit protects against \
                 accidental pipes)",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("format")
            .long("format")
            .value_parser(["table", "json", "csv", "yaml", "jsonl"])
            .help(
                "Output format: json, table, csv, \
                 yaml, jsonl.",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("fields")
            .long("fields")
            .value_name("FIELDS")
            .help(
                "Comma-separated dot-paths to select \
                 from the result (e.g., 'status,data.count').",
            )
            .hide(hide),
    )
    .arg(
        // --sandbox is always hidden (not yet implemented)
        Arg::new("sandbox")
            .long("sandbox")
            .action(ArgAction::SetTrue)
            .help(
                "Run module in an isolated subprocess \
                 with restricted filesystem and env \
                 access",
            )
            .hide(true),
    )
    .arg(
        Arg::new("dry-run")
            .long("dry-run")
            .action(ArgAction::SetTrue)
            .help(
                "Run preflight checks without executing \
                 the module. Shows validation results.",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("trace")
            .long("trace")
            .action(ArgAction::SetTrue)
            .help(
                "Show execution pipeline trace with \
                 per-step timing after the result.",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("stream")
            .long("stream")
            .action(ArgAction::SetTrue)
            .help(
                "Stream module output as JSONL (one JSON \
                 object per line, flushed immediately).",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("strategy")
            .long("strategy")
            .value_parser(["standard", "internal", "testing", "performance", "minimal"])
            .value_name("STRATEGY")
            .help(
                "Execution pipeline strategy: standard \
                 (default), internal, testing, performance.",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("approval-timeout")
            .long("approval-timeout")
            .value_name("SECONDS")
            .help(
                "Override approval prompt timeout in \
                 seconds (default: 60).",
            )
            .hide(hide),
    )
    .arg(
        Arg::new("approval-token")
            .long("approval-token")
            .value_name("TOKEN")
            .help(
                "Resume a pending approval with the \
                 given token (for async approval flows).",
            )
            .hide(hide),
    )
}

/// Build the `exec` clap subcommand.
///
/// `exec` runs an apcore module by its fully-qualified module ID.
pub fn exec_command() -> clap::Command {
    use clap::{Arg, Command};

    let cmd = Command::new("exec").about("Execute an apcore module").arg(
        Arg::new("module_id")
            .required(true)
            .value_name("MODULE_ID")
            .help("Fully-qualified module ID to execute"),
    );
    add_dispatch_flags(cmd)
}

// ---------------------------------------------------------------------------
// Reserved root-level names (FE-13)
// ---------------------------------------------------------------------------
//
// Pre-v0.7, apcore-cli maintained a flat list of built-in command names
// (`BUILTIN_COMMANDS`) that were reserved against business-module collisions.
// FE-13 collapses every former built-in under the reserved `apcli` group, so
// the only collision surface at the root is the `apcli` name itself. See
// `crate::builtin_group::RESERVED_GROUP_NAMES` for the canonical list.
//
// `BUILTIN_COMMANDS` is retained here as a deprecated alias (single-element
// slice containing "apcli") so downstream code that still imports the symbol
// from `apcore_cli::BUILTIN_COMMANDS` keeps compiling — the new name will be
// the only one in v0.8.
#[deprecated(
    since = "0.7.0",
    note = "Use `crate::builtin_group::RESERVED_GROUP_NAMES` instead. FE-13 retires the \
            pre-v0.7 flat built-in list; only the `apcli` group name is reserved now."
)]
pub const BUILTIN_COMMANDS: &[&str] = crate::builtin_group::RESERVED_GROUP_NAMES;

// LazyModuleGroup / GroupedModuleGroup / ModuleExecutor / ApCoreExecutorAdapter
// were deleted per audit findings D9-001..004. See the module-level comment at
// the top of this file. Multi-level grouping now happens at the clap::Command
// build time in main.rs (via schema_parser + dispatch flags), not via a
// separate Lazy/Grouped struct hierarchy.

// ---------------------------------------------------------------------------
// build_module_command
// ---------------------------------------------------------------------------

/// Built-in flag names added to every generated module command. A schema
/// property that collides with one of these names will cause
/// `std::process::exit(2)`.
const RESERVED_FLAG_NAMES: &[&str] = &[
    "approval-timeout",
    "approval-token",
    "dry-run",
    "fields",
    "format",
    "input",
    "large-input",
    "sandbox",
    "strategy",
    "stream",
    "trace",
    "verbose",
    "yes",
];

/// Build a clap `Command` for a single module definition.
///
/// The resulting subcommand has:
/// * its `name` set to `module_def.name`
/// * its `about` derived from the module descriptor (empty if unavailable)
/// * the built-in dispatch flags (`--input`, `--yes`/`-y`, `--large-input`,
///   `--format`, `--sandbox`, `--dry-run`, `--trace`, `--stream`, `--strategy`,
///   `--fields`, `--approval-timeout`, `--approval-token`)
/// * schema-derived flags from `schema_to_clap_args`
///
/// The executor is NOT embedded in the `clap::Command` — clap has no
/// user-data attachment. Dispatch is handled separately by `dispatch_module`
/// which receives the executor as a parameter.
///
/// # Errors
/// Returns `CliError::ReservedModuleId` when `module_def.name` is one of the
/// reserved built-in command names.
///
/// **Design note (audit D9):** This is a convenience wrapper over
/// [`build_module_command_with_limit`] that supplies the default
/// `HELP_TEXT_MAX_LEN`. Audit D9 flagged the pair as bloat, but the wrapper
/// is consumed by `main.rs:624` and 8+ unit tests as the ergonomic default
/// form. Migrating those callers to construct an explicit limit at every site
/// would add ~15 lines of churn for a 1-line save. Retained intentionally.
pub fn build_module_command(
    module_def: &apcore::registry::registry::ModuleDescriptor,
) -> Result<clap::Command, CliError> {
    build_module_command_with_limit(module_def, crate::schema_parser::HELP_TEXT_MAX_LEN)
}

/// Build a clap `Command` for a single module definition with a configurable
/// help text max length.
pub fn build_module_command_with_limit(
    module_def: &apcore::registry::registry::ModuleDescriptor,
    help_text_max_length: usize,
) -> Result<clap::Command, CliError> {
    let module_id = &module_def.module_id;

    // Guard: reject reserved command names immediately (FE-13 §4.10).
    if crate::builtin_group::RESERVED_GROUP_NAMES.contains(&module_id.as_str()) {
        return Err(CliError::ReservedModuleId(module_id.clone()));
    }

    // Resolve $ref pointers in the input schema before generating clap args.
    // Failures (circular ref, missing target, max-depth exceeded) propagate
    // as SchemaRefResolution so the user sees EXIT_SCHEMA_CIRCULAR_REF (48)
    // — previously the error was swallowed via .unwrap_or_else and the user
    // got a downstream clap parse error built from un-resolved $refs (review #8).
    let resolved_schema =
        crate::ref_resolver::resolve_refs(&module_def.input_schema, 32, module_id).map_err(
            |e| CliError::SchemaRefResolution {
                module_id: module_id.clone(),
                source: e,
            },
        )?;

    // Build clap args from JSON Schema properties.
    let schema_args = crate::schema_parser::schema_to_clap_args_with_limit(
        &resolved_schema,
        help_text_max_length,
    )
    .map_err(|e| CliError::InvalidModuleId(format!("schema parse error: {e}")))?;

    // Check for schema property names that collide with built-in flags.
    for arg in &schema_args.args {
        if let Some(long) = arg.get_long() {
            if RESERVED_FLAG_NAMES.contains(&long) {
                return Err(CliError::ReservedModuleId(format!(
                    "module '{module_id}' schema property '{long}' conflicts \
                     with a reserved CLI option name"
                )));
            }
        }
    }

    let hide = !is_verbose_help();

    // Build after_help footer: verbose hint + optional docs link
    let mut footer_parts = Vec::new();
    if hide {
        footer_parts.push(
            "Use --verbose to show all options \
             (including built-in apcore options)."
                .to_string(),
        );
    }
    if let Some(url) = get_docs_url() {
        footer_parts.push(format!("Docs: {url}/commands/{module_id}"));
    }
    let footer = footer_parts.join("\n");

    let mut cmd = add_dispatch_flags(clap::Command::new(module_id.clone()).after_help(footer));

    // Attach schema-derived args.
    for arg in schema_args.args {
        cmd = cmd.arg(arg);
    }

    Ok(cmd)
}

// ---------------------------------------------------------------------------
// collect_input
// ---------------------------------------------------------------------------

const STDIN_SIZE_LIMIT_BYTES: usize = 10 * 1024 * 1024; // 10 MiB

/// Inner implementation: accepts any `Read` source for testability.
///
/// # Arguments
/// * `stdin_flag`  — `Some("-")` to read from `reader`, anything else skips STDIN
/// * `cli_kwargs`  — map of flag name → value (`Null` values are dropped)
/// * `large_input` — if `false`, reject payloads exceeding `STDIN_SIZE_LIMIT_BYTES`
/// * `reader`      — byte source to read from when `stdin_flag == Some("-")`
///
/// # Errors
/// Returns `CliError` on oversized input, invalid JSON, or non-object JSON.
pub fn collect_input_from_reader<R: Read>(
    stdin_flag: Option<&str>,
    cli_kwargs: HashMap<String, Value>,
    large_input: bool,
    mut reader: R,
) -> Result<HashMap<String, Value>, CliError> {
    // Drop Null values from CLI kwargs.
    let cli_non_null: HashMap<String, Value> = cli_kwargs
        .into_iter()
        .filter(|(_, v)| !v.is_null())
        .collect();

    if stdin_flag != Some("-") {
        return Ok(cli_non_null);
    }

    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| CliError::StdinRead(e.to_string()))?;

    if !large_input && buf.len() > STDIN_SIZE_LIMIT_BYTES {
        return Err(CliError::InputTooLarge {
            limit: STDIN_SIZE_LIMIT_BYTES,
            actual: buf.len(),
        });
    }

    if buf.is_empty() {
        return Ok(cli_non_null);
    }

    let stdin_value: Value =
        serde_json::from_slice(&buf).map_err(|e| CliError::JsonParse(e.to_string()))?;

    let stdin_map = match stdin_value {
        Value::Object(m) => m,
        _ => return Err(CliError::NotAnObject),
    };

    // Merge: STDIN base, CLI kwargs override on collision.
    let mut merged: HashMap<String, Value> = stdin_map.into_iter().collect();
    merged.extend(cli_non_null);
    Ok(merged)
}

/// Merge CLI keyword arguments with optional JSON input.
///
/// Resolution order (highest priority first):
/// 1. CLI flags (non-`Null` values in `cli_kwargs`)
/// 2. JSON from `stdin_flag`:
///    - `Some("-")` → read from stdin
///    - `Some(path)` → read from file at `path`
///    - `None` → no JSON input, return CLI kwargs only
///
/// # Arguments
/// * `stdin_flag`  — `Some("-")` for stdin, `Some(path)` for a file, `None` to skip
/// * `cli_kwargs`  — map of flag name → value (`Null` values are ignored)
/// * `large_input` — if `false`, reject payloads exceeding 10 MiB
///
/// # Errors
/// Returns `CliError` (exit code 2) on oversized input, invalid JSON, non-object
/// JSON, or file open failures.
pub fn collect_input(
    stdin_flag: Option<&str>,
    cli_kwargs: HashMap<String, Value>,
    large_input: bool,
) -> Result<HashMap<String, Value>, CliError> {
    match stdin_flag {
        None | Some("") => {
            collect_input_from_reader(None, cli_kwargs, large_input, std::io::stdin())
        }
        Some("-") => {
            collect_input_from_reader(Some("-"), cli_kwargs, large_input, std::io::stdin())
        }
        Some(path) => {
            let file = std::fs::File::open(path).map_err(|e| {
                CliError::StdinRead(format!("cannot open input file '{}': {}", path, e))
            })?;
            collect_input_from_reader(Some("-"), cli_kwargs, large_input, file)
        }
    }
}

// ---------------------------------------------------------------------------
// validate_module_id
// ---------------------------------------------------------------------------

/// Maximum allowed length for a CLI-supplied module ID.
///
/// Tracks PROTOCOL_SPEC §2.7 EBNF constraint #1 — bumped from 128 to 192 in
/// spec 1.6.0-draft to accommodate Java/.NET deep-namespace FQN-derived IDs.
/// Filesystem-safe: `192 + ".binding.yaml".len() = 205 < 255`-byte filename
/// limit on ext4/xfs/NTFS/APFS/btrfs.
const MODULE_ID_MAX_LEN: usize = 192;

/// Validate a module identifier.
///
/// # Rules
/// * Maximum 192 characters (PROTOCOL_SPEC §2.7)
/// * Matches `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$`
/// * No leading/trailing dots, no consecutive dots
/// * Must not start with a digit or uppercase letter
///
/// # Errors
/// Returns `CliError::InvalidModuleId` (exit code 2) on any violation.
pub fn validate_module_id(module_id: &str) -> Result<(), CliError> {
    if module_id.len() > MODULE_ID_MAX_LEN {
        return Err(CliError::InvalidModuleId(format!(
            "Invalid module ID format: '{module_id}'. Maximum length is {MODULE_ID_MAX_LEN} characters."
        )));
    }
    if !is_valid_module_id(module_id) {
        return Err(CliError::InvalidModuleId(format!(
            "Invalid module ID format: '{module_id}'."
        )));
    }
    Ok(())
}

/// Hand-written validator matching `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$`.
///
/// Does not require the `regex` crate.
#[inline]
fn is_valid_module_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Split on '.' and validate each segment individually.
    for segment in s.split('.') {
        if segment.is_empty() {
            // Catches leading dot, trailing dot, and consecutive dots.
            return false;
        }
        let mut chars = segment.chars();
        // First character must be a lowercase ASCII letter.
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() => {}
            _ => return false,
        }
        // Remaining characters: lowercase letter, ASCII digit, or underscore.
        for c in chars {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' {
                return false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Error code mapping
// ---------------------------------------------------------------------------

/// Map an apcore error code string to the appropriate CLI exit code.
///
/// Exit code table:
/// * `MODULE_NOT_FOUND` / `MODULE_LOAD_ERROR` / `MODULE_DISABLED` → 44
/// * `SCHEMA_VALIDATION_ERROR`                                     → 45
/// * `APPROVAL_DENIED` / `APPROVAL_TIMEOUT` / `APPROVAL_PENDING`  → 46
/// * `CONFIG_NOT_FOUND` / `CONFIG_INVALID`                         → 47
/// * `SCHEMA_CIRCULAR_REF`                                         → 48
/// * `ACL_DENIED`                                                  → 77
/// * everything else (including `MODULE_EXECUTE_ERROR` / `MODULE_TIMEOUT`) → 1
pub(crate) fn map_apcore_error_to_exit_code(error_code: &str) -> i32 {
    use crate::{
        EXIT_ACL_DENIED, EXIT_APPROVAL_DENIED, EXIT_CONFIG_BIND_ERROR, EXIT_CONFIG_MOUNT_ERROR,
        EXIT_CONFIG_NAMESPACE_RESERVED, EXIT_CONFIG_NOT_FOUND, EXIT_ERROR_FORMATTER_DUPLICATE,
        EXIT_MODULE_EXECUTE_ERROR, EXIT_MODULE_NOT_FOUND, EXIT_SCHEMA_CIRCULAR_REF,
        EXIT_SCHEMA_VALIDATION_ERROR,
    };
    match error_code {
        "MODULE_NOT_FOUND" | "MODULE_LOAD_ERROR" | "MODULE_DISABLED" => EXIT_MODULE_NOT_FOUND,
        "SCHEMA_VALIDATION_ERROR" => EXIT_SCHEMA_VALIDATION_ERROR,
        "APPROVAL_DENIED" | "APPROVAL_TIMEOUT" | "APPROVAL_PENDING" => EXIT_APPROVAL_DENIED,
        "CONFIG_NOT_FOUND" | "CONFIG_INVALID" => EXIT_CONFIG_NOT_FOUND,
        "SCHEMA_CIRCULAR_REF" => EXIT_SCHEMA_CIRCULAR_REF,
        "ACL_DENIED" => EXIT_ACL_DENIED,
        // Config Bus errors (apcore >= 0.15.0)
        "CONFIG_NAMESPACE_RESERVED"
        | "CONFIG_NAMESPACE_DUPLICATE"
        | "CONFIG_ENV_PREFIX_CONFLICT"
        | "CONFIG_ENV_MAP_CONFLICT" => EXIT_CONFIG_NAMESPACE_RESERVED,
        "CONFIG_MOUNT_ERROR" => EXIT_CONFIG_MOUNT_ERROR,
        "CONFIG_BIND_ERROR" => EXIT_CONFIG_BIND_ERROR,
        "ERROR_FORMATTER_DUPLICATE" => EXIT_ERROR_FORMATTER_DUPLICATE,
        _ => EXIT_MODULE_EXECUTE_ERROR,
    }
}

/// Map an `apcore::errors::ModuleError` directly to an exit code.
///
/// Converts the `ErrorCode` enum variant to its SCREAMING_SNAKE_CASE
/// representation via serde JSON serialisation and delegates to
/// `map_apcore_error_to_exit_code`.
pub(crate) fn map_module_error_to_exit_code(err: &apcore::errors::ModuleError) -> i32 {
    // Serialise the ErrorCode enum to its SCREAMING_SNAKE_CASE string.
    let code_str = serde_json::to_value(err.code)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    map_apcore_error_to_exit_code(&code_str)
}

// ---------------------------------------------------------------------------
// Schema validation helper
// ---------------------------------------------------------------------------

/// Validate `input` against a JSON Schema object.
///
/// This is a lightweight inline checker sufficient until `jsonschema` crate
/// integration lands (FE-08).  It enforces the `required` array only — if
/// every field listed in `required` is present in `input`, the call succeeds.
///
/// # Errors
/// Returns `Err(String)` describing the first missing required field.
pub(crate) fn validate_against_schema(
    input: &HashMap<String, Value>,
    schema: &Value,
) -> Result<(), String> {
    // Extract "required" array if present.
    let required = match schema.get("required") {
        Some(Value::Array(arr)) => arr,
        _ => return Ok(()),
    };
    for req in required {
        if let Some(field_name) = req.as_str() {
            if !input.contains_key(field_name) {
                return Err(format!("required field '{}' is missing", field_name));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dispatch_module — full execution pipeline
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// F3: Enhanced Error Output
// ---------------------------------------------------------------------------

/// Emit structured JSON error to stderr for AI agents / non-TTY consumers.
///
/// When `error_data` is provided (from an apcore ModuleError), its fields
/// (`code`, `details`, `suggestion`, `ai_guidance`, `retryable`,
/// `user_fixable`) are included in the output per FE-11 spec section 3.3.
fn emit_error_json(
    _module_id: &str,
    message: &str,
    exit_code: i32,
    error_data: Option<&serde_json::Value>,
) {
    let mut payload = serde_json::json!({
        "error": true,
        "code": "UNKNOWN",
        "message": message,
        "exit_code": exit_code,
    });
    // Overlay fields from the structured error if available.
    if let Some(data) = error_data {
        if let Some(obj) = data.as_object() {
            for key in &[
                "code",
                "message",
                "details",
                "suggestion",
                "ai_guidance",
                "retryable",
                "user_fixable",
            ] {
                if let Some(val) = obj.get(*key) {
                    if !val.is_null() {
                        payload[*key] = val.clone();
                    }
                }
            }
        }
    }
    eprintln!("{}", serde_json::to_string(&payload).unwrap_or_default());
}

/// Emit human-readable error to stderr with structured guidance fields.
///
/// Shows `[CODE]` header, `Details:` block, `Suggestion:`, and `Retryable:`
/// labels. Hides `ai_guidance` and `user_fixable` (machine-oriented fields).
fn emit_error_tty(
    _module_id: &str,
    message: &str,
    exit_code: i32,
    error_data: Option<&serde_json::Value>,
) {
    // Header with error code.
    if let Some(code) = error_data
        .and_then(|d| d.get("code"))
        .and_then(|v| v.as_str())
    {
        eprintln!("Error [{code}]: {message}");
    } else {
        eprintln!("Error: {message}");
    }

    // Details block.
    if let Some(details) = error_data
        .and_then(|d| d.get("details"))
        .and_then(|v| v.as_object())
    {
        eprintln!("\n  Details:");
        for (k, v) in details {
            eprintln!("    {k}: {v}");
        }
    }

    // Suggestion.
    if let Some(suggestion) = error_data
        .and_then(|d| d.get("suggestion"))
        .and_then(|v| v.as_str())
    {
        eprintln!("\n  Suggestion: {suggestion}");
    }

    // Retryable.
    if let Some(retryable) = error_data
        .and_then(|d| d.get("retryable"))
        .and_then(|v| v.as_bool())
    {
        let label = if retryable {
            "Yes"
        } else {
            "No (same input will fail again)"
        };
        eprintln!("  Retryable: {label}");
    }

    eprintln!("\n  Exit code: {exit_code}");
}

// ---------------------------------------------------------------------------
// Boolean pair reconciliation
// ---------------------------------------------------------------------------

/// Reconcile --flag / --no-flag boolean pairs from ArgMatches into bool values.
///
/// For each BoolFlagPair:
/// - If --flag was set  → prop_name = true
/// - If --no-flag set   → prop_name = false
/// - If neither         → prop_name = default_val
pub fn reconcile_bool_pairs(
    matches: &clap::ArgMatches,
    bool_pairs: &[crate::schema_parser::BoolFlagPair],
) -> HashMap<String, Value> {
    let mut result = HashMap::new();
    for pair in bool_pairs {
        // Use try_get_one to avoid panicking when the flag doesn't exist
        // in ArgMatches (e.g. exec subcommand doesn't have schema-derived flags).
        let pos_set = matches
            .try_get_one::<bool>(&pair.prop_name)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false);
        let neg_id = format!("no-{}", pair.prop_name);
        let neg_set = matches
            .try_get_one::<bool>(&neg_id)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false);
        let val = if pos_set {
            true
        } else if neg_set {
            false
        } else {
            pair.default_val
        };
        result.insert(pair.prop_name.clone(), Value::Bool(val));
    }
    result
}

/// Extract schema-derived CLI kwargs from `ArgMatches` for a given module.
///
/// Iterates schema properties and extracts string values from clap matches.
/// Boolean pairs are handled separately via `reconcile_bool_pairs`.
fn extract_cli_kwargs(
    matches: &clap::ArgMatches,
    module_def: &apcore::registry::registry::ModuleDescriptor,
) -> HashMap<String, Value> {
    use crate::schema_parser::schema_to_clap_args;

    let schema_args = match schema_to_clap_args(&module_def.input_schema) {
        Ok(sa) => sa,
        Err(_) => return HashMap::new(),
    };

    let mut kwargs: HashMap<String, Value> = HashMap::new();

    // Extract non-boolean schema args as strings (or Null if absent).
    for arg in &schema_args.args {
        let id = arg.get_id().as_str().to_string();
        // Skip the no- counterparts of boolean args.
        if id.starts_with("no-") {
            continue;
        }
        // Use try_get_one to avoid panicking when the arg doesn't exist
        // in ArgMatches (e.g. exec subcommand doesn't have schema-derived flags).
        if let Ok(Some(val)) = matches.try_get_one::<String>(&id) {
            kwargs.insert(id, Value::String(val.clone()));
        } else if let Ok(Some(val)) = matches.try_get_one::<std::path::PathBuf>(&id) {
            kwargs.insert(id, Value::String(val.to_string_lossy().to_string()));
        } else {
            kwargs.insert(id, Value::Null);
        }
    }

    // Reconcile boolean pairs.
    let bool_vals = reconcile_bool_pairs(matches, &schema_args.bool_pairs);
    kwargs.extend(bool_vals);

    // Apply enum type reconversion.
    crate::schema_parser::reconvert_enum_values(kwargs, &schema_args)
}

/// Execute a script-based module by spawning the executable as a subprocess.
///
/// JSON input is written to stdin; JSON output is read from stdout.
/// Stderr is captured and included in error messages on failure.
async fn execute_script(executable: &std::path::Path, input: &Value) -> Result<Value, String> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new(executable)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Ensure the child is killed if this future is dropped (e.g. on
        // SIGINT via the tokio::select! race at the call site) — tokio's
        // default is kill_on_drop=false, which would leak the subprocess.
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {}", executable.display(), e))?;

    // Write JSON input to child stdin then close it.
    if let Some(mut stdin) = child.stdin.take() {
        let payload =
            serde_json::to_vec(input).map_err(|e| format!("failed to serialize input: {e}"))?;
        stdin
            .write_all(&payload)
            .await
            .map_err(|e| format!("failed to write to stdin: {e}"))?;
        drop(stdin);
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("failed to read output: {e}"))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr_hint = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "script exited with code {code}{}",
            if stderr_hint.is_empty() {
                String::new()
            } else {
                format!(": {}", stderr_hint.trim())
            }
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("script stdout is not valid JSON: {e}"))
}

/// Execute a module by ID: validate → collect input → validate schema
/// → approve → execute → audit → output.
///
/// Calls `std::process::exit` with the appropriate code; never returns normally.
pub async fn dispatch_module(
    module_id: &str,
    matches: &clap::ArgMatches,
    registry: &Arc<dyn crate::discovery::RegistryProvider>,
    apcore_executor: &apcore::Executor,
) -> ! {
    use crate::{
        EXIT_APPROVAL_DENIED, EXIT_INVALID_INPUT, EXIT_MODULE_NOT_FOUND,
        EXIT_SCHEMA_VALIDATION_ERROR, EXIT_SIGINT, EXIT_SUCCESS,
    };

    // 1. Validate module ID format (exit 2 on bad format).
    if let Err(e) = validate_module_id(module_id) {
        eprintln!("Error: Invalid module ID format: '{module_id}'.");
        let _ = e;
        std::process::exit(EXIT_INVALID_INPUT);
    }

    // 2. Registry lookup (exit 44 if not found).
    let module_def = match registry.get_module_descriptor(module_id) {
        Some(def) => def,
        None => {
            eprintln!("Error: Module '{module_id}' not found in registry.");
            std::process::exit(EXIT_MODULE_NOT_FOUND);
        }
    };

    // 3. Extract built-in flags from matches.
    let stdin_flag = matches.get_one::<String>("input").map(|s| s.as_str());
    let auto_approve = matches.get_flag("yes");
    let large_input = matches.get_flag("large-input");
    let format_flag = matches.get_one::<String>("format").cloned();
    let fields_flag = matches.get_one::<String>("fields").cloned();
    let dry_run = matches.get_flag("dry-run");
    let trace_flag = matches.get_flag("trace");
    let stream_flag = matches.get_flag("stream");
    let strategy_name = matches.get_one::<String>("strategy").cloned();
    let approval_timeout_arg = matches.get_one::<String>("approval-timeout").cloned();
    let approval_token = matches.get_one::<String>("approval-token").cloned();

    // 4. Build CLI kwargs from schema-derived flags (stub: empty map).
    let cli_kwargs = extract_cli_kwargs(matches, &module_def);

    // 5. Collect and merge input (exit 2 on errors).
    let mut merged = match collect_input(stdin_flag, cli_kwargs, large_input) {
        Ok(m) => m,
        Err(CliError::InputTooLarge { .. }) => {
            eprintln!("Error: STDIN input exceeds 10MB limit. Use --large-input to override.");
            std::process::exit(EXIT_INVALID_INPUT);
        }
        Err(CliError::JsonParse(detail)) => {
            eprintln!("Error: STDIN does not contain valid JSON: {detail}.");
            std::process::exit(EXIT_INVALID_INPUT);
        }
        Err(CliError::NotAnObject) => {
            eprintln!("Error: STDIN JSON must be an object, got array or scalar.");
            std::process::exit(EXIT_INVALID_INPUT);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(EXIT_INVALID_INPUT);
        }
    };

    // -- F1: Dry-run / validate: preflight only, no execution --
    if dry_run {
        // --trace --dry-run: show pipeline preview after preflight result.
        let show_trace_preview = trace_flag;
        let print_pipeline_preview = || {
            if show_trace_preview {
                let pure_steps = [
                    "context_creation",
                    "call_chain_guard",
                    "module_lookup",
                    "acl_check",
                    "input_validation",
                ];
                let all_steps = [
                    "context_creation",
                    "call_chain_guard",
                    "module_lookup",
                    "acl_check",
                    "approval_gate",
                    "middleware_before",
                    "input_validation",
                    "execute",
                    "output_validation",
                    "middleware_after",
                    "return_result",
                ];
                eprintln!("\nPipeline preview (dry-run):");
                for s in &all_steps {
                    if pure_steps.contains(s) {
                        eprintln!("  v {:<24} (pure -- would execute)", s);
                    } else {
                        eprintln!("  o {:<24} (impure -- skipped in dry-run)", s);
                    }
                }
            }
        };
        let input_value =
            serde_json::to_value(&merged).unwrap_or(Value::Object(Default::default()));
        let preflight_input = serde_json::json!({
            "module_id": module_id,
            "input": input_value,
        });
        let result = apcore_executor
            .call("system.validate", preflight_input, None, None)
            .await;
        match result {
            Ok(preflight_val) => {
                crate::validate::format_preflight_result(&preflight_val, format_flag.as_deref());
                print_pipeline_preview();
                let valid = preflight_val
                    .get("valid")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if valid {
                    std::process::exit(EXIT_SUCCESS);
                } else {
                    std::process::exit(crate::EXIT_MODULE_EXECUTE_ERROR);
                }
            }
            Err(_e) => {
                tracing::debug!(
                    "system.validate call failed: {_e}; falling back to basic schema validation"
                );
                // Fallback: perform basic schema validation only.
                let schema_ok = if let Some(schema) = module_def.input_schema.as_object() {
                    if schema.contains_key("properties") {
                        validate_against_schema(&merged, &module_def.input_schema).is_ok()
                    } else {
                        true
                    }
                } else {
                    true
                };

                let checks = vec![
                    serde_json::json!({"check": "module_id", "passed": true}),
                    serde_json::json!({"check": "module_lookup", "passed": true}),
                    serde_json::json!({"check": "schema", "passed": schema_ok}),
                ];
                let preflight = serde_json::json!({
                    "valid": schema_ok,
                    "requires_approval": false,
                    "checks": checks,
                });
                crate::validate::format_preflight_result(&preflight, format_flag.as_deref());
                print_pipeline_preview();
                if schema_ok {
                    std::process::exit(EXIT_SUCCESS);
                } else {
                    std::process::exit(EXIT_SCHEMA_VALIDATION_ERROR);
                }
            }
        }
    }

    // 6. Schema validation (if module has input_schema with properties).
    if let Some(schema) = module_def.input_schema.as_object() {
        if schema.contains_key("properties") {
            if let Err(detail) = validate_against_schema(&merged, &module_def.input_schema) {
                eprintln!("Error: Validation failed: {detail}.");
                std::process::exit(EXIT_SCHEMA_VALIDATION_ERROR);
            }
        }
    }

    // -- F5: Inject approval token if provided --
    if let Some(ref token) = approval_token {
        merged.insert("_approval_token".to_string(), Value::String(token.clone()));
    }

    // 7. Approval gate (exit 46 on denial/timeout).
    // Resolve the timeout: --approval-timeout flag > cli.approval_timeout
    // config default > hardcoded 60s. Non-numeric values fall back to the
    // default rather than failing the dispatch.
    let approval_timeout_secs = approval_timeout_arg
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(crate::approval::DEFAULT_APPROVAL_TIMEOUT_SECS);
    let module_json = serde_json::to_value(&module_def).unwrap_or_default();
    if let Err(e) = crate::approval::check_approval_with_timeout(
        &module_json,
        auto_approve,
        approval_timeout_secs,
    )
    .await
    {
        eprintln!("Error: {e}");
        std::process::exit(EXIT_APPROVAL_DENIED);
    }

    // 8. Build merged input as serde_json::Value.
    let input_value = serde_json::to_value(&merged).unwrap_or(Value::Object(Default::default()));

    // Determine sandbox flag.
    let use_sandbox = matches.get_flag("sandbox");

    // Check if this module has a script-based executable.
    let script_executable = EXECUTABLES
        .get()
        .and_then(|map| map.get(module_id))
        .cloned();

    // -- F6: Streaming execution --
    if stream_flag {
        // Streaming always outputs JSONL; --format table is ignored (spec 3.6.2).
        if format_flag.as_deref() == Some("table") {
            eprintln!("Warning: Streaming mode always outputs JSONL; --format table is ignored.");
        }
        let start = std::time::Instant::now();
        // Stream outputs as JSONL.
        if let Some(exec_path) = script_executable.as_ref() {
            // Script-based: fall back to regular execution, output as JSONL.
            let res = tokio::select! {
                res = execute_script(exec_path, &input_value) => res,
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("Execution cancelled.");
                    std::process::exit(EXIT_SIGINT);
                }
            };
            let duration_ms = start.elapsed().as_millis() as u64;
            match res {
                Ok(val) => {
                    println!("{}", serde_json::to_string(&val).unwrap_or_default());
                    audit_success(module_id, &input_value, duration_ms);
                    std::process::exit(EXIT_SUCCESS);
                }
                Err(e) => {
                    audit_error(
                        module_id,
                        &input_value,
                        crate::EXIT_MODULE_EXECUTE_ERROR,
                        duration_ms,
                    );
                    eprintln!("Error: {e}");
                    std::process::exit(crate::EXIT_MODULE_EXECUTE_ERROR);
                }
            }
        }
        // In-process: use executor.stream() if available, else fall through.
        // apcore executor does not expose a stream() method in Rust yet,
        // so we fall back to standard call and output as single JSONL line.
        let res = tokio::select! {
            res = apcore_executor.call(
                module_id, input_value.clone(), None, None,
            ) => res,
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Execution cancelled.");
                std::process::exit(EXIT_SIGINT);
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;
        match res {
            Ok(val) => {
                if let Some(arr) = val.as_array() {
                    for item in arr {
                        println!("{}", serde_json::to_string(item).unwrap_or_default());
                    }
                } else {
                    println!("{}", serde_json::to_string(&val).unwrap_or_default());
                }
                audit_success(module_id, &input_value, duration_ms);
                std::process::exit(EXIT_SUCCESS);
            }
            Err(e) => {
                let code = map_module_error_to_exit_code(&e);
                audit_error(module_id, &input_value, code, duration_ms);
                eprintln!("Error: Module '{module_id}' execution failed: {e}.");
                std::process::exit(code);
            }
        }
    }

    // -- F4: Traced execution --
    if trace_flag {
        let start = std::time::Instant::now();
        // Use standard call; trace output is simulated from timing data.
        // Full PipelineTrace requires call_with_trace(), which may not be
        // available on all executor implementations.
        let res = tokio::select! {
            res = apcore_executor.call(
                module_id,
                input_value.clone(),
                None,
                None,
            ) => res,
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Execution cancelled.");
                std::process::exit(EXIT_SIGINT);
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;
        match res {
            Ok(output) => {
                audit_success(module_id, &input_value, duration_ms);
                // Print result with trace appended.
                let fmt = crate::output::resolve_format(format_flag.as_deref());
                if fmt == "json" {
                    // Merge trace stub into JSON output.
                    let trace_data = serde_json::json!({
                        "strategy": strategy_name.as_deref().unwrap_or("standard"),
                        "total_duration_ms": duration_ms,
                        "success": true,
                    });
                    let combined = if output.is_object() {
                        let mut obj = output.as_object().unwrap().clone();
                        obj.insert("_trace".to_string(), trace_data);
                        Value::Object(obj)
                    } else {
                        serde_json::json!({
                            "result": output,
                            "_trace": trace_data,
                        })
                    };
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&combined).unwrap_or_default()
                    );
                } else {
                    let out_str =
                        crate::output::format_exec_result(&output, fmt, fields_flag.as_deref());
                    println!("{out_str}");
                    eprintln!(
                        "\nPipeline Trace (strategy: {}, {duration_ms}ms)",
                        strategy_name.as_deref().unwrap_or("standard"),
                    );
                }
                std::process::exit(EXIT_SUCCESS);
            }
            Err(e) => {
                let code = map_module_error_to_exit_code(&e);
                audit_error(module_id, &input_value, code, duration_ms);
                eprintln!("Error: Module '{module_id}' execution failed: {e}.");
                std::process::exit(code);
            }
        }
    }

    // 9. Execute with SIGINT race (exit 130 on Ctrl-C).
    let start = std::time::Instant::now();

    // Unify the execution paths into Result<Value, (i32, String, Option<Value>)>
    // where the error tuple is (exit_code, display_message, optional_structured_error).
    let result: Result<Value, (i32, String, Option<Value>)> =
        if let Some(exec_path) = script_executable {
            // Script-based execution: spawn subprocess, pipe JSON via stdin/stdout.
            tokio::select! {
                res = execute_script(&exec_path, &input_value) => {
                    res.map_err(|e| (crate::EXIT_MODULE_EXECUTE_ERROR, e, None))
                }
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("Execution cancelled.");
                    std::process::exit(EXIT_SIGINT);
                }
            }
        } else if use_sandbox {
            let sandbox = crate::security::Sandbox::new(true, 0);
            tokio::select! {
                res = sandbox.execute(module_id, input_value.clone(), apcore_executor) => {
                    res.map_err(|e| {
                        // Preserve protocol exit-code semantics when the
                        // disabled passthrough surfaces an apcore ModuleError;
                        // other sandbox failures (NonZeroExit, Timeout,
                        // OutputParseFailed, SpawnFailed) map to the generic
                        // execute-error code.
                        match &e {
                            crate::security::ModuleExecutionError::ModuleError(inner) => {
                                let code = map_module_error_to_exit_code(inner);
                                let data = serde_json::to_value(inner).ok();
                                (code, e.to_string(), data)
                            }
                            _ => (crate::EXIT_MODULE_EXECUTE_ERROR, e.to_string(), None),
                        }
                    })
                }
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("Execution cancelled.");
                    std::process::exit(EXIT_SIGINT);
                }
            }
        } else {
            // Direct in-process executor call.
            // Note: strategy is configured at Executor construction, not per-call.
            // The strategy_name flag is available for future use when the Executor
            // supports per-call strategy overrides.
            tokio::select! {
                res = apcore_executor.call(
                    module_id,
                    input_value.clone(),
                    None,
                    None,
                ) => {
                    res.map_err(|e| {
                        let code = map_module_error_to_exit_code(&e);
                        // Serialize the ModuleError for F3 structured error output.
                        let data = serde_json::to_value(&e).ok();
                        (code, e.to_string(), data)
                    })
                }
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("Execution cancelled.");
                    std::process::exit(EXIT_SIGINT);
                }
            }
        };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => {
            // 10. Audit log success.
            audit_success(module_id, &input_value, duration_ms);
            // 11. Format and output (F9: with field selection).
            let fmt = crate::output::resolve_format(format_flag.as_deref());
            println!(
                "{}",
                crate::output::format_exec_result(&output, fmt, fields_flag.as_deref(),)
            );
            std::process::exit(EXIT_SUCCESS);
        }
        Err((exit_code, msg, error_data)) => {
            // Audit log error.
            audit_error(module_id, &input_value, exit_code, duration_ms);
            // F3: Enhanced error output with structured guidance fields.
            if format_flag.as_deref() == Some("json") || !std::io::stderr().is_terminal() {
                emit_error_json(module_id, &msg, exit_code, error_data.as_ref());
            } else {
                emit_error_tty(module_id, &msg, exit_code, error_data.as_ref());
            }
            std::process::exit(exit_code);
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_module_id_valid() {
        // Valid IDs must not return an error.
        for id in ["math.add", "text.summarize", "a", "a.b.c"] {
            let result = validate_module_id(id);
            assert!(result.is_ok(), "expected ok for '{id}': {result:?}");
        }
    }

    #[test]
    fn test_validate_module_id_too_long() {
        // PROTOCOL_SPEC §2.7 — bumped from 128 to 192 in spec 1.6.0-draft.
        let long_id = "a".repeat(193);
        assert!(validate_module_id(&long_id).is_err());
    }

    #[test]
    fn test_validate_module_id_invalid_format() {
        for id in ["INVALID!ID", "123abc", ".leading.dot", "a..b", "a."] {
            assert!(validate_module_id(id).is_err(), "expected error for '{id}'");
        }
    }

    #[test]
    fn test_validate_module_id_max_length() {
        // PROTOCOL_SPEC §2.7 — bumped from 128 to 192 in spec 1.6.0-draft.
        let max_id = "a".repeat(192);
        assert!(validate_module_id(&max_id).is_ok());
    }

    #[test]
    fn test_validate_module_id_over_max_length_message() {
        let overlong = "a".repeat(193);
        let err = validate_module_id(&overlong).expect_err("expected length error");
        assert!(format!("{err:?}").contains("Maximum length"));
    }

    // collect_input tests (TDD red → green)

    #[test]
    fn test_collect_input_no_stdin_drops_null_values() {
        use serde_json::json;
        let mut kwargs = HashMap::new();
        kwargs.insert("a".to_string(), json!(5));
        kwargs.insert("b".to_string(), Value::Null);

        let result = collect_input(None, kwargs, false).unwrap();
        assert_eq!(result.get("a"), Some(&json!(5)));
        assert!(!result.contains_key("b"), "Null values must be dropped");
    }

    #[test]
    fn test_collect_input_stdin_valid_json() {
        use serde_json::json;
        use std::io::Cursor;
        let stdin_bytes = b"{\"x\": 42}";
        let reader = Cursor::new(stdin_bytes.to_vec());
        let result = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap();
        assert_eq!(result.get("x"), Some(&json!(42)));
    }

    #[test]
    fn test_collect_input_cli_overrides_stdin() {
        use serde_json::json;
        use std::io::Cursor;
        let stdin_bytes = b"{\"a\": 5}";
        let reader = Cursor::new(stdin_bytes.to_vec());
        let mut kwargs = HashMap::new();
        kwargs.insert("a".to_string(), json!(99));
        let result = collect_input_from_reader(Some("-"), kwargs, false, reader).unwrap();
        assert_eq!(result.get("a"), Some(&json!(99)), "CLI must override STDIN");
    }

    #[test]
    fn test_collect_input_oversized_stdin_rejected() {
        use std::io::Cursor;
        let big = vec![b' '; 10 * 1024 * 1024 + 1];
        let reader = Cursor::new(big);
        let err = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap_err();
        assert!(matches!(err, CliError::InputTooLarge { .. }));
    }

    #[test]
    fn test_collect_input_large_input_allowed() {
        use std::io::Cursor;
        let mut payload = b"{\"k\": \"".to_vec();
        payload.extend(vec![b'x'; 11 * 1024 * 1024]);
        payload.extend(b"\"}");
        let reader = Cursor::new(payload);
        let result = collect_input_from_reader(Some("-"), HashMap::new(), true, reader);
        assert!(
            result.is_ok(),
            "large_input=true must accept oversized payload"
        );
    }

    #[test]
    fn test_collect_input_invalid_json_returns_error() {
        use std::io::Cursor;
        let reader = Cursor::new(b"not json at all".to_vec());
        let err = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap_err();
        assert!(matches!(err, CliError::JsonParse(_)));
    }

    #[test]
    fn test_collect_input_non_object_json_returns_error() {
        use std::io::Cursor;
        let reader = Cursor::new(b"[1, 2, 3]".to_vec());
        let err = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap_err();
        assert!(matches!(err, CliError::NotAnObject));
    }

    #[test]
    fn test_collect_input_empty_stdin_returns_empty_map() {
        use std::io::Cursor;
        let reader = Cursor::new(b"".to_vec());
        let result = collect_input_from_reader(Some("-"), HashMap::new(), false, reader).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_collect_input_no_stdin_flag_returns_cli_kwargs() {
        use serde_json::json;
        let mut kwargs = HashMap::new();
        kwargs.insert("foo".to_string(), json!("bar"));
        let result = collect_input(None, kwargs.clone(), false).unwrap();
        assert_eq!(result.get("foo"), Some(&json!("bar")));
    }

    #[test]
    fn test_collect_input_file_path_reads_json() {
        use serde_json::json;
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, r#"{{"port": 8080}}"#).unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let result = collect_input(Some(&path), HashMap::new(), false).unwrap();
        assert_eq!(result.get("port"), Some(&json!(8080)));
    }

    #[test]
    fn test_collect_input_file_path_cli_overrides_file() {
        use serde_json::json;
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, r#"{{"a": 1, "b": 2}}"#).unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut kwargs = HashMap::new();
        kwargs.insert("a".to_string(), json!(99));
        let result = collect_input(Some(&path), kwargs, false).unwrap();
        assert_eq!(result.get("a"), Some(&json!(99)), "CLI must override file");
        assert_eq!(result.get("b"), Some(&json!(2)));
    }

    #[test]
    fn test_collect_input_file_path_missing_returns_error() {
        let err =
            collect_input(Some("/nonexistent/path/data.json"), HashMap::new(), false).unwrap_err();
        assert!(matches!(err, CliError::StdinRead(_)));
    }

    // ---------------------------------------------------------------------------
    // build_module_command tests (TDD — RED written before GREEN)
    // ---------------------------------------------------------------------------

    /// Construct a minimal `ModuleDescriptor` for use in `build_module_command`
    /// tests. `input_schema` defaults to a JSON null (no properties) when
    /// `schema` is `None`.
    fn make_module_descriptor(
        name: &str,
        description: &str,
        schema: Option<serde_json::Value>,
    ) -> apcore::registry::registry::ModuleDescriptor {
        apcore::registry::registry::ModuleDescriptor {
            module_id: name.to_string(),
            name: None,
            description: description.to_string(),
            documentation: None,
            input_schema: schema.unwrap_or(serde_json::Value::Null),
            output_schema: serde_json::Value::Object(Default::default()),
            version: "1.0.0".to_string(),
            tags: vec![],
            annotations: Some(apcore::module::ModuleAnnotations::default()),
            examples: vec![],
            metadata: std::collections::HashMap::new(),
            display: None,
            sunset_date: None,
            dependencies: vec![],
            enabled: true,
        }
    }

    #[test]
    fn test_build_module_command_name_is_set() {
        let module = make_module_descriptor("math.add", "Add two numbers", None);
        let cmd = build_module_command(&module).unwrap();
        assert_eq!(cmd.get_name(), "math.add");
    }

    #[test]
    fn test_build_module_command_has_input_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let cmd = build_module_command(&module).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"input"), "must have --input flag");
    }

    #[test]
    fn test_build_module_command_has_yes_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let cmd = build_module_command(&module).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"yes"), "must have --yes flag");
    }

    #[test]
    fn test_build_module_command_has_large_input_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let cmd = build_module_command(&module).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(
            names.contains(&"large-input"),
            "must have --large-input flag"
        );
    }

    #[test]
    fn test_build_module_command_has_format_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let cmd = build_module_command(&module).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"format"), "must have --format flag");
    }

    #[test]
    fn test_build_module_command_has_sandbox_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let cmd = build_module_command(&module).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"sandbox"), "must have --sandbox flag");
    }

    #[test]
    fn test_build_module_command_reserved_name_returns_error() {
        // FE-13: only the `apcli` group name is reserved now. Former built-ins
        // (`list`, `describe`, etc.) live under the apcli group and no longer
        // collide at the root.
        for reserved in crate::builtin_group::RESERVED_GROUP_NAMES {
            let module = make_module_descriptor(reserved, "desc", None);
            let result = build_module_command(&module);
            assert!(
                matches!(result, Err(CliError::ReservedModuleId(_))),
                "expected ReservedModuleId for '{reserved}', got {result:?}"
            );
        }
    }

    #[test]
    fn test_build_module_command_former_builtin_names_allowed() {
        // Regression guard: `list`, `describe`, `health` etc. used to be
        // reserved; FE-13 retires that list. They must build cleanly now.
        for name in &["list", "describe", "exec", "init", "health", "config"] {
            let module = make_module_descriptor(name, "desc", None);
            let result = build_module_command(&module);
            assert!(
                result.is_ok(),
                "former built-in '{name}' should no longer be reserved; got {result:?}"
            );
        }
    }

    #[test]
    fn test_build_module_command_yes_has_short_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let cmd = build_module_command(&module).unwrap();
        let has_short_y = cmd
            .get_opts()
            .filter(|a| a.get_long() == Some("yes"))
            .any(|a| a.get_short() == Some('y'));
        assert!(has_short_y, "--yes must have short flag -y");
    }

    // ---------------------------------------------------------------------------
    // Reserved name invariants (FE-13)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_reserved_group_names_single_entry() {
        // FE-13: all former built-ins now live under `apcli`, so the only
        // reserved root-level name is `apcli` itself.
        assert_eq!(crate::builtin_group::RESERVED_GROUP_NAMES, &["apcli"]);
    }

    #[test]
    fn test_apcli_subcommand_names_matches_spec() {
        // Spec §4.1 subcommand table — 13 entries registered under `apcli`.
        let expected: &[&str] = &[
            "list",
            "describe",
            "exec",
            "validate",
            "init",
            "health",
            "usage",
            "enable",
            "disable",
            "reload",
            "config",
            "completion",
            "describe-pipeline",
        ];
        assert_eq!(crate::builtin_group::APCLI_SUBCOMMAND_NAMES, expected);
    }

    // ---------------------------------------------------------------------------
    // map_apcore_error_to_exit_code tests (RED — written before implementation)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_map_error_module_not_found_is_44() {
        assert_eq!(map_apcore_error_to_exit_code("MODULE_NOT_FOUND"), 44);
    }

    #[test]
    fn test_map_error_module_load_error_is_44() {
        assert_eq!(map_apcore_error_to_exit_code("MODULE_LOAD_ERROR"), 44);
    }

    #[test]
    fn test_map_error_module_disabled_is_44() {
        assert_eq!(map_apcore_error_to_exit_code("MODULE_DISABLED"), 44);
    }

    #[test]
    fn test_map_error_schema_validation_error_is_45() {
        assert_eq!(map_apcore_error_to_exit_code("SCHEMA_VALIDATION_ERROR"), 45);
    }

    #[test]
    fn test_map_error_approval_denied_is_46() {
        assert_eq!(map_apcore_error_to_exit_code("APPROVAL_DENIED"), 46);
    }

    #[test]
    fn test_map_error_approval_timeout_is_46() {
        assert_eq!(map_apcore_error_to_exit_code("APPROVAL_TIMEOUT"), 46);
    }

    #[test]
    fn test_map_error_approval_pending_is_46() {
        assert_eq!(map_apcore_error_to_exit_code("APPROVAL_PENDING"), 46);
    }

    #[test]
    fn test_map_error_config_not_found_is_47() {
        assert_eq!(map_apcore_error_to_exit_code("CONFIG_NOT_FOUND"), 47);
    }

    #[test]
    fn test_map_error_config_invalid_is_47() {
        assert_eq!(map_apcore_error_to_exit_code("CONFIG_INVALID"), 47);
    }

    #[test]
    fn test_map_error_schema_circular_ref_is_48() {
        assert_eq!(map_apcore_error_to_exit_code("SCHEMA_CIRCULAR_REF"), 48);
    }

    #[test]
    fn test_map_error_acl_denied_is_77() {
        assert_eq!(map_apcore_error_to_exit_code("ACL_DENIED"), 77);
    }

    #[test]
    fn test_map_error_module_execute_error_is_1() {
        assert_eq!(map_apcore_error_to_exit_code("MODULE_EXECUTE_ERROR"), 1);
    }

    #[test]
    fn test_map_error_module_timeout_is_1() {
        assert_eq!(map_apcore_error_to_exit_code("MODULE_TIMEOUT"), 1);
    }

    #[test]
    fn test_map_error_unknown_is_1() {
        assert_eq!(map_apcore_error_to_exit_code("SOMETHING_UNEXPECTED"), 1);
    }

    #[test]
    fn test_map_error_empty_string_is_1() {
        assert_eq!(map_apcore_error_to_exit_code(""), 1);
    }

    // ---------------------------------------------------------------------------
    // set_audit_logger implementation tests (RED)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_set_audit_logger_none_clears_logger() {
        // Setting None must not panic and must leave AUDIT_LOGGER as None.
        set_audit_logger(None);
        let guard = AUDIT_LOGGER.lock().unwrap();
        assert!(guard.is_none(), "setting None must clear the audit logger");
    }

    #[test]
    fn test_set_audit_logger_some_stores_logger() {
        use crate::security::AuditLogger;
        set_audit_logger(Some(AuditLogger::new(None)));
        let guard = AUDIT_LOGGER.lock().unwrap();
        assert!(guard.is_some(), "setting Some must store the audit logger");
        // Clean up.
        drop(guard);
        set_audit_logger(None);
    }

    // ---------------------------------------------------------------------------
    // validate_against_schema tests (RED)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_validate_against_schema_passes_with_no_properties() {
        let schema = serde_json::json!({});
        let input = std::collections::HashMap::new();
        // Schema without properties must not fail.
        let result = validate_against_schema(&input, &schema);
        assert!(result.is_ok(), "empty schema must pass: {result:?}");
    }

    #[test]
    fn test_validate_against_schema_required_field_missing_fails() {
        let schema = serde_json::json!({
            "properties": {
                "a": {"type": "integer"}
            },
            "required": ["a"]
        });
        let input: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        let result = validate_against_schema(&input, &schema);
        assert!(result.is_err(), "missing required field must fail");
    }

    #[test]
    fn test_validate_against_schema_required_field_present_passes() {
        let schema = serde_json::json!({
            "properties": {
                "a": {"type": "integer"}
            },
            "required": ["a"]
        });
        let mut input = std::collections::HashMap::new();
        input.insert("a".to_string(), serde_json::json!(42));
        let result = validate_against_schema(&input, &schema);
        assert!(
            result.is_ok(),
            "present required field must pass: {result:?}"
        );
    }

    #[test]
    fn test_validate_against_schema_no_required_any_input_passes() {
        let schema = serde_json::json!({
            "properties": {
                "x": {"type": "string"}
            }
        });
        let input: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        let result = validate_against_schema(&input, &schema);
        assert!(result.is_ok(), "no required fields: empty input must pass");
    }

    // GroupedModuleGroup / is_valid_group_name tests deleted along with the
    // corresponding implementations per audit findings D9-002 / D9-005.
}
