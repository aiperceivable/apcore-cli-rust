// apcore-cli -- Pipeline strategy commands (FE-11).
// Provides describe-pipeline subcommand showing execution pipeline steps.

use apcore::StrategyInfo;
use clap::{Arg, Command};
use serde_json::Value;
use std::io::IsTerminal;

// ---------------------------------------------------------------------------
// Strategy info — delegates to apcore preset builders
// ---------------------------------------------------------------------------

/// Return a `StrategyInfo` for the named preset strategy by invoking the
/// canonical builder from the `apcore` crate. This ensures the CLI always
/// reflects the same steps that the `apcore` executor actually uses, rather
/// than maintaining a parallel hardcoded list that can drift out of sync.
///
/// Returns `None` for unknown strategy names.
fn get_strategy_info(strategy: &str) -> Option<StrategyInfo> {
    match strategy {
        "standard" => Some(apcore::build_standard_strategy().info()),
        "internal" => Some(apcore::build_internal_strategy().info()),
        "testing" => Some(apcore::build_testing_strategy().info()),
        "performance" => Some(apcore::build_performance_strategy().info()),
        "minimal" => Some(apcore::build_minimal_strategy().info()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Command builder
// ---------------------------------------------------------------------------

/// Build the `describe-pipeline` clap subcommand.
pub fn describe_pipeline_command() -> Command {
    Command::new("describe-pipeline")
        .about("Show the execution pipeline steps for a strategy")
        .arg(
            Arg::new("strategy")
                .long("strategy")
                .value_parser(["standard", "internal", "testing", "performance", "minimal"])
                .default_value("standard")
                .value_name("STRATEGY")
                .help("Strategy to describe (default: standard)."),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["table", "json"])
                .value_name("FORMAT")
                .help("Output format."),
        )
}

/// Register the describe-pipeline subcommand on the root command.
pub(crate) fn register_pipeline_command(cli: Command) -> Command {
    cli.subcommand(describe_pipeline_command())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the `describe-pipeline` subcommand.
pub fn dispatch_describe_pipeline(matches: &clap::ArgMatches) {
    let strategy = matches
        .get_one::<String>("strategy")
        .map(|s| s.as_str())
        .unwrap_or("standard");
    let format = matches.get_one::<String>("format").map(|s| s.as_str());
    let fmt = crate::output::resolve_format(format);

    let info = match get_strategy_info(strategy) {
        Some(info) => info,
        None => {
            eprintln!("Error: Unknown strategy: {strategy}");
            std::process::exit(2);
        }
    };

    // Step metadata: which steps are pure and which are non-removable.
    let pure_steps = [
        "context_creation",
        "call_chain_guard",
        "module_lookup",
        "acl_check",
        "input_validation",
    ];
    let non_removable = [
        "context_creation",
        "module_lookup",
        "execute",
        "return_result",
    ];

    if fmt == "json" || !std::io::stdout().is_terminal() {
        let steps_json: Vec<Value> = info
            .step_names
            .iter()
            .enumerate()
            .map(|(i, s)| {
                serde_json::json!({
                    "index": i + 1,
                    "name": s,
                    "pure": pure_steps.contains(&s.as_str()),
                    "removable": !non_removable.contains(&s.as_str()),
                })
            })
            .collect();
        let payload = serde_json::json!({
            "strategy": info.name,
            "step_count": info.step_count,
            "steps": steps_json,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("Pipeline: {} ({} steps)\n", info.name, info.step_count);
        println!("  #    Step                         Pure   Removable   Timeout");
        println!("  ---- ---------------------------- ------ ----------- --------");
        for (i, s) in info.step_names.iter().enumerate() {
            let pure = if pure_steps.contains(&s.as_str()) {
                "yes"
            } else {
                "no"
            };
            let removable = if non_removable.contains(&s.as_str()) {
                "no"
            } else {
                "yes"
            };
            println!("  {:<4} {:<28} {:<6} {:<11} --", i + 1, s, pure, removable);
        }
    }

    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_strategy_info_standard() {
        let info = get_strategy_info("standard").expect("standard strategy must exist");
        assert_eq!(info.step_count, 11);
        assert_eq!(info.step_names[0], "context_creation");
        assert!(info.step_names.contains(&"execute".to_string()));
        assert_eq!(info.name, "standard");
    }

    #[test]
    fn test_get_strategy_info_internal() {
        let info = get_strategy_info("internal").expect("internal strategy must exist");
        assert_eq!(info.step_count, 9);
        assert!(!info.step_names.contains(&"acl_check".to_string()));
    }

    #[test]
    fn test_get_strategy_info_testing() {
        let info = get_strategy_info("testing").expect("testing strategy must exist");
        assert_eq!(info.step_count, 8);
        assert!(!info.step_names.contains(&"call_chain_guard".to_string()));
    }

    #[test]
    fn test_get_strategy_info_performance() {
        let info = get_strategy_info("performance").expect("performance strategy must exist");
        assert_eq!(info.step_count, 9);
        assert!(!info.step_names.contains(&"middleware_before".to_string()));
    }

    #[test]
    fn test_get_strategy_info_minimal() {
        let info = get_strategy_info("minimal").expect("minimal strategy must exist");
        assert!(info.step_count <= 4);
        assert!(info.step_names.contains(&"execute".to_string()));
    }

    #[test]
    fn test_get_strategy_info_unknown_returns_none() {
        assert!(get_strategy_info("unknown").is_none());
        assert!(get_strategy_info("").is_none());
    }

    #[test]
    fn test_describe_pipeline_command_builder() {
        let cmd = describe_pipeline_command();
        assert_eq!(cmd.get_name(), "describe-pipeline");
        let opts: Vec<&str> = cmd.get_opts().filter_map(|a| a.get_long()).collect();
        assert!(opts.contains(&"strategy"));
        assert!(opts.contains(&"format"));
    }

    #[test]
    fn test_register_pipeline_command() {
        let root = Command::new("test");
        let root = register_pipeline_command(root);
        let subs: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
        assert!(subs.contains(&"describe-pipeline"));
    }
}
