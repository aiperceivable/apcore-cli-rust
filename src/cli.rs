// apcore-cli — Core CLI dispatcher.
// Protocol spec: FE-01 (LazyModuleGroup equivalent, build_module_command,
//                        collect_input, validate_module_id, set_audit_logger)

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use thiserror::Error;

use crate::security::AuditLogger;

// ---------------------------------------------------------------------------
// Local trait abstractions for Executor
// ---------------------------------------------------------------------------
// apcore::Executor is a concrete struct, not a trait.
// This local trait allows LazyModuleGroup to be generic over both the real
// implementation and test mocks without depending on apcore internals.
// Registry access uses the unified `discovery::RegistryProvider` trait.

/// Minimal executor interface required by LazyModuleGroup.
pub trait ModuleExecutor: Send + Sync {}

/// Adapter that implements ModuleExecutor for the real apcore::Executor.
pub struct ApCoreExecutorAdapter(pub apcore::Executor);

impl ModuleExecutor for ApCoreExecutorAdapter {}

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

// ---------------------------------------------------------------------------
// exec_command — clap subcommand builder for `exec`
// ---------------------------------------------------------------------------

/// Add the standard dispatch flags (--input, --yes, --large-input, --format,
/// --sandbox) to a clap Command. Used by both `exec_command()` and the external
/// subcommand re-parser in main.rs.
pub fn add_dispatch_flags(cmd: clap::Command) -> clap::Command {
    use clap::{Arg, ArgAction};
    cmd.arg(
        Arg::new("input")
            .long("input")
            .value_name("SOURCE")
            .help("Input source (file path or '-' for stdin)"),
    )
    .arg(
        Arg::new("yes")
            .long("yes")
            .short('y')
            .action(ArgAction::SetTrue)
            .help("Auto-approve all confirmation prompts"),
    )
    .arg(
        Arg::new("large-input")
            .long("large-input")
            .action(ArgAction::SetTrue)
            .help("Allow larger-than-default input payloads"),
    )
    .arg(
        Arg::new("format")
            .long("format")
            .value_parser(["table", "json"])
            .help("Output format (table or json)"),
    )
    .arg(
        Arg::new("sandbox")
            .long("sandbox")
            .action(ArgAction::SetTrue)
            .help("Run module in subprocess sandbox"),
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
// LazyModuleGroup — lazy command builder
// ---------------------------------------------------------------------------

/// Built-in command names that are always present regardless of the registry.
pub const BUILTIN_COMMANDS: &[&str] = &["completion", "describe", "exec", "list", "man"];

/// Lazy command registry: builds module subcommands on-demand from the
/// apcore Registry, caching them after first construction.
///
/// This is the Rust equivalent of the Python `LazyModuleGroup` (Click group
/// subclass with lazy `get_command` / `list_commands`).
pub struct LazyModuleGroup {
    registry: Arc<dyn crate::discovery::RegistryProvider>,
    #[allow(dead_code)]
    executor: Arc<dyn ModuleExecutor>,
    /// Cache of module name -> name string (we store the name, not the Command,
    /// since clap::Command is not Clone in all configurations).
    module_cache: HashMap<String, bool>,
    /// Count of registry descriptor lookups (test instrumentation only).
    #[cfg(test)]
    pub registry_lookup_count: usize,
}

impl LazyModuleGroup {
    /// Create a new lazy module group.
    ///
    /// # Arguments
    /// * `registry` — module registry (real or mock)
    /// * `executor` — module executor (real or mock)
    pub fn new(
        registry: Arc<dyn crate::discovery::RegistryProvider>,
        executor: Arc<dyn ModuleExecutor>,
    ) -> Self {
        Self {
            registry,
            executor,
            module_cache: HashMap::new(),
            #[cfg(test)]
            registry_lookup_count: 0,
        }
    }

    /// Return sorted list of all command names: built-ins + module ids.
    pub fn list_commands(&self) -> Vec<String> {
        let mut names: Vec<String> = BUILTIN_COMMANDS.iter().map(|s| s.to_string()).collect();
        names.extend(self.registry.list());
        // Sort and dedup in one pass.
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Look up a command by name. Returns `None` if the name is not a builtin
    /// and is not found in the registry.
    ///
    /// For module commands, builds and caches a lightweight clap Command.
    pub fn get_command(&mut self, name: &str) -> Option<clap::Command> {
        if BUILTIN_COMMANDS.contains(&name) {
            return Some(clap::Command::new(name.to_string()));
        }
        // Check the in-memory cache first.
        if self.module_cache.contains_key(name) {
            return Some(clap::Command::new(name.to_string()));
        }
        // Registry lookup.
        #[cfg(test)]
        {
            self.registry_lookup_count += 1;
        }
        let _descriptor = self.registry.get_module_descriptor(name)?;
        let cmd = clap::Command::new(name.to_string());
        self.module_cache.insert(name.to_string(), true);
        tracing::debug!("Loaded module command: {name}");
        Some(cmd)
    }

    /// Return the number of times the registry was queried for a descriptor.
    /// Available in test builds only.
    #[cfg(test)]
    pub fn registry_lookup_count(&self) -> usize {
        self.registry_lookup_count
    }
}

// ---------------------------------------------------------------------------
// build_module_command
// ---------------------------------------------------------------------------

/// Built-in flag names added to every generated module command. A schema
/// property that collides with one of these names will cause
/// `std::process::exit(2)`.
const RESERVED_FLAG_NAMES: &[&str] = &["input", "yes", "large-input", "format", "sandbox"];

/// Build a clap `Command` for a single module definition.
///
/// The resulting subcommand has:
/// * its `name` set to `module_def.name`
/// * its `about` derived from the module descriptor (empty if unavailable)
/// * the 5 built-in flags: `--input`, `--yes`/`-y`, `--large-input`,
///   `--format`, `--sandbox`
/// * schema-derived flags from `schema_to_clap_args` (stub: empty vec)
///
/// `executor` is accepted for API symmetry with the Python counterpart but is
/// not embedded in the `clap::Command` (clap has no user-data attachment).
/// The executor is passed separately to the dispatch callback.
///
/// # Errors
/// Returns `CliError::ReservedModuleId` when `module_def.name` is one of the
/// reserved built-in command names.
pub fn build_module_command(
    module_def: &apcore::registry::registry::ModuleDescriptor,
    executor: Arc<dyn ModuleExecutor>,
) -> Result<clap::Command, CliError> {
    let module_id = &module_def.name;

    // Guard: reject reserved command names immediately.
    if BUILTIN_COMMANDS.contains(&module_id.as_str()) {
        return Err(CliError::ReservedModuleId(module_id.clone()));
    }

    // Resolve $ref pointers in the input schema before generating clap args.
    let resolved_schema =
        crate::ref_resolver::resolve_refs(&module_def.input_schema, 32, module_id)
            .unwrap_or_else(|_| module_def.input_schema.clone());

    // Build clap args from JSON Schema properties.
    let schema_args = crate::schema_parser::schema_to_clap_args(&resolved_schema)
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

    // Suppress unused-variable warning; executor is kept for API symmetry.
    let _ = executor;

    let mut cmd = clap::Command::new(module_id.clone())
        // Built-in flags present on every generated command.
        .arg(
            clap::Arg::new("input")
                .long("input")
                .value_name("SOURCE")
                .help("Read input from file or STDIN ('-')."),
        )
        .arg(
            clap::Arg::new("yes")
                .long("yes")
                .short('y')
                .action(clap::ArgAction::SetTrue)
                .help("Bypass approval prompts."),
        )
        .arg(
            clap::Arg::new("large-input")
                .long("large-input")
                .action(clap::ArgAction::SetTrue)
                .help("Allow STDIN input larger than 10MB."),
        )
        .arg(
            clap::Arg::new("format")
                .long("format")
                .value_parser(["json", "table"])
                .help("Output format."),
        )
        .arg(
            clap::Arg::new("sandbox")
                .long("sandbox")
                .action(clap::ArgAction::SetTrue)
                .help("Run module in subprocess sandbox."),
        );

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

/// Merge CLI keyword arguments with optional STDIN JSON.
///
/// Resolution order (highest priority first):
/// 1. CLI flags (non-`Null` values in `cli_kwargs`)
/// 2. STDIN JSON (when `stdin_flag` is `Some("-")`)
///
/// # Arguments
/// * `stdin_flag`  — `Some("-")` to read from STDIN, `None` to skip
/// * `cli_kwargs`  — map of flag name → value (`Null` values are ignored)
/// * `large_input` — if `false`, reject STDIN payloads exceeding 10 MiB
///
/// # Errors
/// Returns `CliError` (exit code 2) on oversized input, invalid JSON, or
/// non-object JSON.
pub fn collect_input(
    stdin_flag: Option<&str>,
    cli_kwargs: HashMap<String, Value>,
    large_input: bool,
) -> Result<HashMap<String, Value>, CliError> {
    collect_input_from_reader(stdin_flag, cli_kwargs, large_input, std::io::stdin())
}

// ---------------------------------------------------------------------------
// validate_module_id
// ---------------------------------------------------------------------------

const MODULE_ID_MAX_LEN: usize = 128;

/// Validate a module identifier.
///
/// # Rules
/// * Maximum 128 characters
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
        EXIT_ACL_DENIED, EXIT_APPROVAL_DENIED, EXIT_CONFIG_NOT_FOUND, EXIT_MODULE_EXECUTE_ERROR,
        EXIT_MODULE_NOT_FOUND, EXIT_SCHEMA_CIRCULAR_REF, EXIT_SCHEMA_VALIDATION_ERROR,
    };
    match error_code {
        "MODULE_NOT_FOUND" | "MODULE_LOAD_ERROR" | "MODULE_DISABLED" => EXIT_MODULE_NOT_FOUND,
        "SCHEMA_VALIDATION_ERROR" => EXIT_SCHEMA_VALIDATION_ERROR,
        "APPROVAL_DENIED" | "APPROVAL_TIMEOUT" | "APPROVAL_PENDING" => EXIT_APPROVAL_DENIED,
        "CONFIG_NOT_FOUND" | "CONFIG_INVALID" => EXIT_CONFIG_NOT_FOUND,
        "SCHEMA_CIRCULAR_REF" => EXIT_SCHEMA_CIRCULAR_REF,
        "ACL_DENIED" => EXIT_ACL_DENIED,
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
    _executor: &Arc<dyn ModuleExecutor + 'static>,
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

    // 4. Build CLI kwargs from schema-derived flags (stub: empty map).
    let cli_kwargs = extract_cli_kwargs(matches, &module_def);

    // 5. Collect and merge input (exit 2 on errors).
    let merged = match collect_input(stdin_flag, cli_kwargs, large_input) {
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

    // 6. Schema validation (if module has input_schema with properties).
    if let Some(schema) = module_def.input_schema.as_object() {
        if schema.contains_key("properties") {
            if let Err(detail) = validate_against_schema(&merged, &module_def.input_schema) {
                eprintln!("Error: Validation failed: {detail}.");
                std::process::exit(EXIT_SCHEMA_VALIDATION_ERROR);
            }
        }
    }

    // 7. Approval gate (exit 46 on denial/timeout).
    let module_json = serde_json::to_value(&module_def).unwrap_or_default();
    if let Err(e) = crate::approval::check_approval(&module_json, auto_approve).await {
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

    // 9. Execute with SIGINT race (exit 130 on Ctrl-C).
    let start = std::time::Instant::now();

    // Unify the execution paths into Result<Value, (i32, String)> where
    // the error tuple is (exit_code, display_message).
    let result: Result<Value, (i32, String)> = if let Some(exec_path) = script_executable {
        // Script-based execution: spawn subprocess, pipe JSON via stdin/stdout.
        tokio::select! {
            res = execute_script(&exec_path, &input_value) => {
                res.map_err(|e| (crate::EXIT_MODULE_EXECUTE_ERROR, e))
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Execution cancelled.");
                std::process::exit(EXIT_SIGINT);
            }
        }
    } else if use_sandbox {
        let sandbox = crate::security::Sandbox::new(true, 0);
        tokio::select! {
            res = sandbox.execute(module_id, input_value.clone()) => {
                res.map_err(|e| (crate::EXIT_MODULE_EXECUTE_ERROR, e.to_string()))
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Execution cancelled.");
                std::process::exit(EXIT_SIGINT);
            }
        }
    } else {
        // Direct in-process executor call (4-argument signature).
        tokio::select! {
            res = apcore_executor.call(module_id, input_value.clone(), None, None) => {
                res.map_err(|e| {
                    let code = map_module_error_to_exit_code(&e);
                    (code, e.to_string())
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
            if let Ok(guard) = AUDIT_LOGGER.lock() {
                if let Some(logger) = guard.as_ref() {
                    logger.log_execution(module_id, &input_value, "success", 0, duration_ms);
                }
            }
            // 11. Format and output.
            let fmt = crate::output::resolve_format(format_flag.as_deref());
            println!("{}", crate::output::format_exec_result(&output, fmt));
            std::process::exit(EXIT_SUCCESS);
        }
        Err((exit_code, msg)) => {
            // Audit log error.
            if let Ok(guard) = AUDIT_LOGGER.lock() {
                if let Some(logger) = guard.as_ref() {
                    logger.log_execution(module_id, &input_value, "error", exit_code, duration_ms);
                }
            }
            eprintln!("Error: Module '{module_id}' execution failed: {msg}.");
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
        let long_id = "a".repeat(129);
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
        let max_id = "a".repeat(128);
        assert!(validate_module_id(&max_id).is_ok());
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

    // ---------------------------------------------------------------------------
    // build_module_command tests (TDD — RED written before GREEN)
    // ---------------------------------------------------------------------------

    /// Construct a minimal `ModuleDescriptor` for use in `build_module_command`
    /// tests. `input_schema` defaults to a JSON null (no properties) when
    /// `schema` is `None`.
    fn make_module_descriptor(
        name: &str,
        _description: &str,
        schema: Option<serde_json::Value>,
    ) -> apcore::registry::registry::ModuleDescriptor {
        apcore::registry::registry::ModuleDescriptor {
            name: name.to_string(),
            annotations: apcore::module::ModuleAnnotations::default(),
            input_schema: schema.unwrap_or(serde_json::Value::Null),
            output_schema: serde_json::Value::Object(Default::default()),
            enabled: true,
            tags: vec![],
            dependencies: vec![],
        }
    }

    #[test]
    fn test_build_module_command_name_is_set() {
        let module = make_module_descriptor("math.add", "Add two numbers", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        assert_eq!(cmd.get_name(), "math.add");
    }

    #[test]
    fn test_build_module_command_has_input_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"input"), "must have --input flag");
    }

    #[test]
    fn test_build_module_command_has_yes_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"yes"), "must have --yes flag");
    }

    #[test]
    fn test_build_module_command_has_large_input_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(
            names.contains(&"large-input"),
            "must have --large-input flag"
        );
    }

    #[test]
    fn test_build_module_command_has_format_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"format"), "must have --format flag");
    }

    #[test]
    fn test_build_module_command_has_sandbox_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        let names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(names.contains(&"sandbox"), "must have --sandbox flag");
    }

    #[test]
    fn test_build_module_command_reserved_name_returns_error() {
        for reserved in BUILTIN_COMMANDS {
            let module = make_module_descriptor(reserved, "desc", None);
            let executor = mock_executor();
            let result = build_module_command(&module, executor);
            assert!(
                matches!(result, Err(CliError::ReservedModuleId(_))),
                "expected ReservedModuleId for '{reserved}', got {result:?}"
            );
        }
    }

    #[test]
    fn test_build_module_command_yes_has_short_flag() {
        let module = make_module_descriptor("a.b", "desc", None);
        let executor = mock_executor();
        let cmd = build_module_command(&module, executor).unwrap();
        let has_short_y = cmd
            .get_opts()
            .filter(|a| a.get_long() == Some("yes"))
            .any(|a| a.get_short() == Some('y'));
        assert!(has_short_y, "--yes must have short flag -y");
    }

    // ---------------------------------------------------------------------------
    // LazyModuleGroup tests (TDD)
    // ---------------------------------------------------------------------------

    /// Mock registry that returns a fixed list of module names.
    struct CliMockRegistry {
        modules: Vec<String>,
    }

    impl crate::discovery::RegistryProvider for CliMockRegistry {
        fn list(&self) -> Vec<String> {
            self.modules.clone()
        }

        fn get_definition(&self, name: &str) -> Option<Value> {
            if self.modules.iter().any(|m| m == name) {
                Some(serde_json::json!({
                    "module_id": name,
                    "name": name,
                    "input_schema": {},
                    "output_schema": {},
                    "enabled": true,
                    "tags": [],
                    "dependencies": [],
                }))
            } else {
                None
            }
        }

        fn get_module_descriptor(
            &self,
            name: &str,
        ) -> Option<apcore::registry::registry::ModuleDescriptor> {
            if self.modules.iter().any(|m| m == name) {
                Some(apcore::registry::registry::ModuleDescriptor {
                    name: name.to_string(),
                    annotations: apcore::module::ModuleAnnotations::default(),
                    input_schema: serde_json::Value::Object(Default::default()),
                    output_schema: serde_json::Value::Object(Default::default()),
                    enabled: true,
                    tags: vec![],
                    dependencies: vec![],
                })
            } else {
                None
            }
        }
    }

    /// Mock registry that returns an empty list (simulates unavailable registry).
    struct EmptyRegistry;

    impl crate::discovery::RegistryProvider for EmptyRegistry {
        fn list(&self) -> Vec<String> {
            vec![]
        }

        fn get_definition(&self, _name: &str) -> Option<Value> {
            None
        }
    }

    /// Mock executor (no-op).
    struct MockExecutor;

    impl ModuleExecutor for MockExecutor {}

    fn mock_registry(modules: Vec<&str>) -> Arc<dyn crate::discovery::RegistryProvider> {
        Arc::new(CliMockRegistry {
            modules: modules.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn mock_executor() -> Arc<dyn ModuleExecutor> {
        Arc::new(MockExecutor)
    }

    #[test]
    fn test_lazy_module_group_list_commands_empty_registry() {
        let group = LazyModuleGroup::new(mock_registry(vec![]), mock_executor());
        let cmds = group.list_commands();
        for builtin in ["exec", "list", "describe", "completion", "man"] {
            assert!(
                cmds.contains(&builtin.to_string()),
                "missing builtin: {builtin}"
            );
        }
        // Result must be sorted.
        let mut sorted = cmds.clone();
        sorted.sort();
        assert_eq!(cmds, sorted, "list_commands must return a sorted list");
    }

    #[test]
    fn test_lazy_module_group_list_commands_includes_modules() {
        let group = LazyModuleGroup::new(
            mock_registry(vec!["math.add", "text.summarize"]),
            mock_executor(),
        );
        let cmds = group.list_commands();
        assert!(cmds.contains(&"math.add".to_string()));
        assert!(cmds.contains(&"text.summarize".to_string()));
    }

    #[test]
    fn test_lazy_module_group_list_commands_registry_error() {
        let group = LazyModuleGroup::new(Arc::new(EmptyRegistry), mock_executor());
        let cmds = group.list_commands();
        // Must not be empty; must contain builtins.
        assert!(!cmds.is_empty());
        assert!(cmds.contains(&"list".to_string()));
    }

    #[test]
    fn test_lazy_module_group_get_command_builtin() {
        let mut group = LazyModuleGroup::new(mock_registry(vec![]), mock_executor());
        let cmd = group.get_command("list");
        assert!(cmd.is_some(), "get_command('list') must return Some");
    }

    #[test]
    fn test_lazy_module_group_get_command_not_found() {
        let mut group = LazyModuleGroup::new(mock_registry(vec![]), mock_executor());
        let cmd = group.get_command("nonexistent.module");
        assert!(cmd.is_none());
    }

    #[test]
    fn test_lazy_module_group_get_command_caches_module() {
        let mut group = LazyModuleGroup::new(mock_registry(vec!["math.add"]), mock_executor());
        // First call builds and caches.
        let cmd1 = group.get_command("math.add");
        assert!(cmd1.is_some());
        // Second call returns from cache — registry lookup should not be called again.
        let cmd2 = group.get_command("math.add");
        assert!(cmd2.is_some());
        assert_eq!(
            group.registry_lookup_count(),
            1,
            "cached after first lookup"
        );
    }

    #[test]
    fn test_lazy_module_group_builtin_commands_sorted() {
        // BUILTIN_COMMANDS slice must itself be in sorted order (single source of truth).
        let mut sorted = BUILTIN_COMMANDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            BUILTIN_COMMANDS,
            sorted.as_slice(),
            "BUILTIN_COMMANDS must be sorted"
        );
    }

    #[test]
    fn test_lazy_module_group_list_deduplicates_builtins() {
        // If a registry module name collides with a builtin, the result must not
        // contain duplicates.
        let group = LazyModuleGroup::new(mock_registry(vec!["list", "exec"]), mock_executor());
        let cmds = group.list_commands();
        let list_count = cmds.iter().filter(|c| c.as_str() == "list").count();
        assert_eq!(list_count, 1, "duplicate 'list' entry in list_commands");
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
}
