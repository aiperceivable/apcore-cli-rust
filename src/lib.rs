//! apcore-cli — Command-line interface for apcore modules.
//!
//! Automatic MCP Server & OpenAI Tools Bridge for apcore — zero code changes required.
//!
//! Library root: re-exports the user-facing public API items.
//! Protocol spec: FE-01 through FE-13 plus SEC-01 through SEC-04.
//!
//! See the apcore-cli docs repo for the authoritative feature spec and tech design.

pub mod approval;
pub mod builtin_group;
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
pub mod sandbox_runner;

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
pub const EXIT_CONFIG_MOUNT_ERROR: i32 = 66;
pub const EXIT_CONFIG_BIND_ERROR: i32 = 65;
pub const EXIT_ERROR_FORMATTER_DUPLICATE: i32 = 70;
pub const EXIT_SIGINT: i32 = 130;

// ---------------------------------------------------------------------------
// FE-13 apcli subcommand dispatcher (§4.9)
// ---------------------------------------------------------------------------

/// Subcommand names that are registered regardless of the resolved visibility
/// mode's include/exclude filter. `exec` is the documented always-registered
/// escape hatch (spec §4.9) so downstream callers can always invoke modules
/// by ID even when the apcli group is configured with a minimal surface.
pub const APCLI_ALWAYS_REGISTERED: &[&str] = &["exec"];

/// Central dispatcher for the 13 canonical apcli subcommands. Walks a fixed
/// registration table and honors [`ApcliGroup::resolve_visibility`] for
/// include/exclude modes. Under `"all"` or `"none"` all 13 subcommands are
/// registered (spec §4.9 registration rules table); under `"include"` only
/// listed subcommands + [`APCLI_ALWAYS_REGISTERED`]; under `"exclude"` all
/// except listed + [`APCLI_ALWAYS_REGISTERED`].
///
/// * `apcli_group` — the `apcli` clap [`Command`](clap::Command) to receive
///   subcommands.
/// * `cfg` — the resolved apcli visibility configuration.
/// * `prog_name` — program name forwarded to `register_completion_command`.
///
/// Returns the updated command with registered subcommands attached.
pub fn register_apcli_subcommands(
    apcli_group: clap::Command,
    cfg: &ApcliGroup,
    prog_name: &str,
) -> clap::Command {
    type Registrar = Box<dyn FnOnce(clap::Command) -> clap::Command>;

    let prog_name_for_completion = prog_name.to_string();
    let table: Vec<(&'static str, Registrar)> = vec![
        ("list", Box::new(discovery::register_list_command)),
        ("describe", Box::new(discovery::register_describe_command)),
        ("exec", Box::new(discovery::register_exec_command)),
        ("validate", Box::new(validate::register_validate_command)),
        ("init", Box::new(init_cmd::register_init_command)),
        ("health", Box::new(system_cmd::register_health_command)),
        ("usage", Box::new(system_cmd::register_usage_command)),
        ("enable", Box::new(system_cmd::register_enable_command)),
        ("disable", Box::new(system_cmd::register_disable_command)),
        ("reload", Box::new(system_cmd::register_reload_command)),
        ("config", Box::new(system_cmd::register_config_command)),
        (
            "completion",
            Box::new(move |cli| shell::register_completion_command(cli, &prog_name_for_completion)),
        ),
        (
            "describe-pipeline",
            Box::new(strategy::register_pipeline_command),
        ),
    ];

    let mode = cfg.resolve_visibility();
    let mut cmd = apcli_group;
    for (name, registrar) in table {
        let should_register = match mode {
            // mode:"none" still registers all subcommands — the group itself
            // is hidden but subcommands remain individually reachable
            // (spec §4.6 / §4.9 hidden-but-reachable).
            "all" | "none" => true,
            _ => APCLI_ALWAYS_REGISTERED.contains(&name) || cfg.is_subcommand_included(name),
        };
        if should_register {
            cmd = registrar(cmd);
        }
    }
    cmd
}

// ---------------------------------------------------------------------------
// Crate-root re-exports (USER-FACING API only)
// ---------------------------------------------------------------------------
//
// Per audit D9-005, the crate-root pub-use surface was trimmed from ~110 → ~40
// items in v0.6.x. Internal command-builder helpers (register_*,
// describe_pipeline_command, validate_command, generate_grouped_*_completion,
// build_synopsis, generate_man_page) are now `pub(crate)` at the module level
// — they are only consumed by lib.rs's per-subcommand registrar table and the
// in-crate test suite. The dispatch_* fns in `system_cmd` and `strategy`
// remain `pub` because the binary entry-point in main.rs calls them via the
// full path (`apcore_cli::system_cmd::dispatch_health`, etc.) and main.rs is
// a separate binary crate.
//
// **No `create_cli` / `run_with_config` factory in Rust** (cross-SDK parity
// note from audit D1-005, 2026-04-26): Python exposes `apcore_cli.create_cli`
// and TypeScript exposes `createCli` from `apcore-cli`; Rust intentionally
// has no equivalent factory. The high-level `CliConfig` / `run_with_config`
// embedding API was removed in v0.7.0 (D9-001/002) — it never had a working
// dispatch loop. An embedding API will be reintroduced when actually
// implemented; until then, downstream Rust users invoke the binary directly
// or compose the per-subcommand registrars (e.g.,
// `register_completion_command`) onto a `clap::Command` they own. The
// Python `allowed_prefixes` parameter on `create_cli` is therefore a
// Python-only safety knob with no Rust counterpart at this writing.

// Approval gate (FE-04 + FE-11 §3.5)
pub use approval::{
    check_approval, ApprovalDeniedError, ApprovalError, ApprovalResult, ApprovalStatus,
    ApprovalTimeoutError, CliApprovalHandler,
};

// Built-in command group (FE-13)
pub use builtin_group::{ApcliConfig, ApcliGroup, ApcliMode};

// Core dispatcher (FE-01)
pub use cli::{
    build_module_command, build_module_command_with_limit, collect_input,
    collect_input_from_reader, dispatch_module, get_docs_url, is_verbose_help, set_audit_logger,
    set_docs_url, set_executables, set_verbose_help, validate_module_id,
};

// FE-13 retires `cli::BUILTIN_COMMANDS`. Downstream consumers that pinned to
// the old symbol continue to compile via the deprecated alias on `cli::` for
// one MINOR cycle; the re-export at the crate root is dropped.
pub use builtin_group::{APCLI_SUBCOMMAND_NAMES, RESERVED_GROUP_NAMES};

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
    RESERVED_PROPERTY_NAMES,
};

// Security primitives (SEC-01..04)
pub use security::{
    AuditLogger, AuthProvider, AuthenticationError, ConfigDecryptionError, ConfigEncryptor,
    ModuleExecutionError, ModuleNotFoundError, Sandbox, SchemaValidationError,
};

// Shell integration (FE-06): completion + man page builders.
// build_program_man_page is the user-facing full-program man entry point;
// per-command builders (cmd_completion, cmd_man, has_man_flag, completion_command)
// are kept for downstream embedders that build their own root command tree.
// Embedders compose them directly: each registers the corresponding subcommand
// onto a clap Command. The previous register_shell_commands wrapper was a
// 2-line passthrough with no production callers and was removed in v0.7.0
// (D9-003) — embedders should call register_completion_command and
// register_man_command directly.
pub use shell::{
    build_program_man_page, cmd_completion, cmd_man, completion_command, has_man_flag, ShellError,
};

// FE-11 system commands constant (used by downstream consumers to inspect
// which command names are reserved by the system-management subset).
pub use system_cmd::SYSTEM_COMMANDS;

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard: the Registrar table built inside
    /// `register_apcli_subcommands` must cover every name in
    /// `APCLI_SUBCOMMAND_NAMES`. Adding a subcommand to one list without the
    /// other produces a silent mismatch that was previously invisible across
    /// three declaration sites.
    #[test]
    fn registrar_table_covers_all_apcli_subcommand_names() {
        // Drive visibility to "all" so every table entry is registered.
        let cfg = builtin_group::ApcliGroup::from_yaml(None, /*registry_injected*/ false);
        let root = clap::Command::new("root").about("drift-guard root");
        let built = register_apcli_subcommands(root, &cfg, "apcore-cli");
        let registered: Vec<&str> = built.get_subcommands().map(|s| s.get_name()).collect();
        for name in APCLI_SUBCOMMAND_NAMES {
            assert!(
                registered.contains(name),
                "APCLI_SUBCOMMAND_NAMES lists '{name}' but register_apcli_subcommands \
                 did not attach it — drift between builtin_group.rs constant and the \
                 Registrar table in lib.rs"
            );
        }
    }
}
