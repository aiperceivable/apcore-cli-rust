// apcore-cli -- FE-14 ACL governance integration tests.
//
// Covers the T-ACL-* verification matrix from
// `apcore-cli/docs/features/acl-governance.md` section 8, including the
// section 4.8 audit rows (T-ACL-26, 27, 27a, 27b, 27c). An earlier revision of
// this header held those back pending a public `ACL::set_audit_logger` in
// Python and TypeScript; that prerequisite was retracted (spec section 10) --
// all three SDKs take the callback through the ACL constructor, and Rust has
// the setter on top of that.
//
// The command surface is driven end-to-end through the real binary, so the
// exit codes asserted here are the ones an operator's script would see.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use apcore::acl::{ACLRule, ApprovalRequirement, ACL};
use apcore::module::Module;
use apcore::{Config, Executor, ModuleAnnotations, Registry};
use apcore_cli::acl_cmd::{
    check_exit_code, status_exit_code, validate_exit_code, warn_if_strategy_bypasses_acl,
};
use apcore_cli::acl_loader::{
    acl_audit_enabled, acl_audit_include_denied, install_acl_audit_logger, load_cli_acl,
    load_cli_acl_with_audit, resolve_acl_file, resolve_acl_root, ACL_AUDIT_FIELDS, ACL_ROOT_ENV,
};
use apcore_cli::{AuditLogger, ConfigResolver};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A scratch project: an `extensions/` directory (the binary refuses to start
/// without one) plus whatever ACL fixture the test writes.
struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("extensions")).expect("mkdir extensions");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&path, contents).expect("write fixture");
        path
    }

    /// Run the CLI with the project directory as the working directory, so
    /// tier-4 `./acl` resolution and `apcore.yaml` discovery behave as they
    /// would for a real user.
    fn run(&self, args: &[&str]) -> Output {
        self.run_with_env(args, &[])
    }

    fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_apcore-cli"));
        cmd.current_dir(self.dir.path())
            .args(args)
            // Keep the shared audit log out of the way of a sandboxed run.
            .env("APCORE_CLI_AUDIT_DISABLE", "1")
            .env_remove(ACL_ROOT_ENV);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().expect("failed to spawn apcore-cli")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn json_stdout(out: &Output) -> Value {
    serde_json::from_str(&stdout(out))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", stdout(out)))
}

const THREE_RULE_ACL: &str = "\
default_effect: deny
rules:
  - callers: ['@external']
    targets: ['system.control.*']
    effect: deny
    description: no external control
  - callers: ['*']
    targets: ['db.migrate']
    effect: allow
    approval: required
    description: migrations need a human
    conditions:
      roles: ['admin']
  - callers: ['*']
    targets: ['db.read']
    effect: allow
";

// ---------------------------------------------------------------------------
// T-ACL-01 / T-ACL-02 / T-ACL-05 -- attachment
// ---------------------------------------------------------------------------

#[test]
fn t_acl_01_no_acl_directory_changes_nothing() {
    let project = Project::new();
    let out = project.run(&["apcli", "acl", "status", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    assert_eq!(payload["acl_configured"], false);
    assert_eq!(payload["acl_source"], Value::Null);

    // And an ordinary command is unaffected.
    let listed = project.run(&["apcli", "list"]);
    assert_eq!(listed.status.code(), Some(0), "stderr: {}", stderr(&listed));
}

#[test]
fn t_acl_02_global_acl_in_cwd_is_attached() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "status", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    assert_eq!(payload["acl_configured"], true);
    assert!(payload["acl_source"]
        .as_str()
        .expect("acl_source is reported")
        .ends_with("global_acl.yaml"));
}

#[test]
fn t_acl_05_directory_without_global_acl_attaches_nothing() {
    let project = Project::new();
    std::fs::create_dir(project.path().join("acl")).expect("mkdir acl");
    let out = project.run(&["apcli", "acl", "status", "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "an absent conventional file is not an error; stderr: {}",
        stderr(&out)
    );
    assert_eq!(json_stdout(&out)["acl_configured"], false);
}

// ---------------------------------------------------------------------------
// T-ACL-03 / T-ACL-04 -- precedence
// ---------------------------------------------------------------------------

#[test]
fn t_acl_03_flag_beats_acl_root_in_yaml() {
    let project = Project::new();
    project.write("apcore.yaml", "acl:\n  root: ./from-yaml\n");
    project.write(
        "from-yaml/global_acl.yaml",
        "default_effect: allow\nrules: []\n",
    );
    project.write("custom.yaml", THREE_RULE_ACL);

    let out = project.run(&[
        "--acl",
        "./custom.yaml",
        "apcli",
        "acl",
        "list",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    assert_eq!(payload["default_effect"], "deny", "tier 1 must win");
    assert_eq!(payload["rules"].as_array().map(Vec::len), Some(3));
}

#[test]
fn t_acl_04_env_beats_acl_root_in_yaml() {
    let project = Project::new();
    project.write("apcore.yaml", "acl:\n  root: ./from-yaml\n");
    project.write(
        "from-yaml/global_acl.yaml",
        "default_effect: allow\nrules: []\n",
    );
    project.write("other/global_acl.yaml", THREE_RULE_ACL);

    let out = project.run_with_env(
        &["apcli", "acl", "list", "--format", "json"],
        &[(ACL_ROOT_ENV, "./other")],
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(
        json_stdout(&out)["default_effect"],
        "deny",
        "tier 2 must win"
    );
}

#[test]
fn tier3_yaml_is_used_when_no_flag_or_env_is_present() {
    let project = Project::new();
    project.write("apcore.yaml", "acl:\n  root: ./from-yaml\n");
    project.write("from-yaml/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "list", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(json_stdout(&out)["rules"].as_array().map(Vec::len), Some(3));
}

#[test]
fn resolve_acl_root_reports_the_tiers_directly() {
    // The precedence chain, exercised without a process so the tier boundary
    // is visible rather than inferred from a rendered document.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("apcore.yaml"),
        "acl:\n  root: ./from-yaml\n",
    )
    .expect("write");
    let resolver = ConfigResolver::new(None, Some(dir.path().join("apcore.yaml")));
    assert_eq!(
        resolve_acl_root(&resolver, Some("./flag")),
        Some("./flag".to_string())
    );
    assert_eq!(
        resolve_acl_root(&resolver, None),
        Some("./from-yaml".to_string())
    );
}

// ---------------------------------------------------------------------------
// T-ACL-06 / T-ACL-07 / T-ACL-08 -- structurally invalid files exit 47
// ---------------------------------------------------------------------------

fn assert_invalid_acl_exits_47(fixture: &str, label: &str) {
    let project = Project::new();
    project.write("acl/global_acl.yaml", fixture);
    let out = project.run(&["apcli", "acl", "list"]);
    assert_eq!(
        out.status.code(),
        Some(47),
        "{label} must exit 47 (CONFIG_INVALID), not the generic 1; stderr: {}",
        stderr(&out)
    );
    assert_ne!(
        out.status.code(),
        Some(77),
        "{label} is a configuration fault, never an access decision"
    );
    assert!(
        stderr(&out).contains("Invalid ACL configuration in "),
        "{label} stderr: {}",
        stderr(&out)
    );
}

#[test]
fn t_acl_06_unknown_rule_key_exits_47_and_names_the_rule_index() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: allow\n    bogus_key: 1\n",
    );
    let out = project.run(&["apcli", "acl", "list"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Invalid ACL configuration in "), "{err}");
    assert!(
        err.contains('0') || err.to_lowercase().contains("rule"),
        "the message must locate the offending rule: {err}"
    );
}

#[test]
fn t_acl_07_effect_enum_is_closed() {
    assert_invalid_acl_exits_47(
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: permit\n",
        "effect: permit",
    );
}

#[test]
fn t_acl_08_empty_pattern_array_is_refused() {
    assert_invalid_acl_exits_47(
        "default_effect: deny\nrules:\n  - callers: []\n    targets: ['*']\n    effect: allow\n",
        "callers: []",
    );
}

#[test]
fn a_vanished_file_reports_not_found_rather_than_invalid() {
    // `resolve_acl_file` sees the file; `ACL::load` then fails to read it.
    // Simulated directly because the race cannot be staged through argv.
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("acl.yaml");
    std::fs::write(&file, "default_effect: deny\nrules: []\n").expect("write");
    assert!(resolve_acl_file(file.to_str().unwrap()).is_some());
    std::fs::remove_file(&file).expect("remove");
    assert!(
        load_cli_acl(file.to_str().unwrap())
            .expect("a vanished path is the missing-path no-op")
            .is_none(),
        "a path that no longer exists attaches nothing rather than erroring"
    );
}

// ---------------------------------------------------------------------------
// T-ACL-09 / T-ACL-10 -- `acl list`
// ---------------------------------------------------------------------------

#[test]
fn t_acl_09_list_json_preserves_definition_order() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "list", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    let rules = payload["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 3);
    for (i, rule) in rules.iter().enumerate() {
        assert_eq!(rule["index"], i as u64);
    }
    assert_eq!(rules[0]["targets"][0], "system.control.*");
    assert_eq!(rules[1]["approval"], "required");
    assert_eq!(rules[1]["conditions"]["roles"][0], "admin");
    assert_eq!(rules[2]["targets"][0], "db.read");
}

#[test]
fn t_acl_10_list_with_no_acl_is_the_documented_empty_shape() {
    let project = Project::new();
    let out = project.run(&["apcli", "acl", "list", "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "listing nothing is not an error; stderr: {}",
        stderr(&out)
    );
    let payload = json_stdout(&out);
    assert_eq!(payload["source"], Value::Null);
    assert_eq!(payload["default_effect"], Value::Null);
    assert_eq!(payload["rules"].as_array().map(Vec::len), Some(0));
}

#[test]
fn list_table_with_no_acl_says_so_and_exits_0() {
    let project = Project::new();
    let out = project.run(&["apcli", "acl", "list", "--format", "table"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).contains("No ACL configured."),
        "{}",
        stdout(&out)
    );
}

// ---------------------------------------------------------------------------
// T-ACL-11 .. T-ACL-16 -- `acl check`
// ---------------------------------------------------------------------------

#[test]
fn t_acl_11_allow_rule_exits_0_and_reports_the_matched_rule() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "check", "db.read", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    assert_eq!(payload["access"], "allow");
    assert_eq!(payload["caller"], "@external");
    assert_eq!(payload["matched_rule_index"], 2);
}

#[test]
fn t_acl_12_deny_rule_exits_77() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&[
        "apcli",
        "acl",
        "check",
        "system.control.disable",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(77), "stderr: {}", stderr(&out));
    assert_eq!(json_stdout(&out)["access"], "deny");
    assert!(
        stderr(&out).contains("Access denied: @external -> system.control.disable"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn t_acl_13_allow_with_approval_exits_0_not_77() {
    // The discriminating case for section 4.5: `check()` fails closed on an
    // approval requirement, so a command built on it would report a denial for
    // a call the rule set in fact permits.
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&[
        "--role",
        "admin",
        "apcli",
        "acl",
        "check",
        "db.migrate",
        "--format",
        "json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "an allow-with-approval outcome must exit 0; stderr: {}",
        stderr(&out)
    );
    let payload = json_stdout(&out);
    assert_eq!(payload["access"], "allow");
    assert_eq!(payload["approval_required"], true);
}

#[test]
fn t_acl_14_role_flag_satisfies_a_roles_condition() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&[
        "--role",
        "admin",
        "apcli",
        "acl",
        "check",
        "db.migrate",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(json_stdout(&out)["matched_rule_index"], 1);
}

#[test]
fn t_acl_15_without_a_role_the_rule_does_not_match() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "check", "db.migrate", "--format", "json"]);
    assert_eq!(
        out.status.code(),
        Some(77),
        "falls through to default_effect: deny; stderr: {}",
        stderr(&out)
    );
    let payload = json_stdout(&out);
    assert_eq!(payload["access"], "deny");
    assert_eq!(payload["matched_rule_index"], Value::Null);
}

#[test]
fn t_acl_16_input_flag_feeds_the_arguments_condition() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['git.push']\n    effect: allow\n    conditions:\n      arguments:\n        has_key: ['force']\n",
    );
    let forced = project.run(&[
        "apcli",
        "acl",
        "check",
        "git.push",
        "--input",
        r#"{"force": true}"#,
        "--format",
        "json",
    ]);
    assert_eq!(forced.status.code(), Some(0), "stderr: {}", stderr(&forced));
    assert_eq!(json_stdout(&forced)["access"], "allow");

    // Discriminator: without the key the rule must not grant.
    let plain = project.run(&[
        "apcli",
        "acl",
        "check",
        "git.push",
        "--input",
        r#"{"remote": "origin"}"#,
        "--format",
        "json",
    ]);
    assert_eq!(plain.status.code(), Some(77), "stderr: {}", stderr(&plain));
}

#[test]
fn depth_flag_feeds_the_max_call_depth_condition() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: allow\n    conditions:\n      max_call_depth: 2\n",
    );
    let shallow = project.run(&[
        "apcli", "acl", "check", "x.y", "--depth", "1", "--format", "json",
    ]);
    assert_eq!(
        shallow.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&shallow)
    );
    let deep = project.run(&[
        "apcli", "acl", "check", "x.y", "--depth", "5", "--format", "json",
    ]);
    assert_eq!(
        deep.status.code(),
        Some(77),
        "a chain deeper than the threshold must not match; stderr: {}",
        stderr(&deep)
    );
}

#[test]
fn check_with_no_acl_exits_47() {
    let project = Project::new();
    let out = project.run(&["apcli", "acl", "check", "db.read"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("No ACL configured; nothing to check."),
        "stderr: {}",
        stderr(&out)
    );
}

/// A rule that matches only when BOTH condition keys are satisfied, one
/// supplied at the root level and one at the subcommand level. A merge that
/// resolves the identity all-or-nothing drops whichever level it did not pick,
/// and the rule then fails to match.
const TWO_LEVEL_IDENTITY_ACL: &str = "\
default_effect: deny
rules:
  - callers: ['*']
    targets: ['db.read']
    effect: allow
    description: service accounts with the admin role
    conditions:
      identity_types: ['service']
      roles: ['admin']
";

#[test]
fn identity_flags_merge_per_field_across_root_and_subcommand() {
    // Spec 4.5 precedence, end to end. `--identity-type service` is given
    // ONLY at the root and `--role admin` ONLY at the subcommand, so the rule
    // matches just in case both survived the merge. This is the discriminating
    // case: an all-or-nothing merge silently discards a field the caller never
    // withdrew, and a happy-path test where both flags sit at one level would
    // not notice.
    let project = Project::new();
    project.write("acl/global_acl.yaml", TWO_LEVEL_IDENTITY_ACL);
    let out = project.run(&[
        "--identity-type",
        "service",
        "apcli",
        "acl",
        "check",
        "--role",
        "admin",
        "db.read",
        "--format",
        "json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the root --identity-type must survive the subcommand --role; \
         stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    let payload = json_stdout(&out);
    assert_eq!(payload["access"], "allow");
    assert_eq!(payload["matched_rule_index"], 0);
}

#[test]
fn a_subcommand_role_replaces_the_root_role_rather_than_appending() {
    // The other half of "per field": the restated field is overridden, not
    // merged with, its root counterpart.
    let project = Project::new();
    project.write("acl/global_acl.yaml", TWO_LEVEL_IDENTITY_ACL);
    let out = project.run(&[
        "--identity-type",
        "service",
        "--role",
        "admin",
        "apcli",
        "acl",
        "check",
        "--role",
        "guest",
        "db.read",
        "--format",
        "json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(77),
        "the subcommand --role guest replaces the root --role admin, so the \
         rule no longer matches; stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn a_root_identity_flag_still_applies_when_not_restated() {
    // Both fields at the root, nothing restated: the rule must match.
    let project = Project::new();
    project.write("acl/global_acl.yaml", TWO_LEVEL_IDENTITY_ACL);
    let out = project.run(&[
        "--identity-type",
        "service",
        "--role",
        "admin",
        "apcli",
        "acl",
        "check",
        "db.read",
        "--format",
        "json",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(json_stdout(&out)["access"], "allow");
}

#[test]
fn roles_only_invocation_uses_the_pinned_identity_sentinel() {
    // Cross-SDK pin: `@cli`. apcore's Identity requires an id while spec 4.3
    // permits the roles-only form, so the CLI supplies one -- and it must be
    // the same literal in every SDK, since it is observable in Identity.id.
    assert_eq!(apcore_cli::DEFAULT_IDENTITY_ID, "@cli");
    let identity = apcore_cli::CliIdentity {
        id: None,
        identity_type: None,
        roles: vec!["admin".to_string()],
    };
    assert_eq!(identity.to_identity().id(), "@cli");

    // And it is not the caller: a real execution always presents @external.
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&[
        "--role",
        "admin",
        "apcli",
        "acl",
        "check",
        "db.migrate",
        "--format",
        "json",
    ]);
    assert_eq!(json_stdout(&out)["caller"], "@external");
}

#[test]
fn caller_defaults_to_external_without_a_clap_default_value() {
    // The pinned help text states the default inline; a clap `default_value`
    // would render it a second time and break the byte-match. The default is
    // therefore resolved at the use site, which this pins behaviourally.
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "check", "db.read", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(json_stdout(&out)["caller"], "@external");

    let help = project.run(&["apcli", "acl", "check", "--help"]);
    let rendered = stdout(&help);
    assert!(
        rendered.contains("Simulated caller ID (default: @external)"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("[default: @external]"),
        "clap must not append a second copy of the default: {rendered}"
    );
    assert!(
        !rendered.contains("[default: user]"),
        "same for --identity-type: {rendered}"
    );
}

#[test]
fn caller_flag_is_honoured_because_nothing_runs() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['agent.*']\n    targets: ['*']\n    effect: allow\n",
    );
    let as_agent = project.run(&[
        "apcli",
        "acl",
        "check",
        "db.read",
        "--caller",
        "agent.one",
        "--format",
        "json",
    ]);
    assert_eq!(
        as_agent.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&as_agent)
    );
    assert_eq!(json_stdout(&as_agent)["caller"], "agent.one");

    let default_caller = project.run(&["apcli", "acl", "check", "db.read", "--format", "json"]);
    assert_eq!(
        default_caller.status.code(),
        Some(77),
        "the default caller is @external, which the rule does not cover"
    );
}

// ---------------------------------------------------------------------------
// T-ACL-17 / T-ACL-18 / T-ACL-19 -- `acl validate`
// ---------------------------------------------------------------------------

#[test]
fn t_acl_17_unregistered_condition_key_is_a_finding_and_exits_47() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['db.migrate']\n    effect: deny\n    conditions:\n      mispelled: ['x']\n",
    );
    let out = project.run(&["apcli", "acl", "validate", "--format", "json"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    let payload = json_stdout(&out);
    assert_eq!(payload["count"], 1);
    let finding = &payload["findings"][0];
    assert_eq!(finding["rule_index"], 0);
    assert_eq!(finding["condition_key"], "mispelled");
    assert_eq!(finding["effect"], "deny");
    assert!(finding["condition_path"].is_string());
}

#[test]
fn t_acl_18_sync_and_async_columns_are_not_collapsed() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['db.migrate']\n    effect: deny\n    conditions:\n      mispelled: ['x']\n",
    );
    let json_out = project.run(&["apcli", "acl", "validate", "--format", "json"]);
    let finding = &json_stdout(&json_out)["findings"][0];
    assert!(
        finding["sync_resolvable"].is_boolean() && finding["async_resolvable"].is_boolean(),
        "both axes must be reported separately: {finding}"
    );

    let table_out = project.run(&["apcli", "acl", "validate", "--format", "table"]);
    let rendered = stdout(&table_out);
    assert!(rendered.contains("Sync"), "{rendered}");
    assert!(rendered.contains("Async"), "{rendered}");
}

#[test]
fn t_acl_19_clean_rule_set_reports_zero_findings_and_exits_0() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "validate", "--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert_eq!(json_stdout(&out)["count"], 0);
}

#[test]
fn validate_with_no_acl_exits_47() {
    let project = Project::new();
    let out = project.run(&["apcli", "acl", "validate"]);
    assert_eq!(out.status.code(), Some(47), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("No ACL configured; nothing to check."),
        "stderr: {}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// T-ACL-20 / T-ACL-21 -- `acl status`
// ---------------------------------------------------------------------------

/// A stand-in control module, so `governance_state()` has a control surface to
/// report on. The standalone binary registers no `system.control.*` modules,
/// so this half of the matrix is exercised against a hand-built executor.
struct ControlModule;

#[async_trait::async_trait]
impl Module for ControlModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &str {
        "Disable a module"
    }
    fn annotations(&self) -> ModuleAnnotations {
        ModuleAnnotations {
            requires_approval: false,
            ..Default::default()
        }
    }
    async fn execute(
        &self,
        _inputs: Value,
        _ctx: &apcore::Context<Value>,
    ) -> Result<Value, apcore::errors::ModuleError> {
        Ok(json!({}))
    }
}

fn control_descriptor(module_id: &str) -> apcore::registry::registry::ModuleDescriptor {
    apcore::registry::registry::ModuleDescriptor {
        module_id: module_id.to_string(),
        name: None,
        description: "Disable a module".to_string(),
        documentation: None,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(ModuleAnnotations {
            requires_approval: false,
            ..Default::default()
        }),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    }
}

fn executor_with_control_module() -> Executor {
    let registry = Registry::new();
    // `system` is a reserved first segment, so a control module must go
    // through `register_internal` -- the same door `apcore::sys_modules` uses.
    registry
        .register_internal(
            "system.control.disable",
            Box::new(ControlModule),
            control_descriptor("system.control.disable"),
        )
        .expect("registers");
    Executor::new(std::sync::Arc::new(registry), Config::default())
}

#[test]
fn t_acl_20_control_modules_with_no_acl_are_an_unprotected_surface() {
    let executor = executor_with_control_module();
    let state = executor.governance_state();
    assert!(state.control_modules_registered);
    assert!(!state.acl_configured);
    assert!(
        state.unprotected_control_surface,
        "control modules registered with neither an ACL gate nor a fully \
         annotated approval gate is exactly PROTOCOL_SPEC 6.6.5.1"
    );
}

#[test]
fn t_acl_21_strict_exits_47_on_an_unprotected_surface() {
    let executor = executor_with_control_module();
    let state = executor.governance_state();
    assert_eq!(status_exit_code(&state, /*strict*/ true), 47);
    assert_eq!(
        status_exit_code(&state, /*strict*/ false),
        0,
        "without --strict, status always exits 0"
    );
}

#[test]
fn attaching_an_acl_closes_the_unprotected_surface() {
    // The discriminating half of T-ACL-20: if `unprotected_control_surface`
    // were hardcoded, this assertion would fail.
    let mut executor = executor_with_control_module();
    let mut rule = ACLRule::new(
        vec!["@external".to_string()],
        vec!["system.control.*".to_string()],
        "deny",
    );
    rule.description = Some("no external control".to_string());
    executor.set_acl(ACL::try_new(vec![rule], "deny", None).expect("well-formed"));

    let state = executor.governance_state();
    assert!(state.acl_configured);
    assert!(state.builtin_acl_gate_wired);
    assert!(!state.unprotected_control_surface);
    assert_eq!(status_exit_code(&state, /*strict*/ true), 0);
}

#[test]
fn status_renders_the_nine_observations_over_the_binary() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", THREE_RULE_ACL);
    let out = project.run(&["apcli", "acl", "status", "--format", "table"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let rendered = stdout(&out);
    for label in [
        "Control modules registered:",
        "Read modules registered:",
        "ACL configured:",
        "Built-in ACL gate wired:",
        "Approval handler configured:",
        "Built-in approval gate wired:",
        "Policy strict:",
        "All control modules gated:",
        "Unprotected control surface:",
    ] {
        assert!(rendered.contains(label), "missing '{label}':\n{rendered}");
    }
}

// ---------------------------------------------------------------------------
// T-ACL-22 / T-ACL-23 -- the previously inert enforcement paths go live
// ---------------------------------------------------------------------------

/// Absolute path to the repo's example extensions, so the scratch project can
/// borrow a real module without copying one.
fn example_extensions() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/extensions")
}

const DENY_MATH_ADD: &str = "\
default_effect: allow
rules:
  - callers: ['*']
    targets: ['math.add']
    effect: deny
    description: math.add is off limits
";

#[test]
fn t_acl_22_and_33_exec_denied_by_acl_exits_77() {
    let project = Project::new();
    project.write("acl/global_acl.yaml", DENY_MATH_ADD);
    let ext = example_extensions();
    let out = project.run(&[
        "--extensions-dir",
        ext.to_str().expect("utf8 path"),
        "math.add",
        "--a",
        "3",
        "--b",
        "4",
    ]);
    assert_eq!(
        out.status.code(),
        Some(77),
        "an ACL denial must surface as 77; stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("math.add"),
        "the error must name the module; stderr: {}",
        stderr(&out)
    );
}

#[test]
fn exec_allowed_by_acl_still_runs() {
    // Discriminator for the test above: with the same machinery and an allow
    // rule, the call must succeed. Without this, a broken exec path would make
    // the denial test pass for the wrong reason.
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: allow\n",
    );
    let ext = example_extensions();
    let out = project.run(&[
        "--extensions-dir",
        ext.to_str().expect("utf8 path"),
        "math.add",
        "--a",
        "3",
        "--b",
        "4",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
}

#[tokio::test]
async fn t_acl_23_preflight_reports_a_failed_acl_check_row() {
    // Driven against the executor rather than the binary: `apcli validate`
    // routes through `executor.call("system.validate", ...)` and falls back to
    // a synthetic three-check result when no host has registered the system
    // modules, so the standalone binary has no `acl` row to report. The row
    // itself comes from `Executor::validate`, which is what the CLI forwards
    // verbatim once a host registers them -- so that is what is measured here.
    let registry = Registry::new();
    registry
        .register_module("math.add", Box::new(AddModule))
        .expect("registers");
    let mut executor = Executor::new(std::sync::Arc::new(registry), Config::default());

    let permitted = executor
        .validate("math.add", &json!({"a": 1, "b": 2}), None)
        .await
        .expect("preflight is non-throwing");
    assert!(
        permitted.valid,
        "with no ACL attached the call passes preflight: {permitted:?}"
    );

    let mut rule = ACLRule::new(vec!["*".to_string()], vec!["math.add".to_string()], "deny");
    rule.description = Some("math.add is off limits".to_string());
    executor.set_acl(ACL::try_new(vec![rule], "allow", None).expect("well-formed"));

    let denied = executor
        .validate("math.add", &json!({"a": 1, "b": 2}), None)
        .await
        .expect("preflight is non-throwing");
    assert!(!denied.valid, "an ACL denial must fail preflight");
    let acl_row = denied
        .checks
        .iter()
        .find(|c| c.check == "acl")
        .expect("an `acl` check row must be present once an ACL is attached");
    assert!(!acl_row.passed, "the acl row must report the denial");

    // The CLI's cascade maps the FIRST failing check to an exit code, so the
    // acl row must be the one it reaches -- otherwise a denial would be
    // reported under some other code.
    let first_failed = denied
        .checks
        .iter()
        .find(|c| !c.passed)
        .expect("at least one check failed");
    assert_eq!(
        first_failed.check,
        "acl",
        "the acl row must be the first failure, so the cascade reports {}",
        apcore_cli::EXIT_ACL_DENIED
    );
}

/// Minimal in-process stand-in for the `math.add` example extension.
struct AddModule;

#[async_trait::async_trait]
impl Module for AddModule {
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"]
        })
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &str {
        "Add two numbers"
    }
    async fn execute(
        &self,
        inputs: Value,
        _ctx: &apcore::Context<Value>,
    ) -> Result<Value, apcore::errors::ModuleError> {
        let a = inputs.get("a").and_then(Value::as_f64).unwrap_or_default();
        let b = inputs.get("b").and_then(Value::as_f64).unwrap_or_default();
        Ok(json!({"sum": a + b}))
    }
}

// ---------------------------------------------------------------------------
// T-ACL-31 / T-ACL-32 / T-ACL-33 / T-ACL-34 -- section 4.10: every execution
// path is gated
// ---------------------------------------------------------------------------
//
// Attaching an ACL to the executor gates the calls that go through that
// executor and nothing else. Two CLI paths build their own: the `--sandbox`
// subprocess (whose runner constructs a fresh Registry + Executor with NO ACL)
// and filesystem script modules (spawned directly, never reaching
// `Executor::call`). Both are complete bypasses; the sandbox one inverts the
// user's intent, since `--sandbox` is a security flag.
//
// The observable used here is a sentinel file the module's own script creates.
// If the parent refuses before spawning, the sentinel cannot exist -- which is
// the property section 4.10 requires, and is not implied by the exit code
// alone.

/// Env var carrying the sentinel path. The `APCORE_` prefix matters: the
/// sandbox clears the child's environment except a narrow allowlist, and this
/// prefix is on it, so the value reaches the script on both paths.
const SENTINEL_ENV: &str = "APCORE_TEST_SENTINEL";

const TOUCH_MODULE_JSON: &str = r#"{
  "name": "demo.touch",
  "description": "Create a sentinel file, proving the process ran",
  "input_schema": {"type": "object", "properties": {"x": {"type": "integer"}}},
  "output_schema": {"type": "object"},
  "executable": "run.sh"
}"#;

const TOUCH_RUN_SH: &str = "#!/usr/bin/env bash\n\
                            : > \"${APCORE_TEST_SENTINEL:-/dev/null}\"\n\
                            echo '{\"ok\": true}'\n";

/// A project holding `demo.touch`, plus the absolute sentinel path its script
/// writes when -- and only when -- the process actually runs.
fn touch_project() -> (Project, PathBuf) {
    let project = Project::new();
    project.write("extensions/demo/touch/module.json", TOUCH_MODULE_JSON);
    let script = project.write("extensions/demo/touch/run.sh", TOUCH_RUN_SH);
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
    }
    std::fs::set_permissions(&script, perms).expect("chmod +x");
    let sentinel = project.path().join("SPAWNED");
    (project, sentinel)
}

const DENY_TOUCH: &str = "\
default_effect: allow
rules:
  - callers: ['*']
    targets: ['demo.touch']
    effect: deny
    description: demo.touch is off limits
";

const ALLOW_ALL: &str = "\
default_effect: deny
rules:
  - callers: ['*']
    targets: ['*']
    effect: allow
";

impl Project {
    /// Run with the sentinel env var pointing at `sentinel`.
    fn run_touch(&self, sentinel: &Path, args: &[&str]) -> Output {
        self.run_with_env(args, &[(SENTINEL_ENV, sentinel.to_str().expect("utf8"))])
    }
}

#[test]
fn t_acl_31_sandboxed_denied_module_exits_77_and_is_never_spawned() {
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH);

    let sandboxed = project.run_touch(&sentinel, &["demo.touch", "--x", "1", "--sandbox"]);
    assert_eq!(
        sandboxed.status.code(),
        Some(77),
        "--sandbox must not disable access control; stdout: {}\nstderr: {}",
        stdout(&sandboxed),
        stderr(&sandboxed)
    );
    assert!(
        !sentinel.exists(),
        "the decision must be reached in the parent, BEFORE the subprocess is \
         spawned -- the sentinel proves it ran"
    );
    assert!(
        stderr(&sandboxed).contains("Permission denied for module 'demo.touch'"),
        "stderr: {}",
        stderr(&sandboxed)
    );
}

#[test]
fn t_acl_31_discriminator_same_call_without_sandbox_also_exits_77() {
    // Half one of the discriminator: the sandboxed and unsandboxed forms must
    // agree. A gate on only one path would show up as a divergence here.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH);

    let plain = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(
        plain.status.code(),
        Some(77),
        "stdout: {}\nstderr: {}",
        stdout(&plain),
        stderr(&plain)
    );
    assert!(!sentinel.exists(), "the script must not have run");
}

#[test]
fn t_acl_31_discriminator_both_forms_succeed_once_the_rule_is_removed() {
    // Half two: with the deny rule gone, BOTH forms must run and create the
    // sentinel. Without this, a gate that refused everything unconditionally
    // would pass the two assertions above.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", ALLOW_ALL);

    let plain = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(
        plain.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&plain),
        stderr(&plain)
    );
    assert!(sentinel.exists(), "the allowed call must actually run");
    std::fs::remove_file(&sentinel).expect("reset sentinel");

    let sandboxed = project.run_touch(&sentinel, &["demo.touch", "--x", "1", "--sandbox"]);
    assert_eq!(
        sandboxed.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&sandboxed),
        stderr(&sandboxed)
    );
    assert!(
        sentinel.exists(),
        "the allowed sandboxed call must actually run"
    );
}

#[test]
fn t_acl_32_sandboxed_allowed_module_runs_normally() {
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", ALLOW_ALL);
    let out = project.run_touch(&sentinel, &["demo.touch", "--x", "1", "--sandbox"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).contains("\"ok\""), "{}", stdout(&out));
    assert!(sentinel.exists());
}

#[test]
fn t_acl_33_script_module_denied_by_acl_is_never_spawned() {
    // The FsDiscoverer path, stated as its own case: the exit code alone does
    // not prove the process was not started.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH);
    let out = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(out.status.code(), Some(77), "stderr: {}", stderr(&out));
    assert!(!sentinel.exists(), "the script must not have been spawned");
}

#[test]
fn t_acl_34_acl_sourced_approval_gates_a_subprocess_call() {
    // An ACL-sourced `approval: required` must compose with the annotation
    // before the CLI's approval gate on these paths too -- otherwise the same
    // rule would demand a human on an in-process call and wave through a
    // sandboxed or script one.
    //
    // `demo.touch` carries no `requires_approval` annotation, so only the ACL
    // can put it to a human. stdin is a pipe under `.output()`, so the handler
    // has no TTY and refuses deterministically.
    let (project, sentinel) = touch_project();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['demo.touch']\n    effect: allow\n    approval: required\n",
    );

    let refused = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(
        refused.status.code(),
        Some(46),
        "the ACL requires a human and none is available; stdout: {}\nstderr: {}",
        stdout(&refused),
        stderr(&refused)
    );
    assert!(
        !sentinel.exists(),
        "a call awaiting approval must not have run"
    );

    // Discriminating counterpart: `--yes` bypasses the prompt, so the same
    // call runs. Without this, a gate that simply refused everything would
    // pass the assertion above.
    let approved = project.run_touch(&sentinel, &["demo.touch", "--x", "1", "--yes"]);
    assert_eq!(
        approved.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&approved),
        stderr(&approved)
    );
    assert!(sentinel.exists());
}

/// A conditional deny. With no context the condition is a non-match
/// (PROTOCOL_SPEC 6.5), so the rule steps aside and `default_effect: allow`
/// lets the call through -- which is exactly the bypass a gate passing `None`
/// would reopen. An unconditional rule cannot catch this.
const DENY_TOUCH_CONDITIONAL: &str = "\
default_effect: allow
rules:
  - callers: ['*']
    targets: ['demo.touch']
    effect: deny
    description: external callers may not touch
    conditions:
      identity_types: ['external']
";

/// An `arguments`-scoped deny, which is inert unless the gate supplies the
/// governance projection of the call's arguments (PROTOCOL_SPEC 6.1.7).
const DENY_TOUCH_ARGUMENTS: &str = "\
default_effect: allow
rules:
  - callers: ['*']
    targets: ['demo.touch']
    effect: deny
    description: no calls carrying x
    conditions:
      arguments:
        has_key: ['x']
";

#[test]
fn t_acl_31_conditional_deny_fires_on_the_script_path() {
    // The gate must present a Context even when no identity flag was given:
    // apcore's pipeline creates one at Step 1 for every real call, so a gate
    // passing `None` leaves conditional deny rules inert on the delegated path
    // while they fire in-process.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH_CONDITIONAL);
    let out = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(
        out.status.code(),
        Some(77),
        "a conditional deny must fire without any identity flag; stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(!sentinel.exists(), "the script must not have been spawned");
}

#[test]
fn t_acl_31_conditional_deny_fires_on_the_sandbox_path() {
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH_CONDITIONAL);
    let out = project.run_touch(&sentinel, &["demo.touch", "--x", "1", "--sandbox"]);
    assert_eq!(
        out.status.code(),
        Some(77),
        "stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(!sentinel.exists());
}

#[test]
fn t_acl_31_conditional_deny_matches_the_in_process_verdict() {
    // The delegated paths must agree with what the same rule set says for an
    // in-process call. `demo.touch` is deliberately NOT a `system.*` id:
    // apcore's registry rejects that prefix as reserved, so the comparison
    // would fail with MODULE_NOT_FOUND before reaching the ACL check and
    // prove nothing.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH_CONDITIONAL);
    let simulated = project.run_touch(
        &sentinel,
        &[
            "apcli",
            "acl",
            "check",
            "demo.touch",
            "--identity-type",
            "external",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        simulated.status.code(),
        Some(77),
        "stderr: {}",
        stderr(&simulated)
    );
    assert_eq!(json_stdout(&simulated)["access"], "deny");
}

#[test]
fn t_acl_31_conditional_deny_discriminator_condition_that_does_not_match() {
    // With a condition the default gate context cannot satisfy, the rule must
    // step aside and the call must run -- otherwise the two tests above would
    // pass against a gate that simply denied everything conditional.
    let (project, sentinel) = touch_project();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: allow\nrules:\n  - callers: ['*']\n    targets: ['demo.touch']\n    effect: deny\n    conditions:\n      identity_types: ['service']\n",
    );
    let out = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the gate context is @external, not service; stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(sentinel.exists());
}

#[test]
fn t_acl_31_arguments_scoped_deny_fires_on_the_delegated_paths() {
    // The other half of the gate's inputs: without the governance projection
    // an `arguments` rule is unevaluable and goes inert the same way.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH_ARGUMENTS);

    let script = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(
        script.status.code(),
        Some(77),
        "stdout: {}\nstderr: {}",
        stdout(&script),
        stderr(&script)
    );
    assert!(!sentinel.exists());

    let sandboxed = project.run_touch(&sentinel, &["demo.touch", "--x", "1", "--sandbox"]);
    assert_eq!(
        sandboxed.status.code(),
        Some(77),
        "stdout: {}\nstderr: {}",
        stdout(&sandboxed),
        stderr(&sandboxed)
    );
    assert!(!sentinel.exists());
}

#[test]
fn t_acl_31_arguments_scoped_deny_discriminator_key_absent() {
    // `demo.touch` takes an optional `x`; omitting it means the projection
    // carries no `x` key, the rule does not match, and the call runs.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", DENY_TOUCH_ARGUMENTS);
    let out = project.run_touch(&sentinel, &["demo.touch"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );
    assert!(sentinel.exists());
}

#[test]
fn t_acl_34_a_module_without_an_acl_approval_rule_is_not_gated() {
    // The other direction: the annotation-composition must not gate calls the
    // ACL said nothing about.
    let (project, sentinel) = touch_project();
    project.write("acl/global_acl.yaml", ALLOW_ALL);
    let out = project.run_touch(&sentinel, &["demo.touch", "--x", "1"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(sentinel.exists());
}

// ---------------------------------------------------------------------------
// T-ACL-25 -- strategy bypass warning
// ---------------------------------------------------------------------------

#[test]
fn t_acl_25_testing_strategy_warns_that_a_configured_acl_is_not_enforced() {
    let project = Project::new();
    project.write(
        "acl/global_acl.yaml",
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: allow\n",
    );
    let ext = example_extensions();
    let out = project.run(&[
        "--extensions-dir",
        ext.to_str().expect("utf8 path"),
        "math.add",
        "--strategy",
        "testing",
        "--a",
        "1",
        "--b",
        "2",
    ]);
    let err = stderr(&out);
    assert!(
        err.contains("testing") && err.contains("configured ACL is not enforced"),
        "the banner must name the strategy and say *configured*; stderr: {err}"
    );
}

#[test]
fn no_bypass_warning_without_an_attached_acl() {
    // The warning is about bypassing a *real* rule set; with none attached it
    // must stay silent, or it becomes noise on every testing run.
    let project = Project::new();
    let ext = example_extensions();
    let out = project.run(&[
        "--extensions-dir",
        ext.to_str().expect("utf8 path"),
        "math.add",
        "--strategy",
        "testing",
        "--a",
        "1",
        "--b",
        "2",
    ]);
    assert!(
        !stderr(&out).contains("configured ACL is not enforced"),
        "stderr: {}",
        stderr(&out)
    );
    // And the emitter itself is a no-op on both quiet paths.
    warn_if_strategy_bypasses_acl(Some("testing"), false);
    warn_if_strategy_bypasses_acl(Some("standard"), true);
    warn_if_strategy_bypasses_acl(None, true);
}

// ---------------------------------------------------------------------------
// T-ACL-24 -- ACL-sourced approval reaches the CLI handler
// ---------------------------------------------------------------------------
//
// The full discriminating case (handler refuses, non-TTY, call denied) lives
// in `tests/acl_argument_scoped_approval.rs`, which predates FE-14 and already
// drives an ACL rule carrying `approval: required` against a module annotated
// `requires_approval: false`. This test pins the composition FE-14 relies on:
// an ACL attached by the CLI loader routes such a call to the approval gate.

#[test]
fn t_acl_24_acl_sourced_approval_is_visible_in_preflight() {
    let dir = tempfile::tempdir().expect("tempdir");
    let acl_path = dir.path().join("global_acl.yaml");
    std::fs::write(
        &acl_path,
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['git.push']\n    effect: allow\n    approval: required\n",
    )
    .expect("write");

    let acl = load_cli_acl(acl_path.to_str().unwrap())
        .expect("loads")
        .expect("attached");
    let decision = acl.check_access(Some("@external"), "git.push", None, None);
    assert_eq!(decision.access, "allow");
    assert!(
        decision.approval_required,
        "an ACL rule carrying approval: required must raise the approval axis \
         even for a module whose annotation says otherwise"
    );
    assert_eq!(check_exit_code(&decision), 0);
}

// ---------------------------------------------------------------------------
// Exit-code taxonomy
// ---------------------------------------------------------------------------

#[test]
fn exit_code_helpers_match_the_spec_table() {
    let allow = apcore::AccessDecision::new("allow", false, Some(0), "rule_match");
    let allow_with_approval = apcore::AccessDecision::new("allow", true, Some(0), "rule_match");
    let deny = apcore::AccessDecision::new("deny", false, Some(0), "rule_match");
    assert_eq!(check_exit_code(&allow), 0);
    assert_eq!(
        check_exit_code(&allow_with_approval),
        0,
        "the approval axis must not leak into the exit code"
    );
    assert_eq!(check_exit_code(&deny), 77);
    assert_eq!(validate_exit_code(0), 0);
    assert_eq!(validate_exit_code(1), 47);
    assert_eq!(validate_exit_code(9), 47);
}

#[test]
fn acl_rule_error_maps_to_47_not_77() {
    // T-ACL-30's Rust half: the exit map carries ACL_RULE_ERROR, and it is
    // distinct from the ACL_DENIED code.
    assert_eq!(apcore_cli::EXIT_ACL_RULE_ERROR, 47);
    assert_eq!(apcore_cli::EXIT_ACL_DENIED, 77);
    assert_ne!(apcore_cli::EXIT_ACL_RULE_ERROR, apcore_cli::EXIT_ACL_DENIED);
}

// ---------------------------------------------------------------------------
// Registration (spec 4.10)
// ---------------------------------------------------------------------------

#[test]
fn acl_is_registered_under_the_apcli_group() {
    let group = apcore_cli::register_apcli_subcommands(
        clap::Command::new("apcli"),
        &apcore_cli::ApcliGroup::from_yaml(None, /*registry_injected*/ false),
        "apcore-cli",
    );
    let acl = group
        .get_subcommands()
        .find(|c| c.get_name() == "acl")
        .expect("acl must be registered under apcli");
    let subs: Vec<&str> = acl.get_subcommands().map(|c| c.get_name()).collect();
    for expected in ["list", "check", "validate", "status"] {
        assert!(subs.contains(&expected), "missing '{expected}': {subs:?}");
    }
}

#[test]
fn acl_is_not_always_registered() {
    // Spec 4.10: `acl` is NOT in _ALWAYS_REGISTERED; under `mode: include` it
    // registers only when explicitly listed.
    assert!(!apcore_cli::APCLI_ALWAYS_REGISTERED.contains(&"acl"));
    let cfg = apcore_cli::ApcliGroup::from_cli_config(
        Some(apcore_cli::ApcliConfig {
            mode: apcore_cli::ApcliMode::Include(vec!["list".to_string()]),
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let group =
        apcore_cli::register_apcli_subcommands(clap::Command::new("apcli"), &cfg, "apcore-cli");
    let names: Vec<&str> = group.get_subcommands().map(|c| c.get_name()).collect();
    assert!(!names.contains(&"acl"), "got {names:?}");

    let listed_cfg = apcore_cli::ApcliGroup::from_cli_config(
        Some(apcore_cli::ApcliConfig {
            mode: apcore_cli::ApcliMode::Include(vec!["acl".to_string()]),
            disable_env: true,
        }),
        /*registry_injected*/ false,
    );
    let listed = apcore_cli::register_apcli_subcommands(
        clap::Command::new("apcli"),
        &listed_cfg,
        "apcore-cli",
    );
    let listed_names: Vec<&str> = listed.get_subcommands().map(|c| c.get_name()).collect();
    assert!(listed_names.contains(&"acl"), "got {listed_names:?}");
}

// ---------------------------------------------------------------------------
// Identity flags are assertions, not authentication (spec 4.3 / 7)
// ---------------------------------------------------------------------------

#[test]
fn identity_flag_help_states_it_is_not_authentication() {
    let project = Project::new();
    let out = project.run(&["--help"]);
    let rendered = stdout(&out);
    assert!(rendered.contains("--identity-id"), "{rendered}");
    assert!(rendered.contains("--identity-type"), "{rendered}");
    assert!(rendered.contains("--role"), "{rendered}");
    assert!(
        rendered.contains("not authentication"),
        "each identity flag must document the caveat: {rendered}"
    );
}

#[test]
fn no_flag_sets_a_caller_id() {
    // Spec 7 rule 2: real execution always presents @external, and the CLI
    // must expose no flag that sets Context::caller_id.
    let project = Project::new();
    let rendered = stdout(&project.run(&["--help", "--all-options"]));
    assert!(
        !rendered.contains("--caller-id"),
        "the CLI must not offer a caller_id flag: {rendered}"
    );
}

#[test]
fn approval_requirement_is_representable_only_on_allow_rules() {
    // Guard on the fixture's assumption: apcore refuses `approval: required`
    // on a deny rule, so the three-rule fixture cannot silently drift into an
    // unrepresentable state.
    let mut bad = ACLRule::new(vec!["*".to_string()], vec!["x".to_string()], "deny");
    bad.approval = Some(ApprovalRequirement::Required);
    assert!(
        ACL::try_new(vec![bad], "deny", None).is_err(),
        "approval: required on a deny rule must be refused at load"
    );
}

// ---------------------------------------------------------------------------
// T-ACL-26 / 27 / 27a / 27b / 27c -- section 4.8 audit wiring
// ---------------------------------------------------------------------------
//
// Driven in-process rather than through the binary, deliberately. The
// production audit path writes to `~/.apcore-cli/audit.jsonl`, and a test that
// spawns the real CLI with auditing on would append to the developer's own log
// -- so every other test in this file sets APCORE_CLI_AUDIT_DISABLE=1. These
// tests instead exercise the exact function `main.rs` calls
// (`load_cli_acl_with_audit`) with an `AuditLogger` pointed at a temp file,
// which is where all of section 4.8's observable behaviour lives.

/// An ACL denying `system.control.*` and allowing `db.read`, so one fixture
/// can produce both a deny and an allow decision.
const AUDIT_ACL: &str = "\
default_effect: deny
rules:
  - callers: ['@external']
    targets: ['system.control.*']
    effect: deny
    description: no external control
  - callers: ['*']
    targets: ['db.read']
    effect: allow
";

/// The same rules over a permissive default, for T-ACL-27b.
const AUDIT_ACL_DEFAULT_ALLOW: &str = "\
default_effect: allow
rules:
  - callers: ['@external']
    targets: ['system.control.*']
    effect: deny
    description: no external control
";

/// A scratch project holding an ACL file, an optional `apcore.yaml`, and the
/// audit log the section 4.8 callback writes to.
struct AuditProject {
    dir: tempfile::TempDir,
}

impl AuditProject {
    fn new(acl_yaml: &str, apcore_yaml: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("global_acl.yaml"), acl_yaml).expect("write acl");
        if let Some(contents) = apcore_yaml {
            std::fs::write(dir.path().join("apcore.yaml"), contents).expect("write apcore.yaml");
        }
        Self { dir }
    }

    fn acl_path(&self) -> String {
        self.dir
            .path()
            .join("global_acl.yaml")
            .to_string_lossy()
            .to_string()
    }

    /// A resolver over this project's `apcore.yaml`, matching how `main.rs`
    /// builds one before calling `load_cli_acl_with_audit`.
    fn resolver(&self) -> ConfigResolver {
        let config = self.dir.path().join("apcore.yaml");
        if config.exists() {
            ConfigResolver::new(None, Some(config))
        } else {
            ConfigResolver::new(None, None)
        }
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("audit.jsonl")
    }

    fn logger(&self) -> AuditLogger {
        AuditLogger::new(Some(self.log_path()))
    }

    /// Every line of the audit log, parsed. Empty when nothing was written.
    fn entries(&self) -> Vec<Value> {
        let Ok(raw) = std::fs::read_to_string(self.log_path()) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                serde_json::from_str(l).unwrap_or_else(|e| panic!("bad JSONL line ({e}): {l}"))
            })
            .collect()
    }

    /// Raw log lines, for the assertions that care about byte order.
    fn raw_lines(&self) -> Vec<String> {
        let Ok(raw) = std::fs::read_to_string(self.log_path()) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// Whether an audit callback is installed on `acl`.
///
/// apcore keeps `ACL::audit_logger` private and offers no accessor, but its
/// `Debug` impl renders the slot as `audit_logger: None` or
/// `audit_logger: Some("...")`. T-ACL-27a asks for "no logger was installed",
/// not merely "no entries were written" -- the two differ, and only this
/// distinguishes them.
fn has_audit_logger(acl: &ACL) -> bool {
    let rendered = format!("{acl:?}");
    if rendered.contains("audit_logger: None") {
        return false;
    }
    assert!(
        rendered.contains(r#"audit_logger: Some("#),
        "apcore's ACL Debug impl no longer renders the audit_logger slot; \
         this helper is the only way the tests can see it:\n{rendered}"
    );
    true
}

/// Top-level object keys of one JSON line, in written order.
///
/// Section 4.8 pins key order, and `serde_json::Value` cannot report it
/// portably (its object map is a `BTreeMap` unless `preserve_order` happens to
/// be enabled somewhere in the dependency graph), so the order is read off the
/// raw text.
fn ordered_keys(line: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut pending: Option<String> = None;
    let mut depth: i32 = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // Consume the whole string, so a colon inside a value (an
                // RFC 3339 timestamp, say) is never read as a separator.
                let mut s = String::new();
                while let Some(ch) = chars.next() {
                    match ch {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => s.push(ch),
                    }
                }
                pending = Some(s);
            }
            ':' if depth == 1 => {
                if let Some(key) = pending.take() {
                    keys.push(key);
                }
            }
            '{' | '[' => {
                depth += 1;
                pending = None;
            }
            '}' | ']' => {
                depth -= 1;
                pending = None;
            }
            _ => {}
        }
    }
    keys
}

#[test]
fn t_acl_26_a_denied_call_writes_one_entry_with_all_13_fields() {
    // T-ACL-26: `acl.audit.enabled: true`, run a denied call -> one AuditEntry
    // in the audit log with `decision: deny`, all 13 fields.
    let project = AuditProject::new(AUDIT_ACL, None);
    let resolver = project.resolver();
    assert!(
        acl_audit_enabled(&resolver),
        "auditing is on by default; this test's premise"
    );

    let acl = load_cli_acl_with_audit(&project.acl_path(), &resolver, Some(project.logger()))
        .expect("loads")
        .expect("attached");
    assert!(has_audit_logger(&acl));

    let decision = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(decision.access, "deny");

    let entries = project.entries();
    assert_eq!(entries.len(), 1, "one AuditEntry per check_access call");
    assert_eq!(entries[0]["decision"], "deny");
    assert_eq!(entries[0]["caller_id"], "@external");
    assert_eq!(entries[0]["target_id"], "system.control.disable");
    assert_eq!(entries[0]["reason"], "rule_match");
    assert_eq!(entries[0]["matched_rule_index"], 0);
    assert_eq!(entries[0]["matched_rule"], "no external control");

    // Field set, order and casing in one assertion: an equality check on the
    // ordered key list. Containment would miss a re-ordering, a renamed field,
    // and a CLI-added extra alike.
    let lines = project.raw_lines();
    assert_eq!(
        ordered_keys(&lines[0]),
        ACL_AUDIT_FIELDS,
        "the 13 section-4.8 fields, in apcore's AuditEntry declaration order"
    );

    // The optionals a `None` would have dropped are present as null, and the
    // two the spec calls out by name are present at all.
    assert!(entries[0]["handler_error"].is_null());
    assert_eq!(entries[0]["approval_required"], json!(false));
    assert!(entries[0]["identity_type"].is_null());
    assert!(entries[0]["trace_id"].is_null());
    assert_eq!(entries[0]["roles"], json!([]));
}

#[test]
fn t_acl_27_include_denied_false_drops_the_deny_and_keeps_the_allow() {
    // T-ACL-27: `acl.audit.include_denied: false`, run a denied call and an
    // allowed call -> the deny entry is absent; the allow entry is written.
    //
    // The discriminating half is the allow entry. A reading that inverted the
    // key ("include_denied: false means log denials only") would write exactly
    // the opposite line and still produce a one-entry log.
    let project = AuditProject::new(
        AUDIT_ACL,
        Some("acl:\n  audit:\n    enabled: true\n    include_denied: false\n"),
    );
    let resolver = project.resolver();
    assert!(acl_audit_enabled(&resolver));
    assert!(!acl_audit_include_denied(&resolver));

    let acl = load_cli_acl_with_audit(&project.acl_path(), &resolver, Some(project.logger()))
        .expect("loads")
        .expect("attached");

    let denied = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(denied.access, "deny");
    let allowed = acl.check_access(Some("@external"), "db.read", None, None);
    assert_eq!(allowed.access, "allow");

    let entries = project.entries();
    assert_eq!(
        entries.len(),
        1,
        "the deny is suppressed and the allow is kept: {entries:?}"
    );
    assert_eq!(entries[0]["decision"], "allow");
    assert_eq!(entries[0]["target_id"], "db.read");
    assert!(
        !entries
            .iter()
            .any(|e| e["target_id"] == "system.control.disable"),
        "no deny entry may reach the log: {entries:?}"
    );
}

#[test]
fn t_acl_27a_disabled_installs_no_logger_and_does_not_rebuild() {
    // T-ACL-27a: `acl.audit.enabled: false` -> no audit callback installed and
    // no rebuild; the ACL from `ACL::load` is attached directly; no ACL entries
    // written for either outcome.
    let project = AuditProject::new(AUDIT_ACL, Some("acl:\n  audit:\n    enabled: false\n"));
    let resolver = project.resolver();
    assert!(!acl_audit_enabled(&resolver));

    let mut acl = load_cli_acl_with_audit(&project.acl_path(), &resolver, Some(project.logger()))
        .expect("loads")
        .expect("attached");

    // The point of the row: no *logger*, asserted directly. "No entries were
    // written" would also pass against a callback that silently dropped every
    // entry, which is a different (and worse) implementation.
    assert!(
        !has_audit_logger(&acl),
        "acl.audit.enabled: false must install no callback at all"
    );

    // "and no rebuild": `ACL::new` drops the `yaml_path` that only `ACL::load`
    // sets, and `reload()` is the observable that depends on it. A rebuilt ACL
    // would fail here with ACLRuleError("Cannot reload: ...").
    assert!(
        acl.reload().is_ok(),
        "the ACL from ACL::load must be attached directly, keeping reload()"
    );

    let denied = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(denied.access, "deny");
    let allowed = acl.check_access(Some("@external"), "db.read", None, None);
    assert_eq!(allowed.access, "allow");
    assert!(
        project.entries().is_empty(),
        "no ACL entries for either outcome: {:?}",
        project.entries()
    );
}

#[test]
fn t_acl_27b_default_effect_allow_survives_audit_wiring() {
    // T-ACL-27b: an ACL file declaring `default_effect: allow`, with
    // `acl.audit.enabled: true` -> the attached ACL's default_effect is still
    // `allow`, and a call matching no rule is permitted.
    //
    // Discriminating for section 4.8 requirement 1. This SDK attaches via
    // `ACL::set_audit_logger`, so the file's default_effect is never re-stated
    // and the defect is structurally unreachable -- the test is written anyway,
    // to pin the behaviour against a future refactor to the rebuild form, where
    // a literal "deny" would invert the governing default silently and every
    // fixture using a deny-defaulted file would still pass.
    let project = AuditProject::new(AUDIT_ACL_DEFAULT_ALLOW, None);
    let resolver = project.resolver();
    assert!(
        acl_audit_enabled(&resolver),
        "the auditing path is the one under test"
    );

    let acl = load_cli_acl_with_audit(&project.acl_path(), &resolver, Some(project.logger()))
        .expect("loads")
        .expect("attached");
    assert!(
        has_audit_logger(&acl),
        "a rebuild-form regression would only be caught on the auditing path"
    );

    assert_eq!(
        acl.default_effect(),
        "allow",
        "the file's default_effect must reach the attached ACL unchanged"
    );

    let decision = acl.check_access(Some("@external"), "nothing.matches.this", None, None);
    assert_eq!(
        decision.access, "allow",
        "a call matching no rule falls through to the file's own default_effect"
    );
    assert_eq!(decision.reason, "default_effect");
    assert_eq!(decision.matched_rule_index, None);

    // The rules themselves survive too: the deny rule still denies.
    let denied = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(denied.access, "deny");

    // And both decisions were audited, allow included.
    let entries = project.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["decision"], "allow");
    assert_eq!(entries[0]["reason"], "default_effect");
    assert_eq!(entries[1]["decision"], "deny");
}

#[test]
fn t_acl_27c_an_embedder_supplied_acl_is_attached_unchanged() {
    // T-ACL-27c: an embedder supplies its own ACL with auditing enabled -> the
    // supplied instance is attached unchanged, not rebuilt, so it retains
    // reload().
    //
    // Rust language note: this SDK has no `create_cli(acl=...)` factory (see
    // lib.rs on the embed-API parity gap), so an embedder attaches its own ACL
    // by calling `Executor::set_acl` directly -- a path that never reaches
    // `load_cli_acl_with_audit` and so can acquire neither a CLI callback nor a
    // rebuild. Pointer identity is also not expressible here: `set_acl` moves
    // the ACL and re-wraps it in a fresh `Arc`, so *every* implementation
    // changes the pointer. The observables section 8 names -- "not rebuilt",
    // "retains reload()" -- are asserted instead, on the very instance the
    // executor ends up holding.
    let project = AuditProject::new(AUDIT_ACL, None);
    let resolver = project.resolver();
    assert!(
        acl_audit_enabled(&resolver),
        "auditing is on: the config that would wire a callback is in force"
    );

    // The embedder's own ACL, built the way an embedder would.
    let embedder_acl = ACL::load(&project.acl_path()).expect("the embedder loads its own ACL");
    assert!(!has_audit_logger(&embedder_acl));

    let mut executor = Executor::new(std::sync::Arc::new(Registry::new()), Config::default());
    executor.set_acl(embedder_acl);

    // Recover the exact instance the executor holds. Nothing else cloned the
    // Arc, so this is that instance and not a copy of it.
    let held = executor.acl.take().expect("an ACL is attached");
    let mut held = std::sync::Arc::try_unwrap(held)
        .expect("the executor holds the only reference to the supplied ACL");

    assert!(
        !has_audit_logger(&held),
        "an embedder-supplied ACL must not acquire the CLI's audit callback"
    );
    assert_eq!(held.default_effect(), "deny");
    assert_eq!(held.rules().len(), 2);
    assert!(
        held.reload().is_ok(),
        "attached unchanged, so the yaml_path provenance -- and reload() -- survives"
    );
    assert!(
        project.entries().is_empty(),
        "nothing the embedder supplied may write to the CLI audit log"
    );

    // Discriminating half: the same config, taken through the CLI's own load
    // path, *does* get a callback. Without this the assertions above would
    // pass against an implementation that never wires anything at all.
    let cli_loaded =
        load_cli_acl_with_audit(&project.acl_path(), &resolver, Some(project.logger()))
            .expect("loads")
            .expect("attached");
    assert!(has_audit_logger(&cli_loaded));
}

#[test]
fn no_fe_05_logger_means_no_callback_and_no_write() {
    // Section 4.8, as amended: with no FE-05 logger installed the callback
    // writes nothing and fails silently. FE-05 auditing is off process-wide
    // (APCORE_CLI_AUDIT_DISABLE=1 in main.rs), so there is no log to write to
    // and no callback is installed either.
    let project = AuditProject::new(AUDIT_ACL, None);
    let acl = load_cli_acl_with_audit(&project.acl_path(), &project.resolver(), None)
        .expect("loads")
        .expect("attached");
    assert!(!has_audit_logger(&acl));
    let decision = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(
        decision.access, "deny",
        "the decision is unaffected by having no audit sink"
    );
    assert!(project.entries().is_empty());
}

#[test]
fn an_installed_callback_over_an_unopenable_sink_stays_silent() {
    // The other half of the same rule: a callback IS installed, over an
    // AuditLogger whose path can never be opened (the empty path). It must
    // no-op silently rather than panic or propagate.
    let project = AuditProject::new(AUDIT_ACL, None);
    let mut acl = load_cli_acl(&project.acl_path())
        .expect("loads")
        .expect("attached");
    install_acl_audit_logger(&mut acl, AuditLogger::new(Some(PathBuf::new())), true);
    assert!(has_audit_logger(&acl));

    let decision = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(decision.access, "deny");
    assert!(project.entries().is_empty());
}

#[test]
fn a_logging_fault_does_not_change_the_access_decision() {
    // Section 4.8, as amended: a logging fault MUST NOT change an access
    // decision. The callback swallows its own errors and the decision stands.
    //
    // Fault injection: point the AuditLogger at a path that is a directory, so
    // every append fails with EISDIR.
    let project = AuditProject::new(AUDIT_ACL, None);
    let unwritable = project.dir.path().join("audit-is-a-directory");
    std::fs::create_dir(&unwritable).expect("mkdir");

    let mut acl = load_cli_acl(&project.acl_path())
        .expect("loads")
        .expect("attached");
    install_acl_audit_logger(&mut acl, AuditLogger::new(Some(unwritable)), true);

    // Both axes still answer exactly as they would with auditing off.
    let denied = acl.check_access(Some("@external"), "system.control.disable", None, None);
    assert_eq!(denied.access, "deny");
    assert_eq!(check_exit_code(&denied), 77);
    let allowed = acl.check_access(Some("@external"), "db.read", None, None);
    assert_eq!(allowed.access, "allow");
    assert_eq!(check_exit_code(&allowed), 0);
}
