// apcore-cli — Command-line interface for apcore modules
// Library root: re-exports all public API items.
// Protocol spec: FE-01 through FE-11, SEC-01 through SEC-04

pub mod approval;
pub mod cli;
pub mod config;
pub mod discovery;
pub mod display_helpers;
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

/// Configuration for creating a CLI that uses a pre-populated registry
/// instead of filesystem discovery.
///
/// Frameworks that register modules at runtime (e.g. apflow's bridge) can
/// build their own [`RegistryProvider`] + [`ModuleExecutor`] and pass them
/// here to skip the default filesystem scan.
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
    /// Pre-populated registry provider. When set, skips filesystem discovery.
    pub registry: Option<std::sync::Arc<dyn discovery::RegistryProvider>>,
    /// Pre-built module executor. When set, skips executor construction.
    pub executor: Option<std::sync::Arc<dyn cli::ModuleExecutor>>,
    /// Extra custom commands to add to the CLI root. Each entry is a
    /// `clap::Command` that will be registered as a subcommand.
    pub extra_commands: Vec<clap::Command>,
    /// Group depth for multi-level module grouping (default: 1).
    /// Higher values allow deeper dotted-name grouping.
    pub group_depth: usize,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            prog_name: None,
            extensions_dir: None,
            registry: None,
            executor: None,
            extra_commands: Vec::new(),
            group_depth: 1,
        }
    }
}

// Re-export primary public types at crate root.
pub use approval::{check_approval, ApprovalError};
pub use cli::{
    build_module_command, build_module_command_with_limit, collect_input,
    collect_input_from_reader, get_docs_url, is_verbose_help, set_audit_logger, set_docs_url,
    set_executables, set_verbose_help, validate_module_id, GroupedModuleGroup, ModuleExecutor,
};
pub use config::ConfigResolver;
pub use discovery::{
    cmd_describe, cmd_list, cmd_list_enhanced, register_discovery_commands, ApCoreRegistryProvider,
    DiscoveryError, ListOptions, RegistryProvider,
};
pub use display_helpers::{get_cli_display_fields, get_display};
pub use init_cmd::{handle_init, init_command};
// Test utilities — available but hidden from docs.
// Gated behind cfg(test) for unit tests and the test-support feature for
// integration tests. Excluded from production builds.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use discovery::{mock_module, MockRegistry};
pub use fs_discoverer::FsDiscoverer;
pub use output::{format_exec_result, format_module_detail, format_module_list, resolve_format};
pub use ref_resolver::resolve_refs;
pub use schema_parser::{
    extract_help_with_limit, reconvert_enum_values, schema_to_clap_args,
    schema_to_clap_args_with_limit, BoolFlagPair, SchemaArgs, SchemaParserError, HELP_TEXT_MAX_LEN,
};
pub use security::{AuditLogger, AuthProvider, ConfigEncryptor, Sandbox};
pub use shell::{
    build_program_man_page, build_synopsis, cmd_completion, cmd_man, completion_command,
    generate_man_page, has_man_flag, register_shell_commands, ShellError, KNOWN_BUILTINS,
};
pub use strategy::{
    describe_pipeline_command, dispatch_describe_pipeline, register_pipeline_command,
};
pub use system_cmd::{register_system_commands, SYSTEM_COMMANDS};
pub use validate::{
    dispatch_validate, format_preflight_result, register_validate_command, validate_command,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_config_default_has_all_none() {
        let config = CliConfig::default();
        assert!(config.prog_name.is_none());
        assert!(config.extensions_dir.is_none());
        assert!(config.registry.is_none());
        assert!(config.executor.is_none());
        assert!(config.extra_commands.is_empty());
        assert_eq!(config.group_depth, 1);
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
        assert!(config.executor.is_none());
    }
}
