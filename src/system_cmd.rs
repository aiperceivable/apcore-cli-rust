// apcore-cli -- System management commands (FE-11).
// Delegates to system.* modules via executor. Graceful no-op if unavailable.

use clap::{Arg, ArgAction, Command};
use serde_json::Value;
use std::io::IsTerminal;

/// Attach the `health` subcommand to the given command. Returns the command
/// with the subcommand added.
pub(crate) fn register_health_command(cli: Command) -> Command {
    cli.subcommand(health_command())
}

/// Attach the `usage` subcommand to the given command. Returns the command
/// with the subcommand added.
pub(crate) fn register_usage_command(cli: Command) -> Command {
    cli.subcommand(usage_command())
}

/// Attach the `enable` subcommand to the given command. Returns the command
/// with the subcommand added.
pub(crate) fn register_enable_command(cli: Command) -> Command {
    cli.subcommand(enable_command())
}

/// Attach the `disable` subcommand to the given command. Returns the command
/// with the subcommand added.
pub(crate) fn register_disable_command(cli: Command) -> Command {
    cli.subcommand(disable_command())
}

/// Attach the `reload` subcommand to the given command. Returns the command
/// with the subcommand added.
pub(crate) fn register_reload_command(cli: Command) -> Command {
    cli.subcommand(reload_command())
}

/// Attach the `config` subcommand group to the given command. Returns the
/// command with the subcommand added.
pub(crate) fn register_config_command(cli: Command) -> Command {
    cli.subcommand(config_command())
}

/// Names of all system management subcommands.
pub const SYSTEM_COMMANDS: &[&str] = &["config", "disable", "enable", "health", "reload", "usage"];

// ---------------------------------------------------------------------------
// Command builders
// ---------------------------------------------------------------------------

fn health_command() -> Command {
    Command::new("health")
        .about("Show module health status")
        .arg(
            Arg::new("module_id")
                .value_name("MODULE_ID")
                .help("Module ID for per-module detail (omit for summary)."),
        )
        .arg(
            Arg::new("threshold")
                .long("threshold")
                .value_name("RATE")
                .default_value("0.01")
                .help("Error rate threshold (default: 0.01)."),
        )
        .arg(
            Arg::new("all")
                .long("all")
                .action(ArgAction::SetTrue)
                .help("Include healthy modules."),
        )
        .arg(
            Arg::new("errors")
                .long("errors")
                .value_name("N")
                .default_value("10")
                .help("Max recent errors (module detail only)."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

fn usage_command() -> Command {
    Command::new("usage")
        .about("Show module usage statistics")
        .arg(
            Arg::new("module_id")
                .value_name("MODULE_ID")
                .help("Module ID for per-module detail (omit for summary)."),
        )
        .arg(
            Arg::new("period")
                .long("period")
                .value_name("WINDOW")
                .default_value("24h")
                .help("Time window: 1h, 24h, 7d, 30d (default: 24h)."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

fn enable_command() -> Command {
    Command::new("enable")
        .about("Enable a disabled module at runtime")
        .arg(
            Arg::new("module_id")
                .required(true)
                .value_name("MODULE_ID")
                .help("Module to enable."),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .required(true)
                .value_name("TEXT")
                .help("Reason for enabling (required for audit)."),
        )
        .arg(
            Arg::new("yes")
                .long("yes")
                .short('y')
                .action(ArgAction::SetTrue)
                .help("Skip approval prompt."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

fn disable_command() -> Command {
    Command::new("disable")
        .about("Disable a module at runtime")
        .arg(
            Arg::new("module_id")
                .required(true)
                .value_name("MODULE_ID")
                .help("Module to disable."),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .required(true)
                .value_name("TEXT")
                .help("Reason for disabling (required for audit)."),
        )
        .arg(
            Arg::new("yes")
                .long("yes")
                .short('y')
                .action(ArgAction::SetTrue)
                .help("Skip approval prompt."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

fn reload_command() -> Command {
    Command::new("reload")
        .about("Hot-reload a module from disk")
        .arg(
            Arg::new("module_id")
                .required(true)
                .value_name("MODULE_ID")
                .help("Module to reload."),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .required(true)
                .value_name("TEXT")
                .help("Reason for reload (required for audit)."),
        )
        .arg(
            Arg::new("yes")
                .long("yes")
                .short('y')
                .action(ArgAction::SetTrue)
                .help("Skip approval prompt."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

fn config_command() -> Command {
    Command::new("config")
        .about("Read or update runtime configuration")
        .subcommand(
            Command::new("get")
                .about("Read a configuration value by dot-path key")
                .arg(
                    Arg::new("key")
                        .required(true)
                        .value_name("KEY")
                        .help("Dot-path configuration key."),
                ),
        )
        .subcommand(
            Command::new("set")
                .about("Update a runtime configuration value")
                .arg(
                    Arg::new("key")
                        .required(true)
                        .value_name("KEY")
                        .help("Dot-path configuration key."),
                )
                .arg(
                    Arg::new("value")
                        .required(true)
                        .value_name("VALUE")
                        .help("New value (JSON or plain string)."),
                )
                .arg(
                    Arg::new("reason")
                        .long("reason")
                        .required(true)
                        .value_name("TEXT")
                        .help("Reason for config change (required for audit)."),
                )
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .short('y')
                        .help("Bypass approval prompt for this config change.")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["table", "json"])
                        .value_name("FORMAT")
                        .help("Output format."),
                ),
        )
}

// ---------------------------------------------------------------------------
// Dispatch helpers
// ---------------------------------------------------------------------------

/// Error returned by `call_system_module`. Preserves the
/// `apcore::errors::ModuleError` variant so callers can route it through
/// `crate::cli::map_module_error_to_exit_code` and emit the protocol-spec
/// exit code consistently with `cli::dispatch_module` (no more
/// `exit(1)`-collapse divergence between the two dispatch paths).
enum SystemDispatchError {
    ModuleError(Box<apcore::errors::ModuleError>),
    NoAsyncRuntime,
}

impl std::fmt::Display for SystemDispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SystemDispatchError::ModuleError(e) => write!(f, "{e}"),
            SystemDispatchError::NoAsyncRuntime => write!(f, "no async runtime available"),
        }
    }
}

/// Run the approval gate for a system command. Mirrors the `cli.rs:1086`
/// pattern used by the main `exec` dispatcher so `--yes`, the
/// `APCORE_CLI_AUTO_APPROVE` env var, TTY interactivity, and the
/// 60-second timed prompt all behave consistently across `apcli enable /
/// disable / reload / config set`. All `system.control.*` and
/// `system.config.set` calls are operator-confirmation-required by spec
/// (FR-SYSCMD-013); we synthesize a minimal module_def so the standard
/// approval helper can do its job.
///
/// Exits 46 on denial / timeout / non-interactive — matching the main
/// exec dispatcher's exit-code contract.
pub(crate) fn require_approval_for_system_command(module_id: &str, auto_approve: bool) {
    let module_def = serde_json::json!({
        "module_id": module_id,
        "annotations": { "requires_approval": true },
    });
    let result = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| {
            handle.block_on(crate::approval::check_approval(&module_def, auto_approve))
        }),
        Err(_) => {
            eprintln!("Error: no async runtime available for approval check");
            std::process::exit(crate::EXIT_MODULE_EXECUTE_ERROR);
        }
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(crate::EXIT_APPROVAL_DENIED);
    }
}

/// Call a system module via the executor, returning the result or a typed
/// error. Preserves the `apcore::errors::ModuleError` variant instead of
/// stringifying at the boundary.
fn call_system_module(
    executor: &apcore::Executor,
    module_id: &str,
    inputs: Value,
) -> Result<Value, SystemDispatchError> {
    let rt = tokio::runtime::Handle::try_current();
    match rt {
        Ok(handle) => {
            // We are inside a tokio runtime -- use block_in_place.
            tokio::task::block_in_place(|| {
                handle
                    .block_on(executor.call(module_id, inputs, None, None))
                    .map_err(|e| SystemDispatchError::ModuleError(Box::new(e)))
            })
        }
        Err(_) => Err(SystemDispatchError::NoAsyncRuntime),
    }
}

/// Exit the process with the protocol-spec code for a
/// `SystemDispatchError`. Centralises the common error tail used by every
/// system_cmd dispatcher so all seven exit sites stay consistent.
fn exit_on_system_error(err: SystemDispatchError) -> ! {
    eprintln!("Error: {err}");
    let code = match err {
        SystemDispatchError::ModuleError(e) => {
            crate::cli::map_module_error_to_exit_code(e.as_ref())
        }
        SystemDispatchError::NoAsyncRuntime => crate::EXIT_MODULE_EXECUTE_ERROR,
    };
    std::process::exit(code);
}

/// Dispatch the `health` subcommand.
pub fn dispatch_health(matches: &clap::ArgMatches, executor: &apcore::Executor) {
    let module_id = matches.get_one::<String>("module_id");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());
    let fmt = crate::output::resolve_format(format);

    let result = if let Some(mid) = module_id {
        let errors: i64 = matches
            .get_one::<String>("errors")
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        call_system_module(
            executor,
            "system.health.module",
            serde_json::json!({"module_id": mid, "error_limit": errors}),
        )
    } else {
        let threshold: f64 = matches
            .get_one::<String>("threshold")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.01);
        let include_all = matches.get_flag("all");
        call_system_module(
            executor,
            "system.health.summary",
            serde_json::json!({
                "error_rate_threshold": threshold,
                "include_healthy": include_all,
            }),
        )
    };

    match result {
        Ok(val) => {
            if fmt == "json" || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
                );
            } else if module_id.is_some() {
                format_health_module_tty(&val);
            } else {
                format_health_summary_tty(&val);
            }
            std::process::exit(0);
        }
        Err(e) => exit_on_system_error(e),
    }
}

/// Dispatch the `usage` subcommand.
pub fn dispatch_usage(matches: &clap::ArgMatches, executor: &apcore::Executor) {
    let module_id = matches.get_one::<String>("module_id");
    let period = matches
        .get_one::<String>("period")
        .map(|s| s.as_str())
        .unwrap_or("24h");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());
    let fmt = crate::output::resolve_format(format);

    let result = if let Some(mid) = module_id {
        call_system_module(
            executor,
            "system.usage.module",
            serde_json::json!({"module_id": mid, "period": period}),
        )
    } else {
        call_system_module(
            executor,
            "system.usage.summary",
            serde_json::json!({"period": period}),
        )
    };

    match result {
        Ok(val) => {
            if fmt == "json" || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
                );
            } else if module_id.is_some() {
                println!("{}", crate::output::format_exec_result(&val, "table", None));
            } else {
                format_usage_summary_tty(&val);
            }
            std::process::exit(0);
        }
        Err(e) => exit_on_system_error(e),
    }
}

/// Dispatch the `enable` subcommand.
pub fn dispatch_enable(matches: &clap::ArgMatches, executor: &apcore::Executor) {
    let module_id = matches
        .get_one::<String>("module_id")
        .expect("module_id is required");
    let reason = matches
        .get_one::<String>("reason")
        .expect("reason is required");
    let auto_approve = matches.get_flag("yes");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());
    let fmt = crate::output::resolve_format(format);

    require_approval_for_system_command("system.control.toggle_feature", auto_approve);
    let result = call_system_module(
        executor,
        "system.control.toggle_feature",
        serde_json::json!({
            "module_id": module_id,
            "enabled": true,
            "reason": reason,
        }),
    );

    match result {
        Ok(val) => {
            if fmt == "json" || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                println!("Module '{module_id}' enabled.");
                println!("  Reason: {reason}");
            }
            std::process::exit(0);
        }
        Err(e) => exit_on_system_error(e),
    }
}

/// Dispatch the `disable` subcommand.
pub fn dispatch_disable(matches: &clap::ArgMatches, executor: &apcore::Executor) {
    let module_id = matches
        .get_one::<String>("module_id")
        .expect("module_id is required");
    let auto_approve = matches.get_flag("yes");
    let reason = matches
        .get_one::<String>("reason")
        .expect("reason is required");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());
    let fmt = crate::output::resolve_format(format);

    require_approval_for_system_command("system.control.toggle_feature", auto_approve);
    let result = call_system_module(
        executor,
        "system.control.toggle_feature",
        serde_json::json!({
            "module_id": module_id,
            "enabled": false,
            "reason": reason,
        }),
    );

    match result {
        Ok(val) => {
            if fmt == "json" || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                println!("Module '{module_id}' disabled.");
                println!("  Reason: {reason}");
            }
            std::process::exit(0);
        }
        Err(e) => exit_on_system_error(e),
    }
}

/// Dispatch the `reload` subcommand.
pub fn dispatch_reload(matches: &clap::ArgMatches, executor: &apcore::Executor) {
    let module_id = matches
        .get_one::<String>("module_id")
        .expect("module_id is required");
    let auto_approve = matches.get_flag("yes");
    let reason = matches
        .get_one::<String>("reason")
        .expect("reason is required");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());
    let fmt = crate::output::resolve_format(format);

    require_approval_for_system_command("system.control.reload_module", auto_approve);
    let result = call_system_module(
        executor,
        "system.control.reload_module",
        serde_json::json!({"module_id": module_id, "reason": reason}),
    );

    match result {
        Ok(val) => {
            if fmt == "json" || !std::io::stdout().is_terminal() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                let prev = val
                    .get("previous_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let new = val
                    .get("new_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let dur = val
                    .get("reload_duration_ms")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!("Module '{module_id}' reloaded.");
                println!("  Version: {prev} -> {new}");
                println!("  Duration: {dur}ms");
            }
            std::process::exit(0);
        }
        Err(e) => exit_on_system_error(e),
    }
}

/// Dispatch the `config` subcommand group.
pub fn dispatch_config(matches: &clap::ArgMatches, executor: &apcore::Executor) {
    match matches.subcommand() {
        Some(("get", sub_m)) => {
            let key = sub_m.get_one::<String>("key").expect("key is required");
            // Try reading from apcore Config directly.
            match call_system_module(
                executor,
                "system.config.get",
                serde_json::json!({"key": key}),
            ) {
                Ok(val) => {
                    let display = val
                        .get("value")
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| val.to_string());
                    println!("{key} = {display}");
                    std::process::exit(0);
                }
                Err(e) => exit_on_system_error(e),
            }
        }
        Some(("set", sub_m)) => {
            let key = sub_m.get_one::<String>("key").expect("key is required");
            let raw_value = sub_m.get_one::<String>("value").expect("value is required");
            let reason = sub_m
                .get_one::<String>("reason")
                .expect("reason is required");
            let auto_approve = sub_m.get_flag("yes");
            let format = sub_m.get_one::<String>("format").map(|s| s.as_str());
            let fmt = crate::output::resolve_format(format);

            // Parse value as JSON; fall back to plain string.
            let parsed: Value = serde_json::from_str(raw_value)
                .unwrap_or_else(|_| Value::String(raw_value.clone()));

            require_approval_for_system_command("system.control.update_config", auto_approve);
            let result = call_system_module(
                executor,
                "system.control.update_config",
                serde_json::json!({
                    "key": key,
                    "value": parsed,
                    "reason": reason,
                }),
            );

            match result {
                Ok(val) => {
                    if fmt == "json" || !std::io::stdout().is_terminal() {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
                        );
                    } else {
                        let old = val
                            .get("old_value")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "?".to_string());
                        let new = val
                            .get("new_value")
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "?".to_string());
                        println!("Config updated: {key}");
                        println!("  {old} -> {new}");
                        println!("  Reason: {reason}");
                    }
                    std::process::exit(0);
                }
                Err(e) => exit_on_system_error(e),
            }
        }
        _ => {
            eprintln!("Error: config requires a subcommand (get or set).");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// TTY formatting helpers
// ---------------------------------------------------------------------------

fn format_health_summary_tty(result: &Value) {
    let modules = result
        .get("modules")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let summary = result.get("summary").cloned().unwrap_or(Value::Null);

    if modules.is_empty() {
        println!("No modules found.");
        return;
    }

    let total = summary
        .get("total_modules")
        .and_then(|v| v.as_u64())
        .unwrap_or(modules.len() as u64);

    println!("Health Overview ({total} modules)\n");
    println!(
        "  {:<28} {:<12} {:<12} Top Error",
        "Module", "Status", "Error Rate"
    );
    println!("  {:-<28} {:-<12} {:-<12} {:-<20}", "", "", "", "");
    for m in &modules {
        let mid = m.get("module_id").and_then(|v| v.as_str()).unwrap_or("?");
        let status = m.get("status").and_then(|v| v.as_str()).unwrap_or("?");
        let rate = m
            .get("error_rate")
            .and_then(|v| v.as_f64())
            .map(|r| format!("{:.1}%", r * 100.0))
            .unwrap_or_else(|| "0.0%".to_string());
        let top = m.get("top_error");
        let top_str = match top {
            Some(t) if !t.is_null() => {
                let code = t.get("code").and_then(|v| v.as_str()).unwrap_or("?");
                let count = t
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string());
                format!("{code} ({count})")
            }
            _ => "--".to_string(),
        };
        println!("  {mid:<28} {status:<12} {rate:<12} {top_str}");
    }

    let mut parts = Vec::new();
    for key in ["healthy", "degraded", "error"] {
        if let Some(count) = summary.get(key).and_then(|v| v.as_u64()) {
            if count > 0 {
                parts.push(format!("{count} {key}"));
            }
        }
    }
    let summary_str = if parts.is_empty() {
        "no data".to_string()
    } else {
        parts.join(", ")
    };
    println!("\nSummary: {summary_str}");
}

fn format_health_module_tty(result: &Value) {
    let mid = result
        .get("module_id")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let total = result
        .get("total_calls")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let errors = result
        .get("error_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let rate = result
        .get("error_rate")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let avg = result
        .get("avg_latency_ms")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let p99 = result
        .get("p99_latency_ms")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    println!("Module: {mid}");
    println!("Status: {status}");
    println!(
        "Calls: {total} total | {errors} errors | {:.1}% error rate",
        rate * 100.0
    );
    println!("Latency: {avg:.0}ms avg | {p99:.0}ms p99");

    if let Some(recent) = result.get("recent_errors").and_then(|v| v.as_array()) {
        if !recent.is_empty() {
            println!("\nRecent Errors (top {}):", recent.len());
            for e in recent {
                let code = e.get("code").and_then(|v| v.as_str()).unwrap_or("?");
                let count = e
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let last = e
                    .get("last_occurred")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("  {code:<24} x{count}  (last: {last})");
            }
        }
    }
}

fn format_usage_summary_tty(result: &Value) {
    let modules = result
        .get("modules")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let period = result.get("period").and_then(|v| v.as_str()).unwrap_or("?");

    if modules.is_empty() {
        println!("No usage data for period {period}.");
        return;
    }

    println!("Usage Summary (last {period})\n");
    println!(
        "  {:<24} {:>8} {:>8} {:>12} {:<10}",
        "Module", "Calls", "Errors", "Avg Latency", "Trend"
    );
    println!(
        "  {:-<24} {:-<8} {:-<8} {:-<12} {:-<10}",
        "", "", "", "", ""
    );
    for m in &modules {
        let mid = m.get("module_id").and_then(|v| v.as_str()).unwrap_or("?");
        let calls = m.get("call_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let errs = m.get("error_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let avg = m
            .get("avg_latency_ms")
            .and_then(|v| v.as_f64())
            .map(|v| format!("{v:.0}ms"))
            .unwrap_or_else(|| "0ms".to_string());
        let trend = m.get("trend").and_then(|v| v.as_str()).unwrap_or("");
        println!("  {mid:<24} {calls:>8} {errs:>8} {avg:>12} {trend:>10}");
    }

    let total_calls: u64 = result
        .get("total_calls")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| {
            modules
                .iter()
                .filter_map(|m| m.get("call_count").and_then(|v| v.as_u64()))
                .sum()
        });
    let total_errors: u64 = result
        .get("total_errors")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| {
            modules
                .iter()
                .filter_map(|m| m.get("error_count").and_then(|v| v.as_u64()))
                .sum()
        });
    println!("\nTotal: {total_calls} calls | {total_errors} errors");
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_commands_constant() {
        assert!(SYSTEM_COMMANDS.contains(&"health"));
        assert!(SYSTEM_COMMANDS.contains(&"usage"));
        assert!(SYSTEM_COMMANDS.contains(&"enable"));
        assert!(SYSTEM_COMMANDS.contains(&"disable"));
        assert!(SYSTEM_COMMANDS.contains(&"reload"));
        assert!(SYSTEM_COMMANDS.contains(&"config"));
    }

    #[test]
    fn test_health_command_builder() {
        let cmd = health_command();
        assert_eq!(cmd.get_name(), "health");
        let args: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert!(args.contains(&"module_id"));
        assert!(args.contains(&"threshold"));
        assert!(args.contains(&"all"));
    }

    #[test]
    fn test_usage_command_builder() {
        let cmd = usage_command();
        assert_eq!(cmd.get_name(), "usage");
        let opts: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(opts.contains(&"period"));
    }

    #[test]
    fn test_enable_command_builder() {
        let cmd = enable_command();
        assert_eq!(cmd.get_name(), "enable");
        let opts: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(opts.contains(&"reason"));
        assert!(opts.contains(&"yes"));
    }

    #[test]
    fn test_config_command_has_subcommands() {
        let cmd = config_command();
        assert_eq!(cmd.get_name(), "config");
        let subs: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"get"));
        assert!(subs.contains(&"set"));
    }

    #[test]
    fn test_per_subcommand_registrars_cover_all_system_commands() {
        // Replaces the old test_register_system_commands_adds_all assertion
        // — the deprecated `register_system_commands` wrapper was removed
        // (review #28: zero production callers, FE-13 dispatch goes through
        // the per-subcommand registrars table in lib.rs::register_apcli_subcommands).
        let root = Command::new("test");
        let root = register_health_command(root);
        let root = register_usage_command(root);
        let root = register_enable_command(root);
        let root = register_disable_command(root);
        let root = register_reload_command(root);
        let root = register_config_command(root);
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        for name in SYSTEM_COMMANDS {
            assert!(subs.contains(name), "missing system command: {name}");
        }
    }

    #[test]
    fn test_register_health_command_attaches_health() {
        let root = register_health_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"health"));
    }

    #[test]
    fn test_register_usage_command_attaches_usage() {
        let root = register_usage_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"usage"));
    }

    #[test]
    fn test_register_enable_command_attaches_enable() {
        let root = register_enable_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"enable"));
    }

    #[test]
    fn test_register_disable_command_attaches_disable() {
        let root = register_disable_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"disable"));
    }

    #[test]
    fn test_register_reload_command_attaches_reload() {
        let root = register_reload_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"reload"));
    }

    #[test]
    fn test_register_config_command_attaches_config() {
        let root = register_config_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"config"));
    }

    #[test]
    fn test_register_health_is_isolated() {
        let root = register_health_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"health"));
        assert!(!subs.contains(&"usage"));
        assert!(!subs.contains(&"enable"));
        assert!(!subs.contains(&"disable"));
        assert!(!subs.contains(&"reload"));
        assert!(!subs.contains(&"config"));
    }

    #[test]
    fn test_register_usage_is_isolated() {
        let root = register_usage_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"usage"));
        assert!(!subs.contains(&"health"));
        assert!(!subs.contains(&"enable"));
    }

    #[test]
    fn test_register_config_is_isolated() {
        let root = register_config_command(Command::new("root"));
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"config"));
        assert!(!subs.contains(&"health"));
        assert!(!subs.contains(&"usage"));
    }

    // -- Behavioral tests for dispatch arg-parsing (review #5) ---------------

    /// Build a parsed ArgMatches the way main.rs would feed dispatch_*.
    fn parse_subcommand(args: &[&str]) -> clap::ArgMatches {
        let cmd = Command::new("root")
            .subcommand(health_command())
            .subcommand(usage_command())
            .subcommand(enable_command())
            .subcommand(disable_command())
            .subcommand(reload_command())
            .subcommand(config_command());
        cmd.try_get_matches_from(std::iter::once("root").chain(args.iter().copied()))
            .expect("parse must succeed for valid args")
    }

    #[test]
    fn test_enable_command_requires_module_id_and_reason() {
        let cmd = enable_command();
        // Missing reason → parse error.
        let result = cmd
            .clone()
            .try_get_matches_from(vec!["enable", "my.module"]);
        assert!(
            result.is_err(),
            "enable without --reason must fail to parse"
        );
        // Both required → ok.
        let result = cmd.try_get_matches_from(vec!["enable", "my.module", "--reason", "ops"]);
        assert!(result.is_ok(), "enable with --reason must parse");
    }

    #[test]
    fn test_disable_command_requires_module_id_and_reason() {
        let cmd = disable_command();
        let result = cmd
            .clone()
            .try_get_matches_from(vec!["disable", "my.module"]);
        assert!(result.is_err());
        let result =
            cmd.try_get_matches_from(vec!["disable", "my.module", "--reason", "rolling-back"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_reload_command_requires_module_id_and_reason() {
        let cmd = reload_command();
        let result = cmd
            .clone()
            .try_get_matches_from(vec!["reload", "my.module"]);
        assert!(result.is_err());
        let result =
            cmd.try_get_matches_from(vec!["reload", "my.module", "--reason", "config-change"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_yes_flag_propagation_through_parse() {
        // Regression for review #9: --yes must be readable from the parsed
        // matches that dispatch_enable/disable/reload pass to
        // require_approval_for_system_command. Previously --yes was captured
        // only to gate an eprintln "Note" that never reached the executor.
        let m = parse_subcommand(&["enable", "my.module", "--reason", "ops", "--yes"]);
        let sub = m.subcommand_matches("enable").unwrap();
        assert!(
            sub.get_flag("yes"),
            "--yes flag must surface as true on dispatch_enable matches"
        );

        let m = parse_subcommand(&["disable", "my.module", "--reason", "rolling-back", "-y"]);
        let sub = m.subcommand_matches("disable").unwrap();
        assert!(sub.get_flag("yes"), "-y short form must work for disable");

        let m = parse_subcommand(&["reload", "my.module", "--reason", "config-change", "--yes"]);
        let sub = m.subcommand_matches("reload").unwrap();
        assert!(sub.get_flag("yes"));
    }

    #[test]
    fn test_config_set_exposes_yes_flag() {
        // Regression for review #9: config set was missing --yes entirely.
        let cmd = config_command();
        let result = cmd.try_get_matches_from(vec![
            "config",
            "set",
            "feature.x",
            "true",
            "--reason",
            "ops",
            "--yes",
        ]);
        assert!(
            result.is_ok(),
            "config set must accept --yes (review #9): {:?}",
            result.err()
        );
        let set_m = result.unwrap().subcommand_matches("set").cloned().unwrap();
        assert!(set_m.get_flag("yes"), "--yes must read true on config set");
    }

    #[test]
    fn test_health_module_id_is_optional() {
        let cmd = health_command();
        // Without module_id: summary mode.
        let result = cmd.clone().try_get_matches_from(vec!["health"]);
        assert!(result.is_ok(), "health must default to summary mode");
        // With module_id: per-module mode.
        let result = cmd.try_get_matches_from(vec!["health", "my.module"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_usage_period_default_is_24h() {
        let cmd = usage_command();
        let m = cmd.try_get_matches_from(vec!["usage"]).unwrap();
        let period = m.get_one::<String>("period").cloned().unwrap_or_default();
        assert_eq!(period, "24h", "default usage period must be '24h'");
    }
}
