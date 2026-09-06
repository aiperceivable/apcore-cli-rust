// apcore-cli — Discovery subcommands (list + describe).
// Protocol spec: FE-04

use clap::{Arg, ArgAction, Command};
use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// DiscoveryError
// ---------------------------------------------------------------------------

/// Errors produced by discovery command handlers.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("module '{0}' not found")]
    ModuleNotFound(String),

    #[error("invalid module id: {0}")]
    InvalidModuleId(String),

    #[error("invalid tag format: '{0}'. Tags must match [a-z][a-z0-9_-]*.")]
    InvalidTag(String),
}

// ---------------------------------------------------------------------------
// RegistryProvider trait
// ---------------------------------------------------------------------------

/// Unified registry interface used by both discovery commands and the CLI
/// dispatcher. Provides JSON-based access (`get_definition`) for discovery
/// and typed access (`get_module_descriptor`) for the dispatch pipeline.
///
/// The real `apcore::Registry` implements this trait via `ApCoreRegistryProvider`.
/// Tests use `MockRegistry`.
pub trait RegistryProvider: Send + Sync {
    /// Return all module IDs in the registry.
    fn list(&self) -> Vec<String>;

    /// Return the JSON descriptor for a single module, or `None` if not found.
    fn get_definition(&self, id: &str) -> Option<Value>;

    /// Return the typed descriptor for a single module, or `None` if not found.
    ///
    /// The default implementation deserializes from `get_definition`. Adapters
    /// wrapping a real `apcore::Registry` should override this for efficiency.
    fn get_module_descriptor(
        &self,
        id: &str,
    ) -> Option<apcore::registry::registry::ModuleDescriptor> {
        self.get_definition(id)
            .and_then(|v| serde_json::from_value(v).ok())
    }
}

// ---------------------------------------------------------------------------
// validate_tag
// ---------------------------------------------------------------------------

/// Validate a tag string against the pattern `^[a-z][a-z0-9_-]*$`.
///
/// Returns `true` if valid, `false` otherwise. Does not exit the process.
pub fn validate_tag(tag: &str) -> bool {
    let mut chars = tag.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// module_has_all_tags helper
// ---------------------------------------------------------------------------

fn module_has_all_tags(module: &Value, tags: &[&str]) -> bool {
    let mod_tags: Vec<&str> = module
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    tags.iter().all(|required| mod_tags.contains(required))
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

/// Options for the enhanced list command (FE-11 F7).
#[derive(Default)]
pub struct ListOptions<'a> {
    pub tags: &'a [&'a str],
    pub explicit_format: Option<&'a str>,
    pub search: Option<&'a str>,
    pub status: Option<&'a str>,
    pub annotations: &'a [&'a str],
    pub sort: Option<&'a str>,
    pub reverse: bool,
    pub deprecated: bool,
    /// `--deps`: show a dependency-count column (table) / `dependency_count`
    /// field (json). Read from `sub_m.get_flag("deps")` in `handle_list`.
    pub deps: bool,
    /// `--exposure {exposed,hidden,all}` (FE-12). `None` behaves like
    /// `"exposed"`, matching the flag's own clap default.
    pub exposure: Option<&'a str>,
    /// The `ExposureFilter` to filter/annotate by. `None` disables exposure
    /// filtering and the Exposure column entirely (e.g. embedded callers that
    /// never configured FE-12). `handle_list` always supplies
    /// `Some(&ExposureFilter::default())` when no project-specific filter is
    /// wired in, mirroring Python's `register_list_command` default.
    pub exposure_filter: Option<&'a crate::exposure::ExposureFilter>,
}

/// Execute the `list` subcommand logic.
///
/// Returns `Ok(String)` with the formatted output on success.
/// Returns `Err(DiscoveryError)` on invalid tag format.
///
/// Exit code mapping for the caller: `DiscoveryError::InvalidTag` -> exit 2.
///
/// **Design note (audit D9-W3):** This is a thin convenience wrapper over
/// [`cmd_list_enhanced`] for in-tree tests of the "tags + format only" path.
/// It has no production callers — every dispatch path goes through
/// `cmd_list_enhanced` with full `ListOptions`. Demoted to `pub(crate)` in
/// v0.8 so external consumers cannot bind to a symbol that exists purely for
/// internal test convenience. Gated behind `#[cfg(test)]` so non-test builds
/// don't carry the dead wrapper.
#[cfg(test)]
pub(crate) fn cmd_list(
    registry: &dyn RegistryProvider,
    tags: &[&str],
    explicit_format: Option<&str>,
) -> Result<String, DiscoveryError> {
    cmd_list_enhanced(
        registry,
        &ListOptions {
            tags,
            explicit_format,
            ..Default::default()
        },
    )
}

/// Enhanced list with full filter/sort options (FE-11 F7).
pub fn cmd_list_enhanced(
    registry: &dyn RegistryProvider,
    opts: &ListOptions<'_>,
) -> Result<String, DiscoveryError> {
    // Validate all tag formats before filtering.
    for tag in opts.tags {
        if !validate_tag(tag) {
            return Err(DiscoveryError::InvalidTag(tag.to_string()));
        }
    }

    // Collect all module definitions.
    let mut modules: Vec<Value> = registry
        .list()
        .into_iter()
        .filter_map(|id| registry.get_definition(&id))
        .collect();

    // Apply AND tag filter if any tags were specified.
    if !opts.tags.is_empty() {
        modules.retain(|m| module_has_all_tags(m, opts.tags));
    }

    // F7: Search filter (case-insensitive substring on id + description).
    if let Some(query) = opts.search {
        let q = query.to_lowercase();
        modules.retain(|m| {
            let id = m
                .get("module_id")
                .or_else(|| m.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let desc = m.get("description").and_then(|v| v.as_str()).unwrap_or("");
            id.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q)
        });
    }

    // F7: Status filter.
    match opts.status.unwrap_or("enabled") {
        "enabled" => {
            modules.retain(|m| m.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true));
        }
        "disabled" => {
            modules.retain(|m| m.get("enabled").and_then(|v| v.as_bool()) == Some(false));
        }
        _ => {} // "all": no filter
    }

    // F7: Deprecated filter (excluded by default).
    if !opts.deprecated {
        modules.retain(|m| m.get("deprecated").and_then(|v| v.as_bool()) != Some(true));
    }

    // F7: Annotation filter (AND logic).
    if !opts.annotations.is_empty() {
        for ann_flag in opts.annotations {
            let attr = match *ann_flag {
                "requires-approval" => "requires_approval",
                other => other,
            };
            modules.retain(|m| {
                m.get("annotations")
                    .and_then(|a| a.get(attr))
                    .and_then(|v| v.as_bool())
                    == Some(true)
            });
        }
    }

    // FE-12: --exposure {exposed,hidden,all} filter. `exposure_filter` being
    // `None` means the caller never wired FE-12 in at all (e.g. some
    // embedding contexts) and exposure filtering is skipped entirely, same
    // as before this flag existed. `handle_list` always supplies a filter
    // (defaulting to `ExposureFilter::default()`, mode "all") so this is the
    // normal, exercised path for the standalone binary.
    fn module_id_of(m: &Value) -> &str {
        m.get("module_id")
            .or_else(|| m.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
    }
    if let Some(filter) = opts.exposure_filter {
        match opts.exposure.unwrap_or("exposed") {
            "exposed" => modules.retain(|m| filter.is_exposed(module_id_of(m))),
            "hidden" => modules.retain(|m| !filter.is_exposed(module_id_of(m))),
            _ => {} // "all": no filter, but see the Exposure column below.
        }
    }

    // F7: Sort. The clap value_parser accepts ["id", "calls", "errors",
    // "latency"] for cross-SDK parity (D11-006). Usage-based sorts read
    // aggregates from the local audit log via `system_usage` (issue #17);
    // when the log has no entries in the 24h window the helper falls back
    // to id-sort and we emit a visible note (not just tracing::warn) to
    // stderr so users see the fallback.
    let requested_sort = opts.sort.unwrap_or("id");
    if matches!(requested_sort, "calls" | "errors" | "latency") {
        let used =
            crate::system_usage::sort_modules_by_usage(&mut modules, requested_sort, !opts.reverse);
        if !used {
            // D11 bug: `sort_modules_by_usage`'s own empty-log fallback sorts
            // by id using the SAME pre-inverted `!opts.reverse` it was
            // handed (its normal direction for calls/errors/latency is
            // descending, hence the inversion) -- so with no usage data at
            // all it silently reused that inversion for its id fallback too,
            // producing Z->A order even when the user passed no --reverse.
            // Re-sort here with the user's own UN-inverted flag, discarding
            // whichever direction the helper's internal fallback chose.
            // Matches Python's discovery.py at the equivalent point.
            modules.sort_by(|a, b| {
                let aid = a.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
                let bid = b.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
                aid.cmp(bid)
            });
            if opts.reverse {
                modules.reverse();
            }
            eprintln!(
                "note: no usage data available for --sort {}; sorted by id. \
Run some modules first to populate ~/.apcore-cli/audit.jsonl.",
                requested_sort
            );
        }
    } else {
        modules.sort_by(|a, b| {
            let aid = a.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
            let bid = b.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
            aid.cmp(bid)
        });
        if opts.reverse {
            modules.reverse();
        }
    }

    let fmt = crate::output::resolve_format(opts.explicit_format);
    // The Exposure column is shown only for `--exposure all` (matching
    // Python's `show_exposure_col = exposure == "all"`) — for "exposed" /
    // "hidden" every remaining row would show the same value, so the column
    // would carry no information.
    let show_exposure_col = opts.exposure.unwrap_or("exposed") == "all";
    let display_opts = crate::output::ListDisplayOptions {
        show_deps: opts.deps,
        exposure_filter: opts.exposure_filter.filter(|_| show_exposure_col),
    };
    Ok(crate::output::format_module_list_with_options(
        &modules,
        fmt,
        opts.tags,
        &display_opts,
    ))
}

// ---------------------------------------------------------------------------
// cmd_describe
// ---------------------------------------------------------------------------

/// Execute the `describe` subcommand logic.
///
/// Returns `Ok(String)` with the formatted output on success.
/// Returns `Err(DiscoveryError)` on invalid module ID or module not found.
///
/// Exit code mapping for the caller:
/// - `DiscoveryError::InvalidModuleId` → exit 2
/// - `DiscoveryError::ModuleNotFound`  → exit 44
pub fn cmd_describe(
    registry: &dyn RegistryProvider,
    module_id: &str,
    explicit_format: Option<&str>,
) -> Result<String, DiscoveryError> {
    // Validate module ID format.
    if crate::cli::validate_module_id(module_id).is_err() {
        return Err(DiscoveryError::InvalidModuleId(module_id.to_string()));
    }

    let module = registry
        .get_definition(module_id)
        .ok_or_else(|| DiscoveryError::ModuleNotFound(module_id.to_string()))?;

    let fmt = crate::output::resolve_format(explicit_format);
    Ok(crate::output::format_module_detail(&module, fmt))
}

// ---------------------------------------------------------------------------
// Per-subcommand registrars (FE-13)
// ---------------------------------------------------------------------------
//
// The TS reference (`apcore-cli-typescript/src/discovery.ts`) splits
// registration into `registerListCommand`, `registerDescribeCommand`,
// `registerExecCommand`, `registerValidateCommand` so that the FE-13 built-in
// command-group dispatcher can honor include/exclude filtering on a
// per-subcommand basis.
//
// In Rust, the executor/registry are not baked into `clap::Command` — dispatch
// flows through a separate path (`dispatch_module`), so these registrars take
// only a `Command` and return a `Command`. They are intentionally pure
// "add-subcommand-and-return" helpers with no internal state mutation, which
// makes them safe to invoke on either the root command or the `apcli`
// sub-group (or reused from standalone deprecation shims).

/// Attach the `list` subcommand to the given command (typically the `apcli`
/// group). Returns the command with the subcommand added.
pub fn register_list_command(cli: Command) -> Command {
    cli.subcommand(list_command())
}

/// Attach the `describe` subcommand to the given command. Returns the command
/// with the subcommand added.
pub fn register_describe_command(cli: Command) -> Command {
    cli.subcommand(describe_command())
}

/// Attach the `exec` subcommand to the given command. Delegates to
/// [`crate::cli::exec_command`] to avoid duplicating the builder.
pub fn register_exec_command(cli: Command) -> Command {
    cli.subcommand(crate::cli::exec_command())
}

// ---------------------------------------------------------------------------
// list_command / describe_command builders
// ---------------------------------------------------------------------------

fn list_command() -> Command {
    Command::new("list")
        .about("List available modules in the registry")
        .arg(
            Arg::new("tag")
                .long("tag")
                .action(ArgAction::Append)
                .value_name("TAG")
                .help("Filter modules by tag (AND logic). Repeatable."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(clap::builder::PossibleValuesParser::new([
                    "table", "json", "csv", "yaml", "jsonl", "markdown", "skill",
                ]))
                .value_name("FORMAT")
                .help("Output format. Default: table (TTY) or json (non-TTY)."),
        )
        .arg(
            Arg::new("search")
                .long("search")
                .short('s')
                .value_name("QUERY")
                .help("Filter by substring match on ID and description."),
        )
        .arg(
            Arg::new("status")
                .long("status")
                .value_parser(["enabled", "disabled", "all"])
                .default_value("enabled")
                .value_name("STATUS")
                .help("Filter by module status. Default: enabled."),
        )
        .arg(
            Arg::new("annotation")
                .long("annotation")
                .short('a')
                .action(ArgAction::Append)
                // Cross-SDK parity (D11-006): Python and TS accept the full
                // apcore >= 0.19.0 ModuleAnnotations set including
                // "paginated". Rust now matches.
                .value_parser([
                    "destructive",
                    "requires-approval",
                    "readonly",
                    "streaming",
                    "cacheable",
                    "idempotent",
                    "paginated",
                ])
                .value_name("ANN")
                .help("Filter by annotation flag (AND logic). Repeatable."),
        )
        .arg(
            Arg::new("sort")
                .long("sort")
                // Cross-SDK parity (D11-006): Python and TS accept
                // [id, calls, errors, latency] and emit a runtime warning
                // when usage data is unavailable for the non-id sorts. Rust
                // now matches; cmd_list_enhanced emits the warning and falls
                // back to id when usage modules are not registered.
                .value_parser(["id", "calls", "errors", "latency"])
                .default_value("id")
                .value_name("FIELD")
                .help("Sort order. Default: id. Non-id values require usage data; warns and falls back when unavailable."),
        )
        .arg(
            Arg::new("reverse")
                .long("reverse")
                .action(ArgAction::SetTrue)
                .help("Reverse sort order."),
        )
        .arg(
            Arg::new("deprecated")
                .long("deprecated")
                .action(ArgAction::SetTrue)
                .help("Include deprecated modules."),
        )
        .arg(
            Arg::new("deps")
                .long("deps")
                .action(ArgAction::SetTrue)
                .help("Show dependency count column."),
        )
        .arg(
            Arg::new("exposure")
                .long("exposure")
                .value_parser(clap::builder::PossibleValuesParser::new([
                    "exposed", "hidden", "all",
                ]))
                .default_value("exposed")
                .value_name("STATUS")
                .help("Filter by exposure status. Default: exposed."),
        )
    // Note: no `--flat` here. Python/TS default to a GROUPED list rendering
    // and use `--flat` to opt out of it; Rust's list has no grouping concept
    // at all -- it is unconditionally flat. A `--flat` flag would therefore
    // either (a) be permanently dead (declared, does nothing, exactly the
    // status-quo bug this fix removes) or (b) require implementing full
    // grouped-list rendering, which is a rendering feature out of proportion
    // to this fix pass, not a bug fix. Removed rather than kept as a
    // documented no-op so `--help` stops advertising behavior Rust's list
    // does not have a "flat vs. grouped" distinction to opt out of.
}

fn describe_command() -> Command {
    Command::new("describe")
        .about("Show metadata, schema, and annotations for a module")
        .arg(
            Arg::new("module_id")
                .required(true)
                .value_name("MODULE_ID")
                .help("Canonical module identifier (e.g. math.add)"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(clap::builder::PossibleValuesParser::new([
                    "table", "json", "csv", "yaml", "jsonl", "markdown", "skill",
                ]))
                .value_name("FORMAT")
                .help("Output format. Default: table (TTY) or json (non-TTY)."),
        )
}

// ---------------------------------------------------------------------------
// ApCoreRegistryProvider — wraps apcore::Registry for discovery commands
// ---------------------------------------------------------------------------

/// Adapter that implements `RegistryProvider` for the real `apcore::Registry`.
///
/// Tracks discovered module names separately because `Registry::discover()`
/// stores descriptors but not module implementations, so `Registry::list()`
/// (which iterates over the modules map) would miss them.
pub struct ApCoreRegistryProvider {
    registry: std::sync::Arc<apcore::Registry>,
    discovered_names: Vec<String>,
    descriptions: std::collections::HashMap<String, String>,
}

impl ApCoreRegistryProvider {
    /// Create a new adapter from an owned `apcore::Registry`.
    ///
    /// The registry is wrapped in an `Arc` internally so the same underlying
    /// store can be shared without copying.
    pub fn new(registry: apcore::Registry) -> Self {
        Self {
            registry: std::sync::Arc::new(registry),
            discovered_names: Vec::new(),
            descriptions: std::collections::HashMap::new(),
        }
    }

    /// Record names of modules found via discovery so they appear in `list()`.
    pub fn set_discovered_names(&mut self, names: Vec<String>) {
        self.discovered_names = names;
    }

    /// Store module descriptions loaded from module.json files.
    pub fn set_descriptions(&mut self, descriptions: std::collections::HashMap<String, String>) {
        self.descriptions = descriptions;
    }
}

impl RegistryProvider for ApCoreRegistryProvider {
    fn list(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .registry
            .list(None, None, None)
            .iter()
            .map(|s| s.to_string())
            .collect();
        for name in &self.discovered_names {
            if !ids.contains(name) {
                ids.push(name.clone());
            }
        }
        ids
    }

    fn get_definition(&self, id: &str) -> Option<Value> {
        self.registry
            .get_definition(id)
            .ok()
            .flatten()
            .and_then(|d| serde_json::to_value(d).ok())
            .map(|mut v: Value| {
                // Inject description from discovery metadata if available,
                // since ModuleDescriptor does not carry a description field.
                if let Some(desc) = self.descriptions.get(id) {
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("description".to_string(), Value::String(desc.clone()));
                    }
                }
                v
            })
    }

    fn get_module_descriptor(
        &self,
        id: &str,
    ) -> Option<apcore::registry::registry::ModuleDescriptor> {
        self.registry.get_definition(id).ok().flatten()
    }
}

// ---------------------------------------------------------------------------
// MockRegistry — gated behind cfg(test) or the test-support feature
// ---------------------------------------------------------------------------

/// Test helper: in-memory registry backed by a Vec of JSON module descriptors.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub struct MockRegistry {
    modules: Vec<Value>,
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
impl MockRegistry {
    pub fn new(modules: Vec<Value>) -> Self {
        Self { modules }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl RegistryProvider for MockRegistry {
    fn list(&self) -> Vec<String> {
        self.modules
            .iter()
            .filter_map(|m| {
                m.get("module_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect()
    }

    fn get_definition(&self, id: &str) -> Option<Value> {
        self.modules
            .iter()
            .find(|m| m.get("module_id").and_then(|v| v.as_str()) == Some(id))
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// mock_module helper — gated behind cfg(test) or the test-support feature
// ---------------------------------------------------------------------------

/// Test helper: build a minimal module descriptor JSON value.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn mock_module(id: &str, description: &str, tags: &[&str]) -> Value {
    serde_json::json!({
        "module_id": id,
        "description": description,
        "tags": tags,
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- validate_tag ---

    #[test]
    fn test_validate_tag_valid_simple() {
        assert!(validate_tag("math"), "single lowercase word must be valid");
    }

    #[test]
    fn test_validate_tag_valid_with_digits_and_dash() {
        assert!(validate_tag("ml-v2"), "digits and dash must be valid");
    }

    #[test]
    fn test_validate_tag_valid_with_underscore() {
        assert!(validate_tag("core_util"), "underscore must be valid");
    }

    #[test]
    fn test_validate_tag_invalid_uppercase() {
        assert!(!validate_tag("Math"), "uppercase start must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_starts_with_digit() {
        assert!(!validate_tag("1tag"), "digit start must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_special_chars() {
        assert!(!validate_tag("invalid!"), "special chars must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_empty() {
        assert!(!validate_tag(""), "empty string must be invalid");
    }

    #[test]
    fn test_validate_tag_invalid_space() {
        assert!(!validate_tag("has space"), "space must be invalid");
    }

    // --- RegistryProvider / MockRegistry ---

    #[test]
    fn test_mock_registry_list_returns_ids() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("text.upper", "Uppercase text", &["text"]),
        ]);
        let ids = registry.list();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"math.add".to_string()));
    }

    #[test]
    fn test_mock_registry_get_definition_found() {
        let registry = MockRegistry::new(vec![mock_module("math.add", "Add numbers", &["math"])]);
        let def = registry.get_definition("math.add");
        assert!(def.is_some());
        assert_eq!(def.unwrap()["module_id"], "math.add");
    }

    #[test]
    fn test_mock_registry_get_definition_not_found() {
        let registry = MockRegistry::new(vec![]);
        assert!(registry.get_definition("non.existent").is_none());
    }

    // --- FE-11 --deps / FE-12 --exposure / D11 sort-fallback regressions ---

    #[test]
    fn test_deps_flag_shows_dependency_count_in_json() {
        // issue #6: `--deps` was declared and shown in --help but nothing
        // downstream ever read it. `dependencies` is a top-level field on
        // the module descriptor (ModuleDescriptor.dependencies), matching
        // what Python's output.py reads via `getattr(m, "dependencies",
        // None)` -- not a nested `metadata.dependencies`.
        let registry = MockRegistry::new(vec![serde_json::json!({
            "module_id": "a.b",
            "description": "Desc",
            "tags": [],
            "dependencies": ["dep.one", "dep.two", "dep.three"]
        })]);
        let opts = ListOptions {
            explicit_format: Some("json"),
            deps: true,
            ..Default::default()
        };
        let output = cmd_list_enhanced(&registry, &opts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed[0]["dependency_count"], 3,
            "json output must carry dependency_count when --deps is passed, got: {parsed}"
        );
    }

    #[test]
    fn test_deps_flag_absent_by_default() {
        let registry = MockRegistry::new(vec![serde_json::json!({
            "module_id": "a.b",
            "description": "Desc",
            "tags": [],
            "dependencies": ["dep.one"]
        })]);
        let opts = ListOptions {
            explicit_format: Some("json"),
            ..Default::default()
        };
        let output = cmd_list_enhanced(&registry, &opts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(
            parsed[0].get("dependency_count").is_none(),
            "dependency_count must be absent when --deps was not passed, got: {parsed}"
        );
    }

    #[test]
    fn test_list_command_has_no_flat_flag() {
        // issue #7: `--flat` was declared and fully dead, and there is no
        // grouping concept in Rust's list output to flatten (unlike Python/
        // TS, which default to grouped rendering). Full grouped-list
        // rendering is out of proportion to this fix pass, so the flag is
        // removed rather than half-implemented or left as a documented
        // no-op -- `--help` must stop advertising behavior that does not
        // exist.
        let cmd = list_command();
        let arg_names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(
            !arg_names.contains(&"flat"),
            "--flat must be removed from the list command, got {arg_names:?}"
        );
    }

    #[test]
    fn test_exposure_hidden_filters_using_real_exposure_filter() {
        // issue #8: `--exposure` was missing entirely; the tested
        // `exposure::ExposureFilter` was never imported by discovery.rs.
        let registry = MockRegistry::new(vec![
            mock_module("admin.users", "Admin", &[]),
            mock_module("public.ping", "Ping", &[]),
        ]);
        let filter = crate::exposure::ExposureFilter::new("exclude", &[], &["admin.*".to_string()]);
        let opts = ListOptions {
            explicit_format: Some("json"),
            exposure: Some("hidden"),
            exposure_filter: Some(&filter),
            ..Default::default()
        };
        let output = cmd_list_enhanced(&registry, &opts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let ids: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["admin.users"],
            "--exposure hidden must show only modules the filter excludes from exposure, got {ids:?}"
        );
    }

    #[test]
    fn test_exposure_exposed_is_the_default() {
        let registry = MockRegistry::new(vec![
            mock_module("admin.users", "Admin", &[]),
            mock_module("public.ping", "Ping", &[]),
        ]);
        let filter = crate::exposure::ExposureFilter::new("exclude", &[], &["admin.*".to_string()]);
        let opts = ListOptions {
            explicit_format: Some("json"),
            // No `exposure` set: must default to "exposed".
            exposure_filter: Some(&filter),
            ..Default::default()
        };
        let output = cmd_list_enhanced(&registry, &opts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let ids: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["public.ping"]);
    }

    #[test]
    fn test_exposure_all_shows_exposure_column_and_no_filtering() {
        let registry = MockRegistry::new(vec![
            mock_module("admin.users", "Admin", &[]),
            mock_module("public.ping", "Ping", &[]),
        ]);
        let filter = crate::exposure::ExposureFilter::new("exclude", &[], &["admin.*".to_string()]);
        let opts = ListOptions {
            explicit_format: Some("json"),
            exposure: Some("all"),
            exposure_filter: Some(&filter),
            ..Default::default()
        };
        let output = cmd_list_enhanced(&registry, &opts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2, "--exposure all must not filter anything out");
        let admin = arr.iter().find(|m| m["id"] == "admin.users").unwrap();
        let public = arr.iter().find(|m| m["id"] == "public.ping").unwrap();
        assert_eq!(admin["exposed"], false);
        assert_eq!(public["exposed"], true);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn test_sort_calls_with_empty_usage_log_sorts_ascending_by_default() {
        // issue #9: `sort_modules_by_usage`'s own empty-log fallback sorts by
        // id using the SAME pre-inverted `!opts.reverse` flag it was handed
        // (its normal direction for calls/errors/latency is descending,
        // hence the inversion) -- so an EMPTY usage log silently reused that
        // inversion for its id-sort fallback too, producing Z->A order even
        // when the user passed no --reverse at all.
        //
        // Delete the shared test-support audit-log redirect first so this
        // test's "no usage data" premise holds regardless of what earlier
        // tests in this binary logged; gated on `test-support` so this can
        // never touch a real ~/.apcore-cli/audit.jsonl on a plain `cargo
        // test` run without --all-features.
        static AUDIT_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = AUDIT_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(path) = crate::security::AuditLogger::test_redirect_path() {
            let _ = std::fs::remove_file(&path);
        }

        let registry = MockRegistry::new(vec![
            mock_module("z.last", "Z", &[]),
            mock_module("a.first", "A", &[]),
            mock_module("m.middle", "M", &[]),
        ]);
        let opts = ListOptions {
            explicit_format: Some("json"),
            sort: Some("calls"),
            reverse: false,
            ..Default::default()
        };
        let output = cmd_list_enhanced(&registry, &opts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let ids: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["a.first", "m.middle", "z.last"],
            "empty usage log + no --reverse must sort ascending by module_id, got {ids:?}"
        );
    }

    // --- cmd_list ---

    #[test]
    fn test_cmd_list_all_modules_no_filter() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("text.upper", "Uppercase text", &["text"]),
        ]);
        let output = cmd_list(&registry, &[], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn test_cmd_list_empty_registry_table() {
        let registry = MockRegistry::new(vec![]);
        let output = cmd_list(&registry, &[], Some("table")).unwrap();
        assert_eq!(output.trim(), "No modules found.");
    }

    #[test]
    fn test_cmd_list_empty_registry_json() {
        let registry = MockRegistry::new(vec![]);
        let output = cmd_list(&registry, &[], Some("json")).unwrap();
        assert_eq!(output.trim(), "[]");
    }

    #[test]
    fn test_cmd_list_tag_filter_single_match() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("text.upper", "Uppercase text", &["text"]),
        ]);
        let output = cmd_list(&registry, &["math"], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "math.add");
    }

    #[test]
    fn test_cmd_list_tag_filter_and_semantics() {
        let registry = MockRegistry::new(vec![
            mock_module("math.add", "Add numbers", &["math", "core"]),
            mock_module("math.mul", "Multiply", &["math"]),
        ]);
        // Only math.add has BOTH "math" AND "core".
        let output = cmd_list(&registry, &["math", "core"], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "math.add");
    }

    #[test]
    fn test_cmd_list_tag_filter_no_match_table() {
        let registry = MockRegistry::new(vec![mock_module("math.add", "Add numbers", &["math"])]);
        let output = cmd_list(&registry, &["nonexistent"], Some("table")).unwrap();
        assert!(output.contains("No modules found matching tags:"));
        assert!(output.contains("nonexistent"));
    }

    #[test]
    fn test_cmd_list_tag_filter_no_match_json() {
        let registry = MockRegistry::new(vec![mock_module("math.add", "Add numbers", &["math"])]);
        let output = cmd_list(&registry, &["nonexistent"], Some("json")).unwrap();
        assert_eq!(output.trim(), "[]");
    }

    #[test]
    fn test_cmd_list_invalid_tag_format_returns_error() {
        let registry = MockRegistry::new(vec![]);
        let result = cmd_list(&registry, &["INVALID!"], Some("json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            DiscoveryError::InvalidTag(tag) => assert_eq!(tag, "INVALID!"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_cmd_list_description_truncated_in_table() {
        let long_desc = "x".repeat(100);
        let registry = MockRegistry::new(vec![mock_module("a.b", &long_desc, &[])]);
        let output = cmd_list(&registry, &[], Some("table")).unwrap();
        assert!(output.contains("..."), "long description must be truncated");
        assert!(
            !output.contains(&"x".repeat(100)),
            "full description must not appear"
        );
    }

    #[test]
    fn test_cmd_list_json_contains_id_description_tags() {
        let registry = MockRegistry::new(vec![mock_module("a.b", "Desc", &["x", "y"])]);
        let output = cmd_list(&registry, &[], Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let entry = &parsed[0];
        assert!(entry.get("id").is_some());
        assert!(entry.get("description").is_some());
        assert!(entry.get("tags").is_some());
    }

    // --- cmd_describe ---

    #[test]
    fn test_cmd_describe_valid_module_json() {
        let registry = MockRegistry::new(vec![mock_module(
            "math.add",
            "Add two numbers",
            &["math", "core"],
        )]);
        let output = cmd_describe(&registry, "math.add", Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["id"], "math.add");
        assert_eq!(parsed["description"], "Add two numbers");
    }

    #[test]
    fn test_cmd_describe_valid_module_table() {
        let registry =
            MockRegistry::new(vec![mock_module("math.add", "Add two numbers", &["math"])]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(output.contains("math.add"), "table must contain module id");
        assert!(
            output.contains("Add two numbers"),
            "table must contain description"
        );
    }

    #[test]
    fn test_cmd_describe_not_found_returns_error() {
        let registry = MockRegistry::new(vec![]);
        let result = cmd_describe(&registry, "non.existent", Some("json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            DiscoveryError::ModuleNotFound(id) => assert_eq!(id, "non.existent"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_cmd_describe_invalid_id_returns_error() {
        let registry = MockRegistry::new(vec![]);
        let result = cmd_describe(&registry, "INVALID!ID", Some("json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            DiscoveryError::InvalidModuleId(_) => {}
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_cmd_describe_no_output_schema_table_omits_section() {
        // Module without output_schema: section must be absent from table output.
        let registry = MockRegistry::new(vec![serde_json::json!({
            "module_id": "math.add",
            "description": "Add numbers",
            "input_schema": {"type": "object"},
            "tags": ["math"]
            // note: no output_schema key
        })]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(
            !output.contains("Output Schema:"),
            "output_schema section must be absent"
        );
    }

    #[test]
    fn test_cmd_describe_no_annotations_table_omits_section() {
        let registry = MockRegistry::new(vec![mock_module("math.add", "Add numbers", &["math"])]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(
            !output.contains("Annotations:"),
            "annotations section must be absent"
        );
    }

    #[test]
    fn test_cmd_describe_with_annotations_table_shows_section() {
        let registry = MockRegistry::new(vec![serde_json::json!({
            "module_id": "math.add",
            "description": "Add numbers",
            "annotations": {"readonly": true},
            "tags": []
        })]);
        let output = cmd_describe(&registry, "math.add", Some("table")).unwrap();
        assert!(
            output.contains("Annotations:"),
            "annotations section must be present"
        );
        assert!(output.contains("readonly"), "annotation key must appear");
    }

    #[test]
    fn test_cmd_describe_json_omits_null_fields() {
        // Module with no input_schema, output_schema, annotations.
        let registry = MockRegistry::new(vec![mock_module("a.b", "Desc", &[])]);
        let output = cmd_describe(&registry, "a.b", Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("input_schema").is_none());
        assert!(parsed.get("output_schema").is_none());
        assert!(parsed.get("annotations").is_none());
    }

    #[test]
    fn test_cmd_describe_json_includes_all_fields() {
        let registry = MockRegistry::new(vec![serde_json::json!({
            "module_id": "math.add",
            "description": "Add two numbers",
            "input_schema": {"type": "object", "properties": {"a": {"type": "integer"}}},
            "output_schema": {"type": "object", "properties": {"result": {"type": "integer"}}},
            "annotations": {"readonly": false},
            "tags": ["math", "core"]
        })]);
        let output = cmd_describe(&registry, "math.add", Some("json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("input_schema").is_some());
        assert!(parsed.get("output_schema").is_some());
        assert!(parsed.get("annotations").is_some());
        assert!(parsed.get("tags").is_some());
    }

    #[test]
    fn test_cmd_describe_with_x_fields_table_shows_extension_section() {
        let registry = MockRegistry::new(vec![serde_json::json!({
            "module_id": "a.b",
            "description": "Desc",
            "x-custom": "custom-value",
            "tags": []
        })]);
        let output = cmd_describe(&registry, "a.b", Some("table")).unwrap();
        assert!(
            output.contains("Extension Metadata:") || output.contains("x-custom"),
            "x-fields must appear in table output"
        );
    }

    // --- per-subcommand registrars (audit D9-W3, replaces the deleted
    // register_discovery_commands wrapper) ---

    #[test]
    fn test_register_list_command_adds_list() {
        let root = Command::new("apcore-cli");
        let cmd = register_list_command(root);
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"list"),
            "must have 'list' subcommand, got {names:?}"
        );
    }

    #[test]
    fn test_register_describe_command_adds_describe() {
        let root = Command::new("apcore-cli");
        let cmd = register_describe_command(root);
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"describe"),
            "must have 'describe' subcommand, got {names:?}"
        );
    }

    #[test]
    fn test_list_command_with_tag_filter() {
        let cmd = list_command();
        let arg_names: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(arg_names.contains(&"tag"), "list must have --tag flag");
    }

    #[test]
    fn test_describe_command_module_not_found() {
        // Verify module_id positional arg is present.
        let cmd = describe_command();
        let positionals: Vec<&str> = cmd
            .get_positionals()
            .filter_map(|a| a.get_id().as_str().into())
            .collect();
        assert!(
            positionals.contains(&"module_id"),
            "describe must have module_id positional, got {positionals:?}"
        );
    }

    // --- Per-subcommand registrars (FE-13) ---

    fn find_subcommand<'a>(cmd: &'a Command, name: &str) -> Option<&'a Command> {
        cmd.get_subcommands().find(|c| c.get_name() == name)
    }

    #[test]
    fn test_register_list_command_attaches_list() {
        let root = Command::new("apcli");
        let cmd = register_list_command(root);
        let list = find_subcommand(&cmd, "list").expect("'list' subcommand must be attached");
        let long_flags: Vec<&str> = list.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(
            long_flags.contains(&"tag"),
            "'list' must expose --tag flag, got {long_flags:?}"
        );
    }

    #[test]
    fn test_register_describe_command_attaches_describe() {
        let root = Command::new("apcli");
        let cmd = register_describe_command(root);
        let describe =
            find_subcommand(&cmd, "describe").expect("'describe' subcommand must be attached");
        let positionals: Vec<&str> = describe
            .get_positionals()
            .map(|a| a.get_id().as_str())
            .collect();
        assert!(
            positionals.contains(&"module_id"),
            "'describe' must require module_id positional, got {positionals:?}"
        );
        let module_id_arg = describe
            .get_arguments()
            .find(|a| a.get_id().as_str() == "module_id")
            .expect("module_id arg must exist");
        assert!(
            module_id_arg.is_required_set(),
            "'describe' module_id positional must be required"
        );
    }

    #[test]
    fn test_register_exec_command_attaches_exec() {
        let root = Command::new("apcli");
        let cmd = register_exec_command(root);
        let exec = find_subcommand(&cmd, "exec").expect("'exec' subcommand must be attached");
        let positionals: Vec<&str> = exec
            .get_positionals()
            .map(|a| a.get_id().as_str())
            .collect();
        assert!(
            positionals.contains(&"module_id"),
            "'exec' must require module_id positional, got {positionals:?}"
        );
        let module_id_arg = exec
            .get_arguments()
            .find(|a| a.get_id().as_str() == "module_id")
            .expect("module_id arg must exist");
        assert!(
            module_id_arg.is_required_set(),
            "'exec' module_id positional must be required"
        );
    }

    #[test]
    fn test_register_validate_command_attaches_validate() {
        let root = Command::new("apcli");
        let cmd = crate::validate::register_validate_command(root);
        assert!(
            find_subcommand(&cmd, "validate").is_some(),
            "'validate' subcommand must be attached"
        );
    }

    #[test]
    fn test_per_subcommand_registrars_can_be_called_independently() {
        // Attach only `list` to a fresh group; describe/exec/validate must be
        // absent. Proves registrars are composable without implicit coupling.
        let root = Command::new("apcli");
        let cmd = register_list_command(root);
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(
            names.contains(&"list"),
            "'list' must be present, got {names:?}"
        );
        assert!(
            !names.contains(&"describe"),
            "'describe' must NOT be present when only list was registered, got {names:?}"
        );
        assert!(
            !names.contains(&"exec"),
            "'exec' must NOT be present when only list was registered, got {names:?}"
        );
        assert!(
            !names.contains(&"validate"),
            "'validate' must NOT be present when only list was registered, got {names:?}"
        );
    }
}
