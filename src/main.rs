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
/// Prints an error to stderr and calls `std::process::exit(47)` on failure.
fn validate_extensions_dir(ext_dir: &str) {
    let path = Path::new(ext_dir);
    if !path.exists() {
        eprintln!(
            "Error: Extensions directory not found: '{ext_dir}'. \
             Set APCORE_EXTENSIONS_ROOT or verify the path."
        );
        std::process::exit(EXIT_CONFIG_NOT_FOUND);
    }
    if std::fs::read_dir(path).is_err() {
        eprintln!(
            "Error: Cannot read extensions directory: '{ext_dir}'. Check file permissions."
        );
        std::process::exit(EXIT_CONFIG_NOT_FOUND);
    }
}

// ---------------------------------------------------------------------------
// create_cli
// ---------------------------------------------------------------------------

/// Build the root `clap::Command` tree.
///
/// When `validate` is true, exits 47 if `extensions_dir` does not exist.
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
        validate_extensions_dir(&ext_dir);
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
    // The registry is only needed at dispatch time (cmd_list / cmd_describe), not
    // during clap command registration, so a placeholder is used here.
    let placeholder_registry: std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider> =
        std::sync::Arc::new(apcore_cli::discovery::MockRegistry::new(vec![]));
    cmd = apcore_cli::discovery::register_discovery_commands(cmd, placeholder_registry);
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

    // Build a real registry from the extensions directory for dispatch.
    let registry = apcore::Registry::new();
    let registry_provider: std::sync::Arc<dyn apcore_cli::discovery::RegistryProvider> =
        std::sync::Arc::new(apcore_cli::discovery::ApCoreRegistryProvider::new(registry));

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
        Some((_external, _sub_m)) => {
            // Dynamic module dispatch for unrecognised subcommands.
            eprintln!("Error: Unknown subcommand. Use --help for usage.");
            std::process::exit(apcore_cli::EXIT_MODULE_NOT_FOUND);
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
        // Without env vars set, should default to "warn".
        unsafe {
            std::env::remove_var("APCORE_CLI_LOGGING_LEVEL");
            std::env::remove_var("APCORE_LOGGING_LEVEL");
        }
        assert_eq!(resolve_log_level(None), "warn");
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
