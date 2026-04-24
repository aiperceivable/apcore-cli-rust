// apcore-cli — Binary entry point.
// Protocol spec: FE-01 (create_cli, main, extract_extensions_dir, init_tracing)

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use apcore_cli::EXIT_CONFIG_NOT_FOUND;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{reload, EnvFilter};

// ---------------------------------------------------------------------------
// FE-13 §11.2 deprecation shims (standalone-only)
// ---------------------------------------------------------------------------

/// Root-level command names that had a flat pre-v0.7 shape and moved under
/// the `apcli` group in v0.7. A thin hidden shim at the root forwards
/// invocations to the corresponding `apcli <name>` subcommand after printing
/// a deprecation warning. Removed in v0.8 per spec §11.3.
///
/// Sourced directly from [`apcore_cli::APCLI_SUBCOMMAND_NAMES`] so the
/// deprecation-shim set cannot drift from the canonical subcommand list.
/// Adding a subcommand means updating one constant in `builtin_group.rs`
/// (+ registering a handler in `lib.rs::register_apcli_subcommands`).
const DEPRECATED_ROOT_COMMANDS: &[&str] = apcore_cli::APCLI_SUBCOMMAND_NAMES;

/// Print the FE-13 §11.2 deprecation warning for a root-level invocation of
/// a subcommand that has moved under the `apcli` group.
fn print_deprecation_warning(name: &str, prog: &str) {
    eprintln!(
        "WARNING: '{name}' as a root-level command is deprecated. \
         Use '{prog} apcli {name}' instead.\n         \
         Will be removed in v0.8. See: \
         https://aiperceivable.github.io/apcore-cli/features/builtin-group/#11-migration"
    );
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Valid log-level choices for the --log-level flag.
pub const LOG_LEVELS: &[&str] = &["DEBUG", "INFO", "WARNING", "ERROR"];

// ---------------------------------------------------------------------------
// Reload handle — allows --log-level to update the filter at runtime.
// ---------------------------------------------------------------------------

type ReloadHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

static RELOAD_HANDLE: OnceLock<ReloadHandle> = OnceLock::new();

// ---------------------------------------------------------------------------
// extract_extensions_dir
// ---------------------------------------------------------------------------

/// Pre-parse a `--flag` option from raw argv before clap processes arguments.
///
/// Scans argv linearly -- no clap involvement. Handles both `--flag VALUE`
/// and `--flag=VALUE` forms.
fn extract_argv_option(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(val) = arg.strip_prefix(&prefix) {
            return Some(val.to_string());
        }
    }
    None
}

/// Check if `--verbose` is present in raw argv (pre-parse, before clap).
fn has_verbose_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--verbose")
}

/// Pre-parse `--extensions-dir` from raw argv before clap processes arguments.
pub fn extract_extensions_dir(args: &[String]) -> Option<String> {
    extract_argv_option(args, "--extensions-dir")
}

/// Pre-parse `--commands-dir` from raw argv before clap processes arguments.
pub fn extract_commands_dir(args: &[String]) -> Option<String> {
    extract_argv_option(args, "--commands-dir")
}

/// Pre-parse `--binding` from raw argv before clap processes arguments.
pub fn extract_binding_path(args: &[String]) -> Option<String> {
    extract_argv_option(args, "--binding")
}

// ---------------------------------------------------------------------------
// render_man_page
// ---------------------------------------------------------------------------

/// Render a roff man page to stdout.
///
/// When stdout is a TTY, attempts to render through `mandoc` or `groff` and
/// pipe through a pager for formatted display. When stdout is not a TTY
/// (piped or redirected), outputs raw roff for file redirection.
fn render_man_page(roff: &str) {
    use std::io::{IsTerminal, Write};
    use std::process::{Command, Stdio};

    let is_tty = std::io::stdout().is_terminal();
    if !is_tty {
        print!("{roff}");
        return;
    }

    // Try mandoc first (macOS/BSD), then groff.
    let renderers: &[(&str, &[&str])] = &[("mandoc", &["-a"]), ("groff", &["-man", "-Tutf8"])];
    for &(cmd, args) in renderers {
        let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(roff.as_bytes());
        }
        let Ok(output) = child.wait_with_output() else {
            continue;
        };
        if !output.status.success() || output.stdout.is_empty() {
            continue;
        }
        // Pipe rendered output through PAGER or less.
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
        if let Ok(mut pager_child) = Command::new(&pager)
            .arg("-R")
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
        {
            if let Some(mut stdin) = pager_child.stdin.take() {
                let _ = stdin.write_all(&output.stdout);
            }
            let _ = pager_child.wait();
            return;
        }
    }

    // Fallback: raw roff output.
    print!("{roff}");
}

// ---------------------------------------------------------------------------
// resolve_prog_name
// ---------------------------------------------------------------------------

/// Resolve the program name from argv[0] basename, with an explicit override.
fn resolve_prog_name(prog_name: Option<String>) -> String {
    if let Some(name) = prog_name {
        return name;
    }
    std::env::args()
        .next()
        .as_deref()
        .and_then(|s| Path::new(s).file_name()?.to_str())
        .unwrap_or("apcore-cli")
        .to_string()
}

// ---------------------------------------------------------------------------
// init_tracing
// ---------------------------------------------------------------------------

/// Initialise tracing with three-tier log-level precedence:
/// APCORE_CLI_LOGGING_LEVEL > APCORE_LOGGING_LEVEL > WARNING.
///
/// Stores a reload handle in `RELOAD_HANDLE` so the log level can be updated
/// at runtime when --log-level is passed.
pub fn init_tracing(log_level: &str) {
    let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("warn"));

    let (filtered_layer, handle) = reload::Layer::new(filter);

    let _ = tracing_subscriber::registry()
        .with(filtered_layer)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .try_init();

    // Store handle for runtime reload; ignore if already set (e.g. in tests).
    let _ = RELOAD_HANDLE.set(handle);
}

/// Resolve the effective log level from environment and override.
fn resolve_log_level(override_level: Option<&str>) -> String {
    if let Some(level) = override_level {
        return level.to_string();
    }
    let cli_level = std::env::var("APCORE_CLI_LOGGING_LEVEL").unwrap_or_default();
    let global_level = std::env::var("APCORE_LOGGING_LEVEL").unwrap_or_default();
    if !cli_level.is_empty() {
        cli_level
    } else if !global_level.is_empty() {
        global_level
    } else {
        "warn".to_string()
    }
}

// ---------------------------------------------------------------------------
// validate_extensions_dir
// ---------------------------------------------------------------------------

/// Validate that the extensions directory exists and is readable.
///
/// Returns `Err(message)` if the directory is missing or unreadable.
fn validate_extensions_dir(ext_dir: &str) -> Result<(), String> {
    let path = Path::new(ext_dir);
    if !path.exists() {
        return Err(format!(
            "Extensions directory not found: '{ext_dir}'. \
             Set APCORE_EXTENSIONS_ROOT or verify the path."
        ));
    }
    if std::fs::read_dir(path).is_err() {
        return Err(format!(
            "Cannot read extensions directory: '{ext_dir}'. Check file permissions."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// create_cli
// ---------------------------------------------------------------------------

/// Build the root `clap::Command` tree.
///
/// When `validate` is true, prints an error and exits 47 if `extensions_dir` does not exist.
/// When `validate` is false (used for completion/man page generation),
/// skips the directory check.
fn build_cli_command(
    extensions_dir: Option<String>,
    prog_name: Option<String>,
    validate: bool,
) -> clap::Command {
    let name = resolve_prog_name(prog_name);

    // Resolve extensions_dir: flag > env var > default.
    let ext_dir = match extensions_dir {
        Some(dir) => dir,
        None => std::env::var("APCORE_EXTENSIONS_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "./extensions".to_string()),
    };

    // Validate extensions directory (only when running real commands).
    if validate {
        if let Err(msg) = validate_extensions_dir(&ext_dir) {
            eprintln!("Error: {msg}");
            std::process::exit(EXIT_CONFIG_NOT_FOUND);
        }
    }

    // Build root command.
    let mut cmd = clap::Command::new(name.clone())
        .version(env!("CARGO_PKG_VERSION"))
        .long_version(format!("{}, version {}", name, env!("CARGO_PKG_VERSION")))
        .about("CLI adapter for the apcore module ecosystem.")
        .after_help(
            "Use --help --verbose to show all options (including built-in apcore options).\n\
             Use --help --man to display a formatted man page.",
        )
        .allow_external_subcommands(true)
        .arg(
            clap::Arg::new("log-level")
                .long("log-level")
                .global(true)
                .value_parser(clap::builder::PossibleValuesParser::new(LOG_LEVELS))
                .ignore_case(true)
                .help("Log verbosity (DEBUG|INFO|WARNING|ERROR)."),
        )
        .arg(
            clap::Arg::new("verbose")
                .long("verbose")
                .global(true)
                .action(clap::ArgAction::SetTrue)
                .help(
                    "Show all options in help output \
                     (including built-in apcore options).",
                ),
        )
        .arg(
            clap::Arg::new("man")
                .long("man")
                .global(true)
                .action(clap::ArgAction::SetTrue)
                .hide(true)
                .help(
                    "Output man page in roff format \
                     (use with --help).",
                ),
        );

    // The binary is always standalone (registry not injected), so discovery
    // flags are unconditionally applied here. Embedders that call via
    // `run_with_config` pass `standalone=false` to skip them.
    cmd = apply_discovery_flags(cmd, /*standalone*/ true);

    // ----------------------------------------------------------------------
    // FE-13: Build the `apcli` subcommand group.
    // ----------------------------------------------------------------------
    // Resolve Tier 3 (apcore.yaml) visibility config, if any. resolve_object
    // returns None gracefully when the file is absent, unreadable, or
    // malformed — so this is safe to call unconditionally.
    let yaml_val = if Path::new("apcore.yaml").exists() {
        let resolver = apcore_cli::ConfigResolver::new(None, Some(PathBuf::from("apcore.yaml")));
        resolver.resolve_object("apcli")
    } else {
        None
    };
    // Binary is always standalone — registry is NOT injected.
    let apcli_cfg = apcore_cli::ApcliGroup::from_yaml(yaml_val, /*registry_injected*/ false);

    let apcli_group = clap::Command::new("apcli")
        .about("Built-in apcore-cli commands.")
        .hide(!apcli_cfg.is_group_visible());
    let apcli_group = apcore_cli::register_apcli_subcommands(apcli_group, &apcli_cfg, &name);
    cmd = cmd.subcommand(apcli_group);

    // `man` stays at root as a meta command (FE-13 spec §4.1).
    cmd = apcore_cli::shell::register_man_command(cmd);

    // ----------------------------------------------------------------------
    // FE-13 §11.2: Deprecation shims for the 13 former root-level commands.
    // Each shim is hidden from help and tolerates passthrough arguments. At
    // dispatch time the shim prints a WARNING and delegates to the same
    // handler used under `apcli <name>`.
    // Binary is always standalone, so shims are always registered.
    // ----------------------------------------------------------------------
    for shim_name in DEPRECATED_ROOT_COMMANDS {
        cmd = cmd.subcommand(
            clap::Command::new(*shim_name)
                .hide(true)
                .disable_help_flag(true)
                .allow_external_subcommands(true)
                .arg(
                    clap::Arg::new("")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                ),
        );
    }

    cmd
}

/// Attach discovery-related global flags (`--extensions-dir`, `--commands-dir`,
/// `--binding`) to the root command. Only applied in standalone mode — when
/// the CLI is embedded with a pre-injected registry, these flags have no
/// effect so they are omitted to keep help output clean.
fn apply_discovery_flags(cmd: clap::Command, standalone: bool) -> clap::Command {
    if !standalone {
        return cmd;
    }
    cmd.arg(
        clap::Arg::new("extensions-dir")
            .long("extensions-dir")
            .global(true)
            .value_name("PATH")
            .help("Path to apcore extensions directory."),
    )
    .arg(
        clap::Arg::new("commands-dir")
            .long("commands-dir")
            .global(true)
            .value_name("PATH")
            .help("Path to convention-based commands directory."),
    )
    .arg(
        clap::Arg::new("binding")
            .long("binding")
            .global(true)
            .value_name("PATH")
            .help("Path to binding.yaml for display overlay."),
    )
}

/// Build the root `clap::Command` tree with directory validation.
///
/// * `extensions_dir` — path to the extensions directory, validated here.
///   Exits 47 if provided but does not exist.
/// * `prog_name` — override the program name shown in help text.
pub fn create_cli(extensions_dir: Option<String>, prog_name: Option<String>) -> clap::Command {
    build_cli_command(extensions_dir, prog_name, true)
}

// ---------------------------------------------------------------------------
// FE-13 dispatch helpers
// ---------------------------------------------------------------------------

/// Build a standalone `apcli` clap group populated with all 13 canonical
/// subcommands. Used by the deprecation shim dispatchers to re-parse
/// shim-forwarded argv against the canonical `apcli <name>` command tree.
fn build_apcli_group_for_dispatch(prog_name: &str) -> clap::Command {
    let cfg = apcore_cli::ApcliGroup::from_cli_config(
        Some(apcore_cli::ApcliConfig {
            mode: apcore_cli::ApcliMode::All,
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let group = clap::Command::new("apcli")
        .no_binary_name(true)
        .subcommand_required(false);
    apcore_cli::register_apcli_subcommands(group, &cfg, prog_name)
}

/// Reconstruct the argv tail to forward from a root-level deprecation shim
/// to its canonical `apcli <name>` subcommand. Slices from the shim name
/// onward in `raw_args` so positional args + nested subcommands are
/// preserved verbatim, and prepends the subcommand name so the
/// `apcli` parser sees a valid invocation.
fn forward_shim_args(name: &str, raw_args: &[String]) -> Vec<String> {
    let mut out = vec![name.to_string()];
    if let Some(idx) = raw_args.iter().position(|a| a == name) {
        out.extend(raw_args.iter().skip(idx + 1).cloned());
    }
    out
}

/// Shared deprecation-shim prologue: print the FE-13 §11.2 warning, forward
/// the shim argv tail against a synthetic `apcli` group, and return the
/// inner `ArgMatches` for `name`. Collapses the 10-line prologue that used
/// to be duplicated across every shim arm.
fn parse_shim_for(name: &str, raw_args: &[String], prog_name: &str) -> clap::ArgMatches {
    print_deprecation_warning(name, prog_name);
    let forwarded = forward_shim_args(name, raw_args);
    let apcli_group = build_apcli_group_for_dispatch(prog_name);
    let m = apcli_group
        .try_get_matches_from(&forwarded)
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2);
        });
    m.subcommand_matches(name)
        .cloned()
        .expect("shim dispatch failed")
}

/// Shared `list` handler used by both `apcli list` and the root-level
/// deprecation shim.
fn handle_list(
    sub_m: &clap::ArgMatches,
    registry_provider: &std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider>,
) {
    let tags: Vec<&str> = sub_m
        .get_many::<String>("tag")
        .map(|vals| vals.map(|s| s.as_str()).collect())
        .unwrap_or_default();
    let format = sub_m.get_one::<String>("format").map(|s| s.as_str());
    let search = sub_m.get_one::<String>("search").map(|s| s.as_str());
    let status = sub_m.get_one::<String>("status").map(|s| s.as_str());
    let annotations: Vec<&str> = sub_m
        .get_many::<String>("annotation")
        .map(|vals| vals.map(|s| s.as_str()).collect())
        .unwrap_or_default();
    let sort = sub_m.get_one::<String>("sort").map(|s| s.as_str());
    let reverse = sub_m.get_flag("reverse");
    let deprecated = sub_m.get_flag("deprecated");
    let opts = apcore_cli::discovery::ListOptions {
        tags: &tags,
        explicit_format: format,
        search,
        status,
        annotations: &annotations,
        sort,
        reverse,
        deprecated,
    };
    match apcore_cli::discovery::cmd_list_enhanced(registry_provider.as_ref(), &opts) {
        Ok(output) => {
            println!("{output}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    }
}

/// Shared `describe` handler used by both `apcli describe` and the root-level
/// deprecation shim.
fn handle_describe(
    sub_m: &clap::ArgMatches,
    registry_provider: &std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider>,
) {
    let module_id = sub_m
        .get_one::<String>("module_id")
        .expect("module_id is required");
    let format = sub_m.get_one::<String>("format").map(|s| s.as_str());
    match apcore_cli::discovery::cmd_describe(registry_provider.as_ref(), module_id, format) {
        Ok(output) => {
            println!("{output}");
            std::process::exit(0);
        }
        Err(apcore_cli::discovery::DiscoveryError::ModuleNotFound(_)) => {
            eprintln!(
                "Error: {}",
                apcore_cli::discovery::DiscoveryError::ModuleNotFound(module_id.clone())
            );
            std::process::exit(apcore_cli::EXIT_MODULE_NOT_FOUND);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    }
}

/// Shared `exec` handler used by both `apcli exec` and the root-level
/// deprecation shim.
async fn handle_exec(
    sub_m: &clap::ArgMatches,
    registry_provider: &std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider>,
    apcore_executor: &apcore::Executor,
) {
    let module_id = sub_m
        .get_one::<String>("module_id")
        .expect("module_id is required");
    apcore_cli::cli::dispatch_module(module_id, sub_m, registry_provider, apcore_executor).await;
}

/// Shared `completion` handler used by both `apcli completion` and the
/// root-level deprecation shim.
fn handle_completion(sub_m: &clap::ArgMatches, prog_name: &str) {
    let shell = *sub_m
        .get_one::<clap_complete::Shell>("shell")
        .expect("shell is required");
    let mut cmd = build_cli_command(None, Some(prog_name.to_string()), false);
    let output = apcore_cli::shell::cmd_completion(shell, prog_name, &mut cmd);
    print!("{output}");
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Intercept --internal-sandbox-runner before clap processes argv.
    // This must happen first so clap does not reject the unknown flag.
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.get(1).map(String::as_str) == Some("--internal-sandbox-runner") {
        if let Err(e) = apcore_cli::sandbox_runner::run_sandbox_subprocess().await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    // Intercept --help --man: generate full program man page and exit.
    if apcore_cli::shell::has_man_flag(&raw_args)
        && raw_args.iter().any(|a| a == "--help" || a == "-h")
    {
        let name = resolve_prog_name(None);
        let cmd = build_cli_command(None, Some(name.clone()), false);
        let roff = apcore_cli::shell::build_program_man_page(
            &cmd,
            &name,
            env!("CARGO_PKG_VERSION"),
            None,
            None,
        );
        render_man_page(&roff);
        std::process::exit(0);
    }

    // Intercept --version before validating the extensions directory.
    // Clap exits 0 on --version; we just need to print and exit here.
    if raw_args.len() > 1 && raw_args[1..].iter().any(|a| a == "--version" || a == "-V") {
        let name = resolve_prog_name(None);
        println!("{}, version {}", name, env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    // Pre-parse --verbose before clap sees argv (must happen before
    // create_cli, since clap renders help during parsing).
    let verbose = has_verbose_flag(&raw_args);
    apcore_cli::cli::set_verbose_help(verbose);

    // Pre-parse --extensions-dir before clap sees argv.
    let extensions_dir = extract_extensions_dir(&raw_args[1..]);

    // Initialise tracing with default level (may be updated after parsing).
    let default_level = resolve_log_level(None);
    init_tracing(&default_level);

    // Build and parse CLI.
    let cmd = create_cli(extensions_dir, None);
    let matches = cmd.get_matches();

    // Optionally reload log filter from --log-level flag.
    if let Some(level) = matches.get_one::<String>("log-level") {
        if let Some(handle) = RELOAD_HANDLE.get() {
            let new_filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"));
            let _ = handle.reload(new_filter);
        }
    }

    // Build shared registry, executor, and apcore executor for dispatch.
    // Discover modules from the extensions directory when available.
    let extensions_dir_for_discovery = matches
        .get_one::<String>("extensions-dir")
        .cloned()
        .or_else(|| {
            std::env::var("APCORE_EXTENSIONS_ROOT")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "./extensions".to_string());

    // Propagate the resolved extensions root to APCORE_EXTENSIONS_ROOT so a
    // --internal-sandbox-runner child (spawned via Sandbox) can rebuild the
    // same FsDiscoverer under its SANDBOX_ALLOWED_ENV_PREFIXES allowlist.
    // Set once at startup, before any child spawn.
    // SAFETY: invoked before any thread spawns its own reads of this var.
    unsafe {
        std::env::set_var("APCORE_EXTENSIONS_ROOT", &extensions_dir_for_discovery);
    }

    // Discover modules once — share the single registry for both provider and executor.
    let registry = apcore::Registry::new();
    let discoverer = apcore_cli::FsDiscoverer::new(&extensions_dir_for_discovery);
    if let Err(e) = registry.discover(&discoverer).await {
        tracing::warn!("Module discovery failed: {e}");
    }
    // Collect discovered names from the registry after discovery.
    let discovered_names: Vec<String> = registry.list(None, None);

    // Store discovered executables in the global map for dispatch_module.
    apcore_cli::set_executables(discoverer.executables_snapshot());

    let descriptions = discoverer.load_descriptions();

    // Optional toolkit integration (requires --features toolkit)
    #[cfg(feature = "toolkit")]
    {
        let binding_path = extract_binding_path(&raw_args[1..]);
        let commands_dir = extract_commands_dir(&raw_args[1..]);

        if commands_dir.is_some() {
            tracing::warn!("Convention scanning not yet available in Rust toolkit");
        }

        if let Some(ref bp) = binding_path {
            let _resolver = apcore_toolkit::DisplayResolver::new();
            // DisplayResolver works on ScannedModule, not registry modules
            // directly. This will be fully wired when ConventionScanner
            // is available in the Rust toolkit.
            tracing::info!("Display overlay binding loaded from {}", bp);
        }
    }

    // Build the apcore executor from the discovered registry.
    let apcore_executor =
        apcore::Executor::new(std::sync::Arc::new(registry), apcore::Config::default());

    // Build the provider from a second registry for list/describe.
    // The filesystem scan is fast (local directory) and the discoverer
    // caches executable paths from the first scan.
    let provider_registry = apcore::Registry::new();
    if let Err(e) = provider_registry.discover(&discoverer).await {
        tracing::warn!("Provider registry discovery failed: {e}");
    }
    let mut provider = apcore_cli::discovery::ApCoreRegistryProvider::new(provider_registry);
    provider.set_discovered_names(discovered_names);
    provider.set_descriptions(descriptions);
    let registry_provider: std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider> =
        std::sync::Arc::new(provider);
    // ModuleExecutor trait + ApCoreExecutorAdapter were deleted per audit
    // D9-001..004. dispatch_module now takes the concrete apcore::Executor
    // directly via apcore_executor below.

    // Wire the audit logger for the production dispatch path. dispatch_module
    // emits one JSONL entry per invocation via AUDIT_LOGGER; without this
    // call every execution would be audit-silent (regression-prevented by
    // planning/security/plan.md §Data Flow). The default path
    // (~/.apcore-cli/audit.jsonl) is used; users can opt out by setting
    // APCORE_CLI_AUDIT_DISABLE=1.
    let audit_disabled = std::env::var("APCORE_CLI_AUDIT_DISABLE")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    if !audit_disabled {
        apcore_cli::cli::set_audit_logger(Some(apcore_cli::AuditLogger::new(None)));
    }

    let prog_name = resolve_prog_name(None);

    // Dispatch subcommands.
    //
    // Routing: FE-13 introduces the `apcli` subcommand group. Built-in
    // commands now live under `apcli <name>`; thin root-level shims for the
    // 13 former names print a deprecation warning and delegate to the same
    // handlers used by `apcli <name>`.
    match matches.subcommand() {
        // ----- FE-13 apcli group routing -----
        Some(("apcli", apcli_m)) => match apcli_m.subcommand() {
            Some(("list", sub_m)) => {
                handle_list(sub_m, &registry_provider);
            }
            Some(("describe", sub_m)) => {
                handle_describe(sub_m, &registry_provider);
            }
            Some(("exec", sub_m)) => {
                handle_exec(sub_m, &registry_provider, &apcore_executor).await;
            }
            Some(("validate", sub_m)) => {
                apcore_cli::validate::dispatch_validate(
                    sub_m,
                    &registry_provider,
                    &apcore_executor,
                )
                .await;
            }
            Some(("init", sub_m)) => {
                apcore_cli::init_cmd::handle_init(sub_m);
                std::process::exit(0);
            }
            Some(("health", sub_m)) => {
                apcore_cli::system_cmd::dispatch_health(sub_m, &apcore_executor);
            }
            Some(("usage", sub_m)) => {
                apcore_cli::system_cmd::dispatch_usage(sub_m, &apcore_executor);
            }
            Some(("enable", sub_m)) => {
                apcore_cli::system_cmd::dispatch_enable(sub_m, &apcore_executor);
            }
            Some(("disable", sub_m)) => {
                apcore_cli::system_cmd::dispatch_disable(sub_m, &apcore_executor);
            }
            Some(("reload", sub_m)) => {
                apcore_cli::system_cmd::dispatch_reload(sub_m, &apcore_executor);
            }
            Some(("config", sub_m)) => {
                apcore_cli::system_cmd::dispatch_config(sub_m, &apcore_executor);
            }
            Some(("completion", sub_m)) => {
                handle_completion(sub_m, &prog_name);
            }
            Some(("describe-pipeline", sub_m)) => {
                apcore_cli::strategy::dispatch_describe_pipeline(sub_m);
            }
            _ => {
                let _ = build_cli_command(None, Some(prog_name.clone()), false).print_help();
                println!();
                std::process::exit(0);
            }
        },
        // ----- FE-13 §11.2 deprecation shims (standalone only) -----
        Some(("list", _)) => {
            let sm = parse_shim_for("list", &raw_args, &prog_name);
            handle_list(&sm, &registry_provider);
        }
        Some(("describe", _)) => {
            let sm = parse_shim_for("describe", &raw_args, &prog_name);
            handle_describe(&sm, &registry_provider);
        }
        Some(("exec", _)) => {
            let sm = parse_shim_for("exec", &raw_args, &prog_name);
            handle_exec(&sm, &registry_provider, &apcore_executor).await;
        }
        Some(("init", _)) => {
            let sm = parse_shim_for("init", &raw_args, &prog_name);
            apcore_cli::init_cmd::handle_init(&sm);
            std::process::exit(0);
        }
        Some(("validate", _)) => {
            let sm = parse_shim_for("validate", &raw_args, &prog_name);
            apcore_cli::validate::dispatch_validate(&sm, &registry_provider, &apcore_executor)
                .await;
        }
        Some(("health", _)) => {
            let sm = parse_shim_for("health", &raw_args, &prog_name);
            apcore_cli::system_cmd::dispatch_health(&sm, &apcore_executor);
        }
        Some(("usage", _)) => {
            let sm = parse_shim_for("usage", &raw_args, &prog_name);
            apcore_cli::system_cmd::dispatch_usage(&sm, &apcore_executor);
        }
        Some(("enable", _)) => {
            let sm = parse_shim_for("enable", &raw_args, &prog_name);
            apcore_cli::system_cmd::dispatch_enable(&sm, &apcore_executor);
        }
        Some(("disable", _)) => {
            let sm = parse_shim_for("disable", &raw_args, &prog_name);
            apcore_cli::system_cmd::dispatch_disable(&sm, &apcore_executor);
        }
        Some(("reload", _)) => {
            let sm = parse_shim_for("reload", &raw_args, &prog_name);
            apcore_cli::system_cmd::dispatch_reload(&sm, &apcore_executor);
        }
        Some(("config", _)) => {
            let sm = parse_shim_for("config", &raw_args, &prog_name);
            apcore_cli::system_cmd::dispatch_config(&sm, &apcore_executor);
        }
        Some(("completion", _)) => {
            let sm = parse_shim_for("completion", &raw_args, &prog_name);
            handle_completion(&sm, &prog_name);
        }
        Some(("describe-pipeline", _)) => {
            let sm = parse_shim_for("describe-pipeline", &raw_args, &prog_name);
            apcore_cli::strategy::dispatch_describe_pipeline(&sm);
        }
        // ----- Root-level meta commands (stay at root per FE-13 §4.1) -----
        Some(("man", sub_m)) => {
            let command_name = sub_m
                .get_one::<String>("command")
                .expect("command is required");
            let cmd = build_cli_command(None, Some(prog_name.clone()), false);
            match apcore_cli::shell::cmd_man(
                command_name,
                &cmd,
                &prog_name,
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(output) => {
                    println!("{output}");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            }
        }
        // ----- External (business-module) subcommand -----
        Some((external, sub_m)) => {
            // External subcommand: re-parse trailing args through a command
            // that includes both built-in flags and schema-derived flags.
            let external = external.to_string();
            let trailing: Vec<String> = sub_m
                .get_many::<std::ffi::OsString>("")
                .into_iter()
                .flatten()
                .filter_map(|s| match s.to_str() {
                    Some(v) => Some(v.to_string()),
                    None => {
                        tracing::warn!("Dropping non-UTF8 argument: {:?}", s);
                        None
                    }
                })
                .collect();

            // Look up the module in the registry to get schema-derived flags.
            // If found, build the full command with --a, --b, etc. from input_schema.
            // If not found, use basic dispatch flags (dispatch_module will exit 44).
            let temp_cmd = match registry_provider.get_module_descriptor(&external) {
                Some(descriptor) => match apcore_cli::build_module_command(&descriptor) {
                    Ok(cmd) => cmd.no_binary_name(true),
                    Err(e) => {
                        // Surface schema-resolution failures with the
                        // protocol-spec exit code rather than degrading
                        // silently to an empty command (review #8).
                        eprintln!("Error: {e}");
                        std::process::exit(e.exit_code());
                    }
                },
                None => apcore_cli::cli::add_dispatch_flags(
                    clap::Command::new(&external).no_binary_name(true),
                ),
            };

            let ext_matches = temp_cmd
                .try_get_matches_from(&trailing)
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(2);
                });

            apcore_cli::cli::dispatch_module(
                &external,
                &ext_matches,
                &registry_provider,
                &apcore_executor,
            )
            .await;
        }
        None => {
            // No subcommand: print help.
            let _ = clap::Command::new(env!("CARGO_PKG_NAME")).print_help();
            println!();
            std::process::exit(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutex serializes tests that manipulate environment variables.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // --- extract_argv_option / extract_extensions_dir ---

    #[test]
    fn test_extract_extensions_dir_flag_space_form() {
        let args: Vec<String> = vec!["--extensions-dir".to_string(), "/tmp/ext".to_string()];
        assert_eq!(extract_extensions_dir(&args), Some("/tmp/ext".to_string()));
    }

    #[test]
    fn test_extract_extensions_dir_flag_equals_form() {
        let args: Vec<String> = vec!["--extensions-dir=/tmp/ext".to_string()];
        assert_eq!(extract_extensions_dir(&args), Some("/tmp/ext".to_string()));
    }

    #[test]
    fn test_extract_extensions_dir_missing_returns_none() {
        let args: Vec<String> = vec!["--log-level".to_string(), "DEBUG".to_string()];
        assert_eq!(extract_extensions_dir(&args), None);
    }

    #[test]
    fn test_extract_extensions_dir_empty_argv_returns_none() {
        assert_eq!(extract_extensions_dir(&[]), None);
    }

    #[test]
    fn test_extract_extensions_dir_partial_match_ignored() {
        // --extensions-dir-extra should NOT match.
        let args: Vec<String> = vec!["--extensions-dir-extra=/tmp/ext".to_string()];
        assert_eq!(extract_extensions_dir(&args), None);
    }

    #[test]
    fn test_extract_extensions_dir_flag_at_end_without_value() {
        // --extensions-dir with no following value should return None.
        let args: Vec<String> = vec!["--extensions-dir".to_string()];
        assert_eq!(extract_extensions_dir(&args), None);
    }

    // --- extract_commands_dir ---

    #[test]
    fn test_extract_commands_dir_space_form() {
        let args: Vec<String> = vec!["--commands-dir".to_string(), "/tmp/cmds".to_string()];
        assert_eq!(extract_commands_dir(&args), Some("/tmp/cmds".to_string()));
    }

    #[test]
    fn test_extract_commands_dir_equals_form() {
        let args: Vec<String> = vec!["--commands-dir=/tmp/cmds".to_string()];
        assert_eq!(extract_commands_dir(&args), Some("/tmp/cmds".to_string()));
    }

    #[test]
    fn test_extract_commands_dir_missing_returns_none() {
        assert_eq!(extract_commands_dir(&[]), None);
    }

    // --- extract_binding_path ---

    #[test]
    fn test_extract_binding_path_space_form() {
        let args: Vec<String> = vec!["--binding".to_string(), "binding.yaml".to_string()];
        assert_eq!(
            extract_binding_path(&args),
            Some("binding.yaml".to_string())
        );
    }

    #[test]
    fn test_extract_binding_path_equals_form() {
        let args: Vec<String> = vec!["--binding=binding.yaml".to_string()];
        assert_eq!(
            extract_binding_path(&args),
            Some("binding.yaml".to_string())
        );
    }

    #[test]
    fn test_extract_binding_path_missing_returns_none() {
        assert_eq!(extract_binding_path(&[]), None);
    }

    // --- extract_argv_option generic ---

    #[test]
    fn test_extract_argv_option_generic() {
        let args: Vec<String> = vec!["--foo".to_string(), "bar".to_string()];
        assert_eq!(extract_argv_option(&args, "--foo"), Some("bar".to_string()));
        assert_eq!(extract_argv_option(&args, "--baz"), None);
    }

    // --- resolve_log_level ---

    #[test]
    fn test_resolve_log_level_override_wins() {
        assert_eq!(resolve_log_level(Some("DEBUG")), "DEBUG");
    }

    #[test]
    fn test_resolve_log_level_no_override_returns_warn() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Without env vars set, should default to "warn".
        // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
        unsafe {
            std::env::remove_var("APCORE_CLI_LOGGING_LEVEL");
            std::env::remove_var("APCORE_LOGGING_LEVEL");
        }
        assert_eq!(resolve_log_level(None), "warn");
    }

    // --- LOG_LEVELS constant ---

    // --- validate_extensions_dir ---

    #[test]
    fn test_validate_extensions_dir_nonexistent_returns_err() {
        let result = validate_extensions_dir("/nonexistent/path/xxx");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_validate_extensions_dir_valid_returns_ok() {
        let dir = std::env::temp_dir();
        let result = validate_extensions_dir(dir.to_str().unwrap());
        assert!(result.is_ok());
    }

    // --- exec subcommand ---

    #[test]
    fn test_exec_subcommand_exists() {
        let cmd = build_cli_command(None, None, false);
        let exec = cmd.get_subcommands().find(|c| c.get_name() == "exec");
        assert!(
            exec.is_some(),
            "build_cli_command must include 'exec' subcommand"
        );
    }

    #[test]
    fn test_exec_subcommand_has_required_module_id() {
        // FE-13: `exec` with its full arg surface lives under `apcli`.
        let cmd = build_cli_command(None, None, false);
        let apcli = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "apcli")
            .expect("apcli group must exist");
        let exec = apcli
            .get_subcommands()
            .find(|c| c.get_name() == "exec")
            .expect("apcli exec subcommand must exist");
        let module_id = exec.get_arguments().find(|a| a.get_id() == "module_id");
        assert!(module_id.is_some(), "exec must have a 'module_id' argument");
        assert!(
            module_id.unwrap().is_required_set(),
            "module_id must be required"
        );
    }

    #[test]
    fn test_exec_subcommand_has_optional_flags() {
        // FE-13: `exec` with its full arg surface lives under `apcli`.
        let cmd = build_cli_command(None, None, false);
        let apcli = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "apcli")
            .expect("apcli group must exist");
        let exec = apcli
            .get_subcommands()
            .find(|c| c.get_name() == "exec")
            .expect("apcli exec subcommand must exist");

        let arg_names: Vec<&str> = exec.get_arguments().map(|a| a.get_id().as_str()).collect();

        assert!(arg_names.contains(&"input"), "exec must have --input flag");
        assert!(arg_names.contains(&"yes"), "exec must have --yes flag");
        assert!(
            arg_names.contains(&"large-input"),
            "exec must have --large-input flag"
        );
        assert!(
            arg_names.contains(&"format"),
            "exec must have --format flag"
        );
    }

    #[test]
    fn test_exec_subcommand_parses_valid_args() {
        // FE-13: canonical path is now `apcli exec <module_id>`.
        let cmd = build_cli_command(None, None, false);
        let matches = cmd.try_get_matches_from(vec![
            "apcore-cli",
            "apcli",
            "exec",
            "my.module",
            "--yes",
            "--format",
            "json",
        ]);
        assert!(
            matches.is_ok(),
            "apcli exec with valid args must parse successfully"
        );
        let m = matches.unwrap();
        let apcli_m = m.subcommand_matches("apcli").unwrap();
        let sub = apcli_m.subcommand_matches("exec").unwrap();
        assert_eq!(
            sub.get_one::<String>("module_id").map(|s| s.as_str()),
            Some("my.module")
        );
        assert!(sub.get_flag("yes"));
        assert_eq!(
            sub.get_one::<String>("format").map(|s| s.as_str()),
            Some("json")
        );
    }

    // --- LOG_LEVELS constant ---

    #[test]
    fn test_log_levels_constant_has_expected_values() {
        assert!(LOG_LEVELS.contains(&"DEBUG"));
        assert!(LOG_LEVELS.contains(&"INFO"));
        assert!(LOG_LEVELS.contains(&"WARNING"));
        assert!(LOG_LEVELS.contains(&"ERROR"));
    }
}
