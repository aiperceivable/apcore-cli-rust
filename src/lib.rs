// apcore-cli — Command-line interface for apcore modules
// Library root: re-exports all public API items.
// Protocol spec: FE-01 through FE-10, SEC-01 through SEC-04

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
pub const EXIT_CONFIG_NAMESPACE_RESERVED: i32 = 78;
pub const EXIT_CONFIG_NAMESPACE_DUPLICATE: i32 = 78;
pub const EXIT_CONFIG_ENV_PREFIX_CONFLICT: i32 = 78;
pub const EXIT_CONFIG_MOUNT_ERROR: i32 = 66;
pub const EXIT_CONFIG_BIND_ERROR: i32 = 65;
pub const EXIT_ERROR_FORMATTER_DUPLICATE: i32 = 70;
pub const EXIT_SIGINT: i32 = 130;

// Re-export primary public types at crate root.
pub use approval::{check_approval, ApprovalError};
pub use cli::{
    build_module_command, build_module_command_with_limit, collect_input,
    collect_input_from_reader, get_docs_url, is_verbose_help, set_audit_logger, set_docs_url,
    set_executables, set_verbose_help, validate_module_id, GroupedModuleGroup, ModuleExecutor,
};
pub use config::ConfigResolver;
pub use discovery::{
    cmd_describe, cmd_list, register_discovery_commands, ApCoreRegistryProvider, DiscoveryError,
    RegistryProvider,
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
