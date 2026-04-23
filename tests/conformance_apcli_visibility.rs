// FE-13 cross-language conformance fixture tests (T-APCLI-31).
//
// All fixtures (`create_cli.json` / `env.json` / `input.yaml` /
// `expected_help.txt`) live in the spec repo at
// `../apcore-cli/conformance/fixtures/apcli-visibility/<scenario>/` and
// are shared across every SDK (TypeScript, Python, Rust). Each SDK is
// responsible for making its underlying help renderer (Commander.js /
// Click / clap) emit the canonical clap-style format — see the spec
// repo's `conformance/fixtures/apcli-visibility/README.md` for the
// format rules.
//
// Behavioral assertions (apcli group visibility, registered
// subcommands) run today — they verify that the Rust SDK resolves the
// FE-13 4-tier decision chain identically to the TypeScript reference
// implementation. Byte-matching against `expected_help.txt` is gated
// behind `#[ignore]` until the canonical help formatter (tracked
// alongside `apcore-cli-typescript/src/canonical-help.ts`) is ported
// to clap.

use apcore_cli::{register_apcli_subcommands, ApcliConfig, ApcliGroup, ApcliMode, ConfigResolver};
use clap::Command;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Paths — resolve the spec repo relative to this crate. Overridable via
// APCORE_CLI_SPEC_REPO for CI where the spec repo is checked out
// separately.
// ---------------------------------------------------------------------------

fn spec_repo_root() -> PathBuf {
    if let Ok(p) = std::env::var("APCORE_CLI_SPEC_REPO") {
        return PathBuf::from(p);
    }
    // CARGO_MANIFEST_DIR points at apcore-cli-rust; spec repo sits alongside.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir must have a parent")
        .join("apcore-cli")
}

fn fixture_root() -> PathBuf {
    spec_repo_root().join("conformance/fixtures/apcli-visibility")
}

fn discover_scenarios() -> Vec<String> {
    let root = fixture_root();
    if !root.is_dir() {
        return vec![];
    }
    let mut scenarios = vec![];
    for entry in std::fs::read_dir(&root).expect("read_dir fixture_root") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() && path.join("create_cli.json").is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                scenarios.push(name.to_string());
            }
        }
    }
    scenarios.sort();
    scenarios
}

// ---------------------------------------------------------------------------
// Fixture data
// ---------------------------------------------------------------------------

struct Scenario {
    name: String,
    shared: Value,                // create_cli.json
    env: HashMap<String, String>, // env.json
    yaml_text: Option<String>,    // input.yaml (optional)
    expected_help: String,        // expected_help.txt
}

fn load_scenario(name: &str) -> Scenario {
    let dir = fixture_root().join(name);
    let shared: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("create_cli.json")).expect("create_cli.json"),
    )
    .expect("create_cli.json must be valid JSON");
    let env: HashMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("env.json")).expect("env.json"))
            .expect("env.json must be valid JSON");
    let yaml_path = dir.join("input.yaml");
    let yaml_text = if yaml_path.is_file() {
        Some(std::fs::read_to_string(&yaml_path).expect("input.yaml"))
    } else {
        None
    };
    let expected_help =
        std::fs::read_to_string(dir.join("expected_help.txt")).expect("expected_help.txt");
    Scenario {
        name: name.to_string(),
        shared,
        env,
        yaml_text,
        expected_help,
    }
}

// ---------------------------------------------------------------------------
// Env + cwd serialization — std::env is process-global, so tests that
// touch APCORE_CLI_APCLI or chdir must run under a shared mutex.
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
    saved_cwd: Option<PathBuf>,
}

impl EnvGuard {
    fn apply(env: &HashMap<String, String>, cwd: &Path) -> Self {
        let mut saved = Vec::new();
        // Always baseline APCORE_CLI_APCLI so scenario isolation is
        // deterministic regardless of the ambient shell.
        let baseline_key = "APCORE_CLI_APCLI";
        saved.push((baseline_key.to_string(), std::env::var(baseline_key).ok()));
        std::env::remove_var(baseline_key);
        for (k, v) in env {
            if saved.iter().all(|(sk, _)| sk != k) {
                saved.push((k.clone(), std::env::var(k).ok()));
            }
            std::env::set_var(k, v);
        }
        let saved_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(cwd).expect("chdir");
        Self { saved, saved_cwd }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in self.saved.drain(..) {
            match v {
                Some(val) => std::env::set_var(&k, val),
                None => std::env::remove_var(&k),
            }
        }
        if let Some(cwd) = self.saved_cwd.take() {
            let _ = std::env::set_current_dir(cwd);
        }
    }
}

// ---------------------------------------------------------------------------
// Scenario → clap::Command composition.
//
// Mirrors the logic in `build_cli_command` (main.rs) using the public
// building blocks exposed by the crate. `create_cli` does not yet accept
// `apcli` / `registry_injected` arguments, so this helper composes the
// root directly — a reference harness for the Rust port of the
// canonical formatter.
// ---------------------------------------------------------------------------

fn build_scenario_command(shared: &Value) -> Command {
    let prog_name = shared
        .get("prog_name")
        .and_then(Value::as_str)
        .unwrap_or("apcore-cli")
        .to_string();
    let registry_injected = shared
        .get("registry_injected")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Tier 1 — explicit CliConfig apcli override.
    let cli_cfg = shared.get("apcli").map(parse_apcli_value);

    // Tier 3 — apcore.yaml (consumed from current cwd, set by the scenario
    // harness).
    let yaml_val = if Path::new("apcore.yaml").exists() {
        ConfigResolver::new(None, Some(PathBuf::from("apcore.yaml"))).resolve_object("apcli")
    } else {
        None
    };

    // Tier resolution: CliConfig > yaml > auto-detect.
    let apcli_cfg = match cli_cfg {
        Some(cfg) => ApcliGroup::from_cli_config(Some(cfg), registry_injected),
        None => ApcliGroup::from_yaml(yaml_val, registry_injected),
    };

    // Root command — match the description/flag set used by
    // `build_cli_command` so the help output is comparable.
    let mut cmd = Command::new(prog_name.clone())
        .version(env!("CARGO_PKG_VERSION"))
        .about("CLI adapter for the apcore module ecosystem.")
        .allow_external_subcommands(true)
        .arg(
            clap::Arg::new("log-level")
                .long("log-level")
                .global(true)
                .value_name("LEVEL")
                .help("Logging level (DEBUG|INFO|WARNING|ERROR)"),
        )
        .arg(
            clap::Arg::new("verbose")
                .long("verbose")
                .global(true)
                .action(clap::ArgAction::SetTrue)
                .help("Show all options in help output (including built-in apcore options)"),
        );
    if !registry_injected {
        cmd = cmd
            .arg(
                clap::Arg::new("extensions-dir")
                    .long("extensions-dir")
                    .global(true)
                    .value_name("PATH")
                    .help("Path to extensions directory"),
            )
            .arg(
                clap::Arg::new("commands-dir")
                    .long("commands-dir")
                    .global(true)
                    .value_name("PATH")
                    .help("Path to convention-based commands directory"),
            )
            .arg(
                clap::Arg::new("binding")
                    .long("binding")
                    .global(true)
                    .value_name("PATH")
                    .help("Path to binding.yaml for display overlay"),
            );
    }

    let apcli_group = Command::new("apcli")
        .about("apcore-cli built-in commands")
        .hide(!apcli_cfg.is_group_visible());
    let apcli_group = register_apcli_subcommands(apcli_group, &apcli_cfg, &prog_name);
    cmd = cmd.subcommand(apcli_group);
    cmd
}

fn parse_apcli_value(v: &Value) -> ApcliConfig {
    // Shorthand booleans: true → all, false → none.
    if let Some(b) = v.as_bool() {
        return ApcliConfig {
            mode: if b { ApcliMode::All } else { ApcliMode::None },
            disable_env: false,
        };
    }
    let obj = v.as_object().cloned().unwrap_or_default();
    let mode_str = obj
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string();
    let include: Vec<String> = obj
        .get("include")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let exclude: Vec<String> = obj
        .get("exclude")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let disable_env = obj
        .get("disable_env")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mode = match mode_str.as_str() {
        "all" => ApcliMode::All,
        "none" => ApcliMode::None,
        "include" => ApcliMode::Include(include),
        "exclude" => ApcliMode::Exclude(exclude),
        // Unknown modes aren't expected in the curated fixtures; fall
        // through to All so a misconfigured fixture surfaces via the
        // behavioral assertions rather than a panic here.
        _ => ApcliMode::All,
    };
    ApcliConfig { mode, disable_env }
}

// ---------------------------------------------------------------------------
// Behavioral assertions
// ---------------------------------------------------------------------------

fn expected_apcli_visible(expected_help: &str) -> bool {
    // Walk the Commands section of the canonical help output. The
    // section is always bounded by the "Commands:" and "Options:"
    // headers that the canonical format mandates.
    let after = match expected_help.split_once("Commands:") {
        Some((_, rest)) => rest,
        None => return false,
    };
    let section = match after.split_once("Options:") {
        Some((s, _)) => s,
        None => after,
    };
    section
        .lines()
        .any(|line| line.trim_start().starts_with("apcli "))
}

fn assert_group_visibility_matches(scenario: &Scenario, root: &Command) {
    let apcli = root
        .get_subcommands()
        .find(|c| c.get_name() == "apcli")
        .unwrap_or_else(|| panic!("[{}] apcli group must always be registered", scenario.name));
    let actual_visible = !apcli.is_hide_set();
    let want_visible = expected_apcli_visible(&scenario.expected_help);
    assert_eq!(
        actual_visible, want_visible,
        "[{}] apcli group visibility mismatch: visible={}, expected={}",
        scenario.name, actual_visible, want_visible,
    );
}

fn assert_subcommand_registration(scenario: &Scenario, root: &Command) {
    let apcli = root
        .get_subcommands()
        .find(|c| c.get_name() == "apcli")
        .expect("apcli group");
    let registered: Vec<&str> = apcli.get_subcommands().map(|c| c.get_name()).collect();

    // Spec §4.9: exec is always registered regardless of mode.
    assert!(
        registered.contains(&"exec"),
        "[{}] 'exec' must always be registered; got {:?}",
        scenario.name,
        registered,
    );

    // Mode: include must expose exactly the listed subcommands + exec.
    let apcli_opt = scenario.shared.get("apcli");
    let yaml_include = extract_yaml_include(scenario.yaml_text.as_deref());
    let cli_include = apcli_opt
        .and_then(|v| {
            let mode = v.get("mode").and_then(Value::as_str);
            if mode == Some("include") {
                v.get("include").and_then(Value::as_array).map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
            } else {
                None
            }
        })
        .or(yaml_include);

    if let Some(list) = cli_include {
        for required in &list {
            assert!(
                registered.contains(&required.as_str()),
                "[{}] include list requires '{}'; got {:?}",
                scenario.name,
                required,
                registered,
            );
        }
        let mut allowed: Vec<String> = list;
        allowed.push("exec".to_string());
        let stray: Vec<&&str> = registered
            .iter()
            .filter(|n| !allowed.iter().any(|a| a == *n))
            .collect();
        assert!(
            stray.is_empty(),
            "[{}] include-mode leaked subcommands: {:?}",
            scenario.name,
            stray,
        );
    }
}

fn extract_yaml_include(yaml: Option<&str>) -> Option<Vec<String>> {
    let text = yaml?;
    let parsed: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let apcli = parsed.get("apcli")?;
    if apcli.get("mode")?.as_str()? != "include" {
        return None;
    }
    let include = apcli.get("include")?.as_sequence()?;
    Some(
        include
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Test runners — one per scenario. Dynamic test generation is awkward
// in stable Rust, so the five scenarios are enumerated explicitly.
// Each body is a one-line dispatch so adding a new scenario is cheap.
// ---------------------------------------------------------------------------

fn run_scenario_behavior(scenario_name: &str) {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let root_dir = fixture_root();
    if !root_dir.is_dir() {
        eprintln!(
            "skipping {scenario_name}: spec repo fixtures not found at {}",
            root_dir.display()
        );
        return;
    }
    let scenario = load_scenario(scenario_name);
    let cwd = tempfile::tempdir().expect("tempdir");
    if let Some(yaml) = &scenario.yaml_text {
        std::fs::write(cwd.path().join("apcore.yaml"), yaml).expect("write apcore.yaml");
    }
    let _guard = EnvGuard::apply(&scenario.env, cwd.path());
    let cmd = build_scenario_command(&scenario.shared);
    assert_group_visibility_matches(&scenario, &cmd);
    assert_subcommand_registration(&scenario, &cmd);
}

#[test]
fn conformance_standalone_default() {
    run_scenario_behavior("standalone-default");
}

#[test]
fn conformance_embedded_default() {
    run_scenario_behavior("embedded-default");
}

#[test]
fn conformance_cli_override() {
    run_scenario_behavior("cli-override");
}

#[test]
fn conformance_env_override() {
    run_scenario_behavior("env-override");
}

#[test]
fn conformance_yaml_include() {
    run_scenario_behavior("yaml-include");
}

// ---------------------------------------------------------------------------
// Golden byte-match — ignored pending canonical help formatter port.
//
// The Rust SDK uses clap's default help layout, which diverges from the
// canonical clap v4 / GNU format asserted by the shared golden (see
// apcore-cli-typescript/src/canonical-help.ts for the reference). This
// test runs under `cargo test -- --ignored` once the formatter is
// ported; today it documents the contract and wires the fixture reader.
// ---------------------------------------------------------------------------

#[ignore = "canonical clap-style help format not yet implemented in the Rust SDK; tracked for parity with apcore-cli-typescript/src/canonical-help.ts"]
#[test]
fn conformance_help_matches_golden_all_scenarios() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let scenarios = discover_scenarios();
    assert!(!scenarios.is_empty(), "no conformance scenarios found");
    for name in scenarios {
        let scenario = load_scenario(&name);
        let cwd = tempfile::tempdir().expect("tempdir");
        if let Some(yaml) = &scenario.yaml_text {
            std::fs::write(cwd.path().join("apcore.yaml"), yaml).expect("write apcore.yaml");
        }
        let _guard = EnvGuard::apply(&scenario.env, cwd.path());
        let mut cmd = build_scenario_command(&scenario.shared);
        let actual = cmd.render_help().to_string();
        assert_eq!(
            actual, scenario.expected_help,
            "[{}] help output diverges from canonical golden",
            name,
        );
    }
}
