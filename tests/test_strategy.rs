//! Behavioral tests for the `strategy` module (FE-11 describe-pipeline +
//! --strategy). Covers the clap command builder surface and value-parser
//! constraints — the dispatch_* function calls std::process::exit at every
//! exit path, so it's tested via subprocess in tests/test_e2e.rs and via the
//! parse layer here. Review #28 expanded this from a single smoke test.

use apcore_cli::strategy::describe_pipeline_command;

#[test]
fn strategy_module_describe_pipeline_command_constructible() {
    let cmd = describe_pipeline_command();
    assert_eq!(cmd.get_name(), "describe-pipeline");
}

#[test]
fn describe_pipeline_command_exposes_strategy_and_format_flags() {
    let cmd = describe_pipeline_command();
    let arg_ids: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
    assert!(
        arg_ids.contains(&"strategy"),
        "must have --strategy flag, got {arg_ids:?}"
    );
    assert!(
        arg_ids.contains(&"format"),
        "must have --format flag, got {arg_ids:?}"
    );
}

#[test]
fn describe_pipeline_strategy_flag_accepts_all_five_presets() {
    let cmd = describe_pipeline_command();
    for preset in ["standard", "internal", "testing", "performance", "minimal"] {
        let m = cmd
            .clone()
            .try_get_matches_from(vec!["describe-pipeline", "--strategy", preset]);
        assert!(
            m.is_ok(),
            "--strategy {preset} must parse, got error: {:?}",
            m.err()
        );
        let val = m
            .unwrap()
            .get_one::<String>("strategy")
            .cloned()
            .unwrap_or_default();
        assert_eq!(val, preset);
    }
}

#[test]
fn describe_pipeline_strategy_flag_rejects_unknown_value() {
    let cmd = describe_pipeline_command();
    let result = cmd.try_get_matches_from(vec!["describe-pipeline", "--strategy", "exotic-mode"]);
    assert!(
        result.is_err(),
        "unknown strategy value must be rejected by clap, got {result:?}"
    );
}

#[test]
fn describe_pipeline_format_flag_accepts_table_and_json() {
    let cmd = describe_pipeline_command();
    for fmt in ["table", "json"] {
        let m = cmd
            .clone()
            .try_get_matches_from(vec!["describe-pipeline", "--format", fmt]);
        assert!(m.is_ok(), "--format {fmt} must parse");
    }
}

#[test]
fn describe_pipeline_format_flag_rejects_unknown_value() {
    let cmd = describe_pipeline_command();
    let result = cmd.try_get_matches_from(vec!["describe-pipeline", "--format", "yaml"]);
    assert!(
        result.is_err(),
        "unknown format value must be rejected, got {result:?}"
    );
}

#[test]
fn describe_pipeline_strategy_default_is_standard() {
    let cmd = describe_pipeline_command();
    let m = cmd.try_get_matches_from(vec!["describe-pipeline"]).unwrap();
    let val = m.get_one::<String>("strategy").cloned().unwrap_or_default();
    assert_eq!(val, "standard", "default strategy must be 'standard'");
}
