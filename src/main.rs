// apcore-cli — Binary entry point.
// Protocol spec: FE-01 (create_cli, main, extract_extensions_dir, init_tracing)

use std::path::Path;
use std::sync::OnceLock;

use apcore_cli::EXIT_CONFIG_NOT_FOUND;
use tracing_subscriber::{reload, EnvFilter};
use tracing_subscriber::prelude::*;

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

/// Pre-parse `--extensions-dir` from raw argv before clap processes arguments.
///
/// Scans argv linearly — no clap involvement. Mirrors Python's
/// `_extract_extensions_dir`. Handles both `--extensions-dir VALUE` and
/// `--extensions-dir=VALUE` forms.
pub fn extract_extensions_dir(args: &[String]) -> Option<String> {
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--extensions-dir" {
            return iter.next().cloned();
        }
        if let Some(val) = arg.strip_prefix("--extensions-dir=") {
            return Some(val.to_string());
        }
    }
    None
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
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false),
        )
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
fn build_cli_command(extensions_dir: Option<String>, prog_name: Option<String>, validate: bool) -> clap::Command {
    let name = resolve_prog_name(prog_name);

    // Resolve extensions_dir: flag > env var > default.
    let ext_dir = match extensions_dir {
        Some(dir) => dir,
        None => {
            std::env::var("APCORE_EXTENSIONS_ROOT")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "./extensions".to_string())
        }
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
        .allow_external_subcommands(true)
        .arg(
            clap::Arg::new("extensions-dir")
                .long("extensions-dir")
                .global(true)
                .value_name("PATH")
                .help("Path to apcore extensions directory."),
        )
        .arg(
            clap::Arg::new("log-level")
                .long("log-level")
                .global(true)
                .value_parser(clap::builder::PossibleValuesParser::new(LOG_LEVELS))
                .ignore_case(true)
                .help("Log verbosity (DEBUG|INFO|WARNING|ERROR)."),
        );

    // Register built-in subcommands from discovery and shell modules.
    // The registry parameter is unused during clap command registration —
    // it is only needed at dispatch time for cmd_list / cmd_describe.
    let placeholder: std::sync::Arc<dyn apcore_cli::RegistryProvider> =
        std::sync::Arc::new(apcore_cli::ApCoreRegistryProvider::new(apcore::Registry::new()));
    cmd = cmd.subcommand(apcore_cli::cli::exec_command());
    cmd = apcore_cli::discovery::register_discovery_commands(cmd, placeholder);
    cmd = apcore_cli::shell::register_shell_commands(cmd, &name);

    cmd
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
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Intercept --internal-sandbox-runner before clap processes argv.
    // This must happen first so clap does not reject the unknown flag.
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.get(1).map(String::as_str) == Some("--internal-sandbox-runner") {
        if let Err(e) = apcore_cli::_sandbox_runner::run_sandbox_subprocess().await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    // Intercept --version before validating the extensions directory.
    // Clap exits 0 on --version; we just need to print and exit here.
    if raw_args.len() > 1 && raw_args[1..].iter().any(|a| a == "--version" || a == "-V") {
        let name = resolve_prog_name(None);
        println!("{}, version {}", name, env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

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
    let registry = apcore::Registry::new();
    let registry_provider: std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider> =
        std::sync::Arc::new(apcore_cli::discovery::ApCoreRegistryProvider::new(registry));
    let executor: std::sync::Arc<dyn apcore_cli::ModuleExecutor> =
        std::sync::Arc::new(apcore_cli::cli::ApCoreExecutorAdapter(
            apcore::Executor::new(apcore::Registry::new(), apcore::Config::default()),
        ));
    let apcore_executor = apcore::Executor::new(apcore::Registry::new(), apcore::Config::default());

    let prog_name = resolve_prog_name(None);

    // Dispatch subcommands.
    match matches.subcommand() {
        Some(("list", sub_m)) => {
            let tags: Vec<&str> = sub_m
                .get_many::<String>("tag")
                .map(|vals| vals.map(|s| s.as_str()).collect())
                .unwrap_or_default();
            let format = sub_m.get_one::<String>("format").map(|s| s.as_str());
            match apcore_cli::discovery::cmd_list(registry_provider.as_ref(), &tags, format) {
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
        Some(("describe", sub_m)) => {
            let module_id = sub_m.get_one::<String>("module_id").expect("module_id is required");
            let format = sub_m.get_one::<String>("format").map(|s| s.as_str());
            match apcore_cli::discovery::cmd_describe(registry_provider.as_ref(), module_id, format) {
                Ok(output) => {
                    println!("{output}");
                    std::process::exit(0);
                }
                Err(apcore_cli::discovery::DiscoveryError::ModuleNotFound(_)) => {
                    eprintln!("Error: {}", apcore_cli::discovery::DiscoveryError::ModuleNotFound(module_id.clone()));
                    std::process::exit(apcore_cli::EXIT_MODULE_NOT_FOUND);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            }
        }
        Some(("completion", sub_m)) => {
            let shell = *sub_m.get_one::<clap_complete::Shell>("shell").expect("shell is required");
            let mut cmd = build_cli_command(None, Some(prog_name.clone()), false);
            let output = apcore_cli::shell::cmd_completion(shell, &prog_name, &mut cmd);
            print!("{output}");
            std::process::exit(0);
        }
        Some(("man", sub_m)) => {
            let command_name = sub_m
                .get_one::<String>("command")
                .expect("command is required");
            let cmd = build_cli_command(None, Some(prog_name.clone()), false);
            match apcore_cli::shell::cmd_man(command_name, &cmd, &prog_name, env!("CARGO_PKG_VERSION")) {
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
        Some(("exec", sub_m)) => {
            let module_id = sub_m.get_one::<String>("module_id")
                .expect("module_id is required");
            apcore_cli::cli::dispatch_module(
                module_id, sub_m, &registry_provider, &executor, &apcore_executor,
            ).await;
        }
        Some((external, sub_m)) => {
            // External subcommand: re-parse trailing args through a temporary
            // command so that dispatch_module can access built-in flags like
            // --yes, --input, --format, --sandbox, etc.
            let external = external.to_string();
            let trailing: Vec<String> = sub_m
                .get_many::<std::ffi::OsString>("")
                .into_iter()
                .flatten()
                .filter_map(|s| {
                    match s.to_str() {
                        Some(v) => Some(v.to_string()),
                        None => {
                            tracing::warn!("Dropping non-UTF8 argument: {:?}", s);
                            None
                        }
                    }
                })
                .collect();

            // Reuse the shared dispatch flags via add_dispatch_flags.
            let temp_cmd = apcore_cli::cli::add_dispatch_flags(
                clap::Command::new(&external).no_binary_name(true),
            );

            let ext_matches = temp_cmd.try_get_matches_from(&trailing)
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(2);
                });

            apcore_cli::cli::dispatch_module(
                &external, &ext_matches, &registry_provider, &executor, &apcore_executor,
            ).await;
        }
        None => {
            // No subcommand: print help.
            let _ = clap::Command::new(env!("CARGO_PKG_NAME"))
                .print_help();
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

    // --- extract_extensions_dir ---

    #[test]
    fn test_extract_extensions_dir_flag_space_form() {
        let args: Vec<String> = vec![
            "--extensions-dir".to_string(),
            "/tmp/ext".to_string(),
        ];
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
        let exec = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "exec");
        assert!(exec.is_some(), "build_cli_command must include 'exec' subcommand");
    }

    #[test]
    fn test_exec_subcommand_has_required_module_id() {
        let cmd = build_cli_command(None, None, false);
        let exec = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "exec")
            .expect("exec subcommand must exist");
        let module_id = exec.get_arguments().find(|a| a.get_id() == "module_id");
        assert!(module_id.is_some(), "exec must have a 'module_id' argument");
        assert!(module_id.unwrap().is_required_set(), "module_id must be required");
    }

    #[test]
    fn test_exec_subcommand_has_optional_flags() {
        let cmd = build_cli_command(None, None, false);
        let exec = cmd
            .get_subcommands()
            .find(|c| c.get_name() == "exec")
            .expect("exec subcommand must exist");

        let arg_names: Vec<&str> = exec
            .get_arguments()
            .map(|a| a.get_id().as_str())
            .collect();

        assert!(arg_names.contains(&"input"), "exec must have --input flag");
        assert!(arg_names.contains(&"yes"), "exec must have --yes flag");
        assert!(arg_names.contains(&"large-input"), "exec must have --large-input flag");
        assert!(arg_names.contains(&"format"), "exec must have --format flag");
    }

    #[test]
    fn test_exec_subcommand_parses_valid_args() {
        let cmd = build_cli_command(None, None, false);
        let matches = cmd.try_get_matches_from(vec![
            "apcore-cli", "exec", "my.module", "--yes", "--format", "json",
        ]);
        assert!(matches.is_ok(), "exec with valid args must parse successfully");
        let m = matches.unwrap();
        let sub = m.subcommand_matches("exec").unwrap();
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
