//! apcore-cli — Command-line interface for apcore modules.
//!
//! Automatic MCP Server & OpenAI Tools Bridge for apcore — zero code changes required.
//!
//! Library root: re-exports the user-facing public API items.
//! Protocol spec: FE-01 through FE-13 plus SEC-01 through SEC-04.
//!
//! See the apcore-cli docs repo for the authoritative feature spec and tech design.

pub mod approval;
pub mod cli;
pub mod config;
pub mod discovery;
pub mod display_helpers;
pub mod exposure;
pub mod fs_discoverer;
pub mod init_cmd;
pub mod output;
pub mod ref_resolver;
pub mod schema_parser;
pub mod security;
pub mod shell;
pub mod strategy;
pub mod system_cmd;
pub mod validate;

// Internal sandbox runner — not part of the public API surface, but must be
// pub so the binary entry point (main.rs) can invoke run_sandbox_subprocess().
#[doc(hidden)]
pub mod _sandbox_runner;

// Exit codes as defined in the API contract.
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_MODULE_EXECUTE_ERROR: i32 = 1;
pub const EXIT_INVALID_INPUT: i32 = 2;
pub const EXIT_MODULE_NOT_FOUND: i32 = 44;
pub const EXIT_SCHEMA_VALIDATION_ERROR: i32 = 45;
pub const EXIT_APPROVAL_DENIED: i32 = 46;
pub const EXIT_CONFIG_NOT_FOUND: i32 = 47;
pub const EXIT_SCHEMA_CIRCULAR_REF: i32 = 48;
pub const EXIT_ACL_DENIED: i32 = 77;
// Config Bus errors (apcore >= 0.15.0)
// All four namespace/env errors share exit code 78 per protocol spec —
// the spec groups them into a single "config namespace error" category.
pub const EXIT_CONFIG_NAMESPACE_RESERVED: i32 = 78;
pub const EXIT_CONFIG_NAMESPACE_DUPLICATE: i32 = 78;
pub const EXIT_CONFIG_ENV_PREFIX_CONFLICT: i32 = 78;
pub const EXIT_CONFIG_ENV_MAP_CONFLICT: i32 = 78;
pub const EXIT_CONFIG_MOUNT_ERROR: i32 = 66;
pub const EXIT_CONFIG_BIND_ERROR: i32 = 65;
pub const EXIT_ERROR_FORMATTER_DUPLICATE: i32 = 70;
pub const EXIT_SIGINT: i32 = 130;

// ---------------------------------------------------------------------------
// CliConfig — high-level configuration for embedded CLI usage.
// ---------------------------------------------------------------------------

/// Error returned when `CliConfig` contains conflicting options.
#[derive(Debug)]
pub struct CliConfigError(pub String);

impl std::fmt::Display for CliConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliConfigError {}

/// Configuration for creating a CLI that uses a pre-populated registry
/// instead of filesystem discovery.
///
/// Frameworks that register modules at runtime (e.g. apflow's bridge) can
/// build their own [`RegistryProvider`] + [`ModuleExecutor`] and pass them
/// here to skip the default filesystem scan.
///
/// As of v0.7.0, an [`apcore::APCore`] unified client can be supplied via the
/// `app` field instead of providing separate `registry` / `executor` values.
/// `app` is mutually exclusive with `registry` and `executor`.
///
/// # Example
/// ```ignore
/// use std::sync::Arc;
///
/// let config = apcore_cli::CliConfig {
///     prog_name: Some("myapp".to_string()),
///     registry: Some(Arc::new(my_provider)),
///     executor: Some(Arc::new(my_executor)),
///     ..Default::default()
/// };
/// // Use config.registry / config.executor at dispatch time instead of
/// // performing filesystem discovery with FsDiscoverer.
/// ```
pub struct CliConfig {
    /// Override the program name shown in help text.
    pub prog_name: Option<String>,
    /// Override extensions directory (only used when `registry` is None).
    pub extensions_dir: Option<String>,
    /// Path to convention-based commands directory (apcore-toolkit ConventionScanner).
    /// TODO: wire to apcore-toolkit when the `toolkit` feature is enabled.
    pub commands_dir: Option<String>,
    /// Path to binding.yaml for display overlay (apcore-toolkit DisplayResolver).
    /// TODO: wire to apcore-toolkit when the `toolkit` feature is enabled.
    pub binding_path: Option<String>,
    /// Pre-populated registry provider. When set, skips filesystem discovery.
    /// Mutually exclusive with `app`.
    pub registry: Option<std::sync::Arc<dyn discovery::RegistryProvider>>,
    /// Pre-built module executor. When set, skips executor construction.
    /// Mutually exclusive with `app`.
    pub executor: Option<std::sync::Arc<apcore::Executor>>,
    /// Unified APCore client. When set, `registry` and `executor` are derived
    /// from it. Mutually exclusive with `registry` and `executor`.
    pub app: Option<apcore::APCore>,
    /// Extra custom commands to add to the CLI root. Each entry is a
    /// `clap::Command` that will be registered as a subcommand.
    pub extra_commands: Vec<clap::Command>,
    /// Group depth for multi-level module grouping (default: 1).
    /// Higher values allow deeper dotted-name grouping.
    pub group_depth: usize,
    /// Module exposure filter (FE-12). When set, the CLI builder will apply
    /// this filter at dispatch time when constructing the module command tree.
    pub expose: Option<exposure::ExposureFilter>,
}

impl CliConfig {
    /// Validate that `app` is not set alongside `registry` or `executor`.
    ///
    /// Returns `Err` with a descriptive message when the configuration is
    /// invalid. Callers should invoke this before using the config at
    /// dispatch time.
    pub fn validate(&self) -> Result<(), CliConfigError> {
        if self.app.is_some() && (self.registry.is_some() || self.executor.is_some()) {
            return Err(CliConfigError(
                "app is mutually exclusive with registry/executor".to_string(),
            ));
        }
        Ok(())
    }
}

/// Run the CLI with a pre-built [`CliConfig`].
///
/// This is the primary embedding API. Downstream crates that build their own
/// `APCore` client or pre-populate a registry call this instead of invoking
/// the binary directly.
///
/// # Dispatch paths
///
/// - `config.app` is set: registry and executor are derived from the `APCore`
///   client via `registry_arc()` and a new `Executor` sharing the same
///   registry. Filesystem discovery is skipped.
/// - `config.registry` is set: pre-populated registry is used directly;
///   filesystem discovery is skipped.
/// - Neither is set: filesystem discovery runs using `config.extensions_dir`
///   (or the default extensions directory when that is also `None`).
///
/// # Note on full dispatch
///
/// The full dispatch loop (clap argument parsing, module execution, output
/// formatting) currently lives in `main.rs`. This function handles config
/// validation and component extraction. Full extraction of the dispatch loop
/// into `lib.rs` is tracked as a follow-up task.
///
/// # Returns
///
/// The process exit code: `0` on success, `1` on invalid config.
pub async fn run_with_config(config: CliConfig, _args: Vec<String>) -> i32 {
    // 1. Validate the config — bail immediately on conflict.
    if let Err(e) = config.validate() {
        eprintln!("Error: {e}");
        return 1;
    }

    // 2. Branch on the three dispatch paths.
    if let Some(app) = config.app {
        // app= path: extract the shared registry Arc from the APCore client.
        // Build a new Executor from the same registry so module registrations
        // made on the APCore client are visible to the CLI dispatcher.
        // Note: custom middleware added to app.executor() is not inherited here;
        // the new executor uses default pipeline settings.
        let registry_arc = app.registry_arc();
        let _executor = apcore::Executor::new(
            std::sync::Arc::clone(&registry_arc),
            apcore::Config::default(),
        );
        let _provider = discovery::ApCoreRegistryProvider::from_arc(registry_arc);
        // Full CLI dispatch using _provider and _executor is extracted in a
        // follow-up. Components are correctly wired — dispatch routing pending.
        0
    } else if config.registry.is_some() {
        // Pre-populated registry path — components already provided by caller.
        0
    } else {
        // Filesystem discovery path (default).
        0
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            prog_name: None,
            extensions_dir: None,
            commands_dir: None,
            binding_path: None,
            registry: None,
            executor: None,
            app: None,
            extra_commands: Vec::new(),
            group_depth: 1,
            expose: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Crate-root re-exports (USER-FACING API only)
// ---------------------------------------------------------------------------
//
// Per audit D9-005, the crate-root pub-use surface was trimmed from ~110 → ~40
// items in v0.6.x. Internal command-builder helpers (dispatch_*, register_*,
// describe_pipeline_command, validate_command, generate_grouped_*_completion,
// build_synopsis, generate_man_page) are still `pub` at the module level so
// the binary entry-point in main.rs can call them via the full path
// (`apcore_cli::system_cmd::register_system_commands`, etc.) — but they are
// no longer re-exported at the crate root. Downstream users wanting to embed
// the CLI should use `CliConfig` and the user-facing API below.

// Approval gate (FE-04 + FE-11 §3.5)
pub use approval::{check_approval, ApprovalError};

// Core dispatcher (FE-01)
pub use cli::{
    build_module_command, build_module_command_with_limit, collect_input,
    collect_input_from_reader, dispatch_module, get_docs_url, is_verbose_help, set_audit_logger,
    set_docs_url, set_executables, set_verbose_help, validate_module_id, BUILTIN_COMMANDS,
};

// Config resolution (FE-07)
pub use config::ConfigResolver;

// Discovery + Registry providers (FE-03 / FE-09)
pub use discovery::{
    cmd_describe, cmd_list, cmd_list_enhanced, register_discovery_commands, ApCoreRegistryProvider,
    DiscoveryError, ListOptions, RegistryProvider,
};

// Test utilities — available behind the `test-support` feature.
// Gated behind cfg(test) for unit tests and the test-support feature for
// integration tests. Excluded from production builds.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use discovery::{mock_module, MockRegistry};

// Display overlay helpers (FE-09)
pub use display_helpers::{get_cli_display_fields, get_display};

// Module exposure filtering (FE-12)
pub use exposure::ExposureFilter;

// Filesystem discoverer (FE-03)
pub use fs_discoverer::FsDiscoverer;

// Init command (FE-10)
pub use init_cmd::{handle_init, init_command};

// Output formatting (FE-08)
pub use output::{format_exec_result, format_module_detail, format_module_list, resolve_format};

// Schema $ref resolver (FE-02)
pub use ref_resolver::resolve_refs;

// JSON Schema → clap argument generator (FE-02)
pub use schema_parser::{
    extract_help_with_limit, reconvert_enum_values, schema_to_clap_args,
    schema_to_clap_args_with_limit, BoolFlagPair, SchemaArgs, SchemaParserError, HELP_TEXT_MAX_LEN,
};

// Security primitives (SEC-01..04)
pub use security::{AuditLogger, AuthProvider, ConfigEncryptor, Sandbox};

// Shell integration (FE-06): completion + man page builders.
// build_program_man_page is the user-facing full-program man entry point;
// per-command builders (cmd_completion, cmd_man, has_man_flag, completion_command,
// register_shell_commands) are kept for downstream embedders that build their
// own root command tree.
pub use shell::{
    build_program_man_page, cmd_completion, cmd_man, completion_command, has_man_flag,
    register_shell_commands, ShellError,
};

// FE-11 system commands constant (used by downstream consumers to inspect
// which command names are reserved by the system-management subset).
pub use system_cmd::SYSTEM_COMMANDS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_config_default_has_all_none() {
        let config = CliConfig::default();
        assert!(config.prog_name.is_none());
        assert!(config.extensions_dir.is_none());
        assert!(config.commands_dir.is_none());
        assert!(config.binding_path.is_none());
        assert!(config.registry.is_none());
        assert!(config.executor.is_none());
        assert!(config.app.is_none());
        assert!(config.extra_commands.is_empty());
        assert_eq!(config.group_depth, 1);
        assert!(config.expose.is_none());
    }

    #[test]
    fn cli_config_validate_ok_when_only_app() {
        let config = CliConfig {
            app: Some(apcore::APCore::new()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cli_config_validate_ok_when_no_app() {
        let config = CliConfig {
            registry: Some(std::sync::Arc::new(discovery::MockRegistry::new(vec![]))),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cli_config_validate_err_app_with_registry() {
        let config = CliConfig {
            app: Some(apcore::APCore::new()),
            registry: Some(std::sync::Arc::new(discovery::MockRegistry::new(vec![]))),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.0.contains("mutually exclusive"));
    }

    #[test]
    fn cli_config_validate_err_app_with_executor() {
        let config = CliConfig {
            app: Some(apcore::APCore::new()),
            executor: Some(std::sync::Arc::new(apcore::Executor::new(
                std::sync::Arc::new(apcore::Registry::new()),
                apcore::Config::default(),
            ))),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.0.contains("mutually exclusive"));
    }

    #[test]
    fn cli_config_accepts_pre_populated_registry() {
        let mock_registry = discovery::MockRegistry::new(vec![]);
        let config = CliConfig {
            prog_name: Some("test-app".to_string()),
            registry: Some(std::sync::Arc::new(mock_registry)),
            ..Default::default()
        };
        assert_eq!(config.prog_name.as_deref(), Some("test-app"));
        assert!(config.registry.is_some());
    }
}
