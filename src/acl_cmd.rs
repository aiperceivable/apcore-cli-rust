// apcore-cli -- `apcli acl` command group (FE-14 sections 4.3-4.7).
//
// Four read-only surfaces over the ACL attached to the CLI's executor:
//
// * `list`     -- render the rule set and its `default_effect` in definition
//                 order, which is also evaluation order (first-match-wins).
// * `check`    -- evaluate a *simulated* call through `ACL::check_access`.
// * `validate` -- report every `RuleValidationFinding` from
//                 `ACL::validate_rules`.
// * `status`   -- render `Executor::governance_state`.
//
// Rule authoring (`add` / `remove`) is deliberately out of scope for v1 (spec
// section 9 question 2): `ACL::add_rule` mutates only the in-memory ACL, so a
// CLI `add` that did not persist would mislead.

use std::sync::Mutex;

use apcore::acl::{GovernanceProjection, ACL};
use apcore::{Context, Identity};
use clap::{Arg, ArgAction, Command};
use serde_json::{Map, Value};

use crate::{EXIT_ACL_DENIED, EXIT_CONFIG_NOT_FOUND, EXIT_INVALID_INPUT, EXIT_SUCCESS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The effective caller of a top-level CLI invocation.
///
/// apcore deliberately makes `Context::caller_id` unsettable by callers -- it
/// is managed exclusively by `Context::child()` -- so a top-level CLI call is
/// always `@external`. The CLI MUST NOT fabricate a `caller_id`; doing so
/// would let any user assume any module's identity by passing a flag (spec
/// section 4.3 / section 7 rule 2).
pub const DEFAULT_CALLER: &str = apcore::acl::EXTERNAL_CALLER;

/// Default `Identity.type` when `--identity-id` is given without
/// `--identity-type`.
pub const DEFAULT_IDENTITY_TYPE: &str = "user";

/// Default `Identity.id` when `--role` or `--identity-type` is given without
/// `--identity-id`.
///
/// Spec section 4.3 permits the roles-only form, but apcore's `Identity`
/// requires an id, so the CLI supplies one. The `@` prefix follows apcore's
/// convention for synthetic principals (`@external`, `@system`) so the value
/// cannot be mistaken for a real user whose id happens to be `cli`.
///
/// This is `Identity.id`, **not** `caller_id`: it feeds no built-in condition
/// and gets no special pattern-matching treatment. The prefix is for
/// legibility and collision-avoidance only. Pinned across all three SDKs.
pub const DEFAULT_IDENTITY_ID: &str = "@cli";

/// Strategies whose step list omits the built-in `acl_check` gate. Passing one
/// of these with an ACL attached is a materially different event from running
/// with no rules at all, so it is warned about (spec section 6.2).
pub const ACL_BYPASSING_STRATEGIES: &[&str] = &["internal", "testing", "minimal"];

// ---------------------------------------------------------------------------
// Identity flags (section 4.3)
// ---------------------------------------------------------------------------

/// The unauthenticated caller assertion built from `--identity-id`,
/// `--identity-type` and `--role`.
///
/// These flags are **not authentication**. They are argv values, exactly like
/// `--caller` on `apcli acl check`, and they are useful for *evaluating* a
/// rule set locally. A deployment that needs the identity to be trustworthy
/// must supply it through FE-05 auth, not through argv (spec section 7 rule
/// 1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliIdentity {
    pub id: Option<String>,
    pub identity_type: Option<String>,
    pub roles: Vec<String>,
}

impl CliIdentity {
    /// `true` when none of the three flags was supplied, in which case no
    /// `Identity` is constructed at all and conditional rules keyed on `roles`
    /// or `identity_types` simply do not match.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.id.is_none() && self.identity_type.is_none() && self.roles.is_empty()
    }

    /// Build the apcore `Identity` this assertion describes.
    ///
    /// `Identity.id` falls back to [`DEFAULT_IDENTITY_ID`] when only `--role`
    /// or `--identity-type` was given, because apcore's `Identity` requires an
    /// id while spec section 4.3 permits the roles-only form. The sentinel is
    /// not a `caller_id` and grants nothing on its own.
    #[must_use]
    pub fn to_identity(&self) -> Identity {
        Identity::new(
            self.id
                .clone()
                .unwrap_or_else(|| DEFAULT_IDENTITY_ID.to_string()),
            self.identity_type
                .clone()
                .unwrap_or_else(|| DEFAULT_IDENTITY_TYPE.to_string()),
            self.roles.clone(),
            std::collections::HashMap::new(),
        )
    }
}

/// Process-global identity assertion, published once by the binary entry point
/// after argv is parsed and read by every path that builds a `Context`.
///
/// Mirrors the `AUDIT_LOGGER` / `ALL_OPTIONS_HELP` cells in `cli.rs`: the
/// value is decided once at startup and consulted from call sites that have no
/// other channel to receive it.
static CLI_IDENTITY: Mutex<Option<CliIdentity>> = Mutex::new(None);

/// Publish the identity assertion parsed from argv. `None` clears it.
pub fn set_cli_identity(identity: Option<CliIdentity>) {
    if let Ok(mut guard) = CLI_IDENTITY.lock() {
        *guard = identity.filter(|i| !i.is_empty());
    }
}

/// Read the published identity assertion, if any.
#[must_use]
pub fn cli_identity() -> Option<CliIdentity> {
    CLI_IDENTITY.lock().ok().and_then(|g| g.clone())
}

/// Build the `Context` the CLI hands to `Executor::call` / `Executor::validate`.
///
/// Returns `None` when no identity flag was given, so `apcore` constructs its
/// own default context exactly as it does today -- the pre-FE-14 behaviour.
#[must_use]
pub fn identity_context() -> Option<Context<Value>> {
    let identity = cli_identity()?;
    Some(Context::<Value>::new(identity.to_identity()))
}

/// The `Context` a **delegated-execution gate** must present (spec section
/// 4.10).
///
/// Never `None`. PROTOCOL_SPEC section 6.5 makes every conditional rule a
/// non-match when a call supplies no context, while apcore's pipeline creates
/// one at Step 1 for *every* real call. A gate passing `None` would therefore
/// leave conditional `deny` rules inert on the delegated path while they fire
/// in-process -- the same silent bypass section 4.10 exists to close, one
/// level down.
///
/// With no identity flag the fallback reproduces exactly what
/// `Executor::call` builds for `ctx: None`: the `@external` caller with
/// identity type `external`, so an `identity_types` rule behaves identically
/// on both paths.
#[must_use]
pub fn delegated_gate_context() -> Context<Value> {
    match cli_identity() {
        Some(identity) => Context::<Value>::new(identity.to_identity()),
        None => Context::<Value>::new(Identity::new(
            DEFAULT_CALLER.to_string(),
            "external".to_string(),
            Vec::new(),
            std::collections::HashMap::new(),
        )),
    }
}

/// Help text for `--identity-id`. Normative across all three SDKs (spec
/// section 4.3): the `apcli-visibility` conformance fixtures byte-match root
/// `--help`, and spec section 7 rule 1 requires the
/// "Unauthenticated assertion, not authentication." clause.
pub const IDENTITY_ID_HELP: &str =
    "Assert Identity.id for ACL conditions. Unauthenticated assertion, not authentication.";

/// Help text for `--identity-type`. Normative -- see [`IDENTITY_ID_HELP`].
pub const IDENTITY_TYPE_HELP: &str = "Assert Identity.type for ACL conditions (default: user). \
     Unauthenticated assertion, not authentication.";

/// Help text for `--role`. Normative -- see [`IDENTITY_ID_HELP`].
pub const ROLE_HELP: &str = "Assert an Identity role for ACL conditions. Repeatable. \
     Unauthenticated assertion, not authentication.";

/// The three identity flags, built once so the root and `apcli acl check`
/// copies cannot drift in wording or metavar.
///
/// `global` is `true` at the root (so `apcli exec`, `apcli validate` and
/// business-module dispatch see them) and `false` on `acl check`, which
/// declares its own copies per spec section 4.5.
fn identity_args(global: bool) -> [Arg; 3] {
    [
        Arg::new("identity-id")
            .long("identity-id")
            .global(global)
            .value_name("ID")
            .help(IDENTITY_ID_HELP),
        Arg::new("identity-type")
            .long("identity-type")
            .global(global)
            .value_name("TYPE")
            .help(IDENTITY_TYPE_HELP),
        Arg::new("role")
            .long("role")
            .global(global)
            .action(ArgAction::Append)
            .value_name("ROLE")
            .help(ROLE_HELP),
    ]
}

/// Attach `--identity-id`, `--identity-type` and `--role` as global root
/// flags.
///
/// They are global so that `apcli exec`, `apcli validate` and business-module
/// dispatch see the same values. `apcli acl check` additionally declares its
/// own non-global copies (spec section 4.5); clap resolves the two levels
/// **per argument**, so a subcommand-level flag overrides only its own
/// counterpart and a root flag not restated at the subcommand level still
/// applies.
pub fn apply_identity_flags(cmd: Command) -> Command {
    cmd.args(identity_args(/*global*/ true))
}

/// Read the three identity flags out of parsed matches.
#[must_use]
pub fn identity_from_matches(matches: &clap::ArgMatches) -> CliIdentity {
    CliIdentity {
        id: matches.get_one::<String>("identity-id").cloned(),
        identity_type: matches.get_one::<String>("identity-type").cloned(),
        roles: matches
            .get_many::<String>("role")
            .map(|vals| vals.cloned().collect())
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Strategy bypass warning (section 6.2)
// ---------------------------------------------------------------------------

/// Warn on stderr when a strategy that omits `acl_check` is selected while an
/// ACL is attached.
///
/// The wording says *configured* on purpose: bypassing a real rule set is not
/// the same event as running with no rules at all. The spec renders the banner
/// with a warning sign glyph; this crate is ASCII-only (`make check-chars`), so
/// the marker is spelled `Warning:`.
pub fn warn_if_strategy_bypasses_acl(strategy: Option<&str>, acl_attached: bool) {
    if !acl_attached {
        return;
    }
    if let Some(name) = strategy {
        if ACL_BYPASSING_STRATEGIES.contains(&name) {
            eprintln!("Warning: Using '{name}' strategy - the configured ACL is not enforced.");
        }
    }
}

// ---------------------------------------------------------------------------
// Command builder
// ---------------------------------------------------------------------------

/// Build the `acl` clap subcommand group.
///
/// **API divergence (matching `register_pipeline_command`):** the spec writes
/// this registrar as `register_acl_command(apcli_group, executor, acl)`. The
/// Rust registrar table in `lib.rs` is a `FnOnce(Command) -> Command` table of
/// static clap metadata with no executor in scope; the executor and the loaded
/// ACL are consumed at dispatch time by [`dispatch_acl`] instead, which is the
/// same shape `describe-pipeline` already uses.
pub fn acl_command() -> Command {
    Command::new("acl")
        .about("Inspect and lint the configured access-control rules")
        .subcommand(
            Command::new("list")
                .about("List the attached rule set and its default effect")
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["table", "json", "csv", "yaml", "jsonl"])
                        .value_name("FORMAT")
                        .help("Output format."),
                ),
        )
        .subcommand(
            Command::new("check")
                .about("Evaluate a simulated call against the rule set (executes nothing)")
                .arg(
                    Arg::new("target")
                        .required(true)
                        .value_name("TARGET")
                        .help("Target module ID."),
                )
                // Spec section 4.5: `acl check` carries its own copies of the
                // identity triple, worded identically to the root flags, so
                // the same flag cannot read two different ways inside one CLI.
                // clap merges the two levels per argument.
                .args(identity_args(/*global*/ false))
                .arg(Arg::new("caller").long("caller").value_name("ID").help(
                    "Simulated caller ID (default: @external). \
                     Nothing is executed, so any value is accepted.",
                ))
                .arg(
                    Arg::new("depth")
                        .long("depth")
                        .value_name("N")
                        .help("Simulated call-chain depth for the max_call_depth condition."),
                )
                .arg(Arg::new("input").long("input").value_name("JSON").help(
                    "Argument map for the arguments condition. \
                     Key presence only; values are not compared.",
                ))
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["table", "json"])
                        .value_name("FORMAT")
                        .help("Output format."),
                ),
        )
        .subcommand(
            Command::new("validate")
                .about("Report every rule-validation finding in the attached rule set")
                .arg(
                    Arg::new("format")
                        .long("format")
                        .value_parser(["table", "json"])
                        .value_name("FORMAT")
                        .help("Output format."),
                ),
        )
        .subcommand(
            Command::new("status")
                .about("Show what is actually gating the registry")
                .arg(
                    Arg::new("strict")
                        .long("strict")
                        .action(ArgAction::SetTrue)
                        .help("Exit 47 when the control surface is unprotected."),
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

/// Attach the `acl` subcommand group to the given command.
pub fn register_acl_command(cli: Command) -> Command {
    cli.subcommand(acl_command())
}

// ---------------------------------------------------------------------------
// Rendering helpers -- pure, so the tests can assert on them directly.
// ---------------------------------------------------------------------------

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

/// The condition **keys** carried by a rule, comma-joined in lexicographic
/// order. Full condition bodies stay available in `--format json`, which keeps
/// the table readable while the machine format stays lossless (spec 4.4).
#[must_use]
pub fn condition_keys(conditions: Option<&Value>) -> Vec<String> {
    let Some(obj) = conditions.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = obj.keys().cloned().collect();
    keys.sort();
    keys
}

fn approval_label(rule: &apcore::ACLRule) -> &'static str {
    match rule.approval {
        Some(a) if a.is_required() => "required",
        _ => "not_required",
    }
}

/// The JSON body of `apcli acl list`.
#[must_use]
pub fn list_payload(acl: Option<&ACL>, source: Option<&str>) -> Value {
    let Some(acl) = acl else {
        // Listing nothing is not an error (spec 4.4).
        return serde_json::json!({
            "source": Value::Null,
            "default_effect": Value::Null,
            "rules": [],
        });
    };
    let rules: Vec<Value> = acl.rules().iter().enumerate().map(rule_row).collect();
    serde_json::json!({
        "source": source.map(Value::from).unwrap_or(Value::Null),
        "default_effect": acl.default_effect(),
        "rules": rules,
    })
}

fn rule_row((index, rule): (usize, &apcore::ACLRule)) -> Value {
    serde_json::json!({
        "index": index,
        "effect": rule.effect,
        "approval": approval_label(rule),
        "callers": rule.callers,
        "targets": rule.targets,
        "conditions": rule.conditions.clone().unwrap_or(Value::Null),
        "description": rule.description.clone().map(Value::from).unwrap_or(Value::Null),
    })
}

fn value_rows(payload: &Value, key: &str) -> Vec<Map<String, Value>> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_object().cloned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Render `apcli acl list` for the resolved format.
#[must_use]
pub fn render_list(acl: Option<&ACL>, source: Option<&str>, format: &str) -> String {
    let payload = list_payload(acl, source);
    match format {
        "json" => serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()),
        "yaml" => serde_yaml_ng::to_string(&payload)
            .map(|s| s.trim_end().to_string())
            .unwrap_or_default(),
        "csv" => apcore_toolkit::format_csv(&value_rows(&payload, "rules"), false)
            .trim_end_matches("\r\n")
            .to_string(),
        "jsonl" => apcore_toolkit::format_jsonl(&value_rows(&payload, "rules"))
            .trim_end_matches('\n')
            .to_string(),
        _ => render_list_table(acl, source),
    }
}

fn render_list_table(acl: Option<&ACL>, source: Option<&str>) -> String {
    use comfy_table::{ContentArrangement, Table};

    let Some(acl) = acl else {
        return "No ACL configured.".to_string();
    };

    let rules = acl.rules();
    let plural = if rules.len() == 1 { "rule" } else { "rules" };
    let header = format!(
        "Default effect: {}   (source: {}, {} {plural})",
        acl.default_effect(),
        source.unwrap_or("<unknown>"),
        rules.len(),
    );

    if rules.is_empty() {
        return header;
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "#",
        "Effect",
        "Approval",
        "Callers",
        "Targets",
        "Conditions",
        "Description",
    ]);
    for (index, rule) in rules.iter().enumerate() {
        let conditions = condition_keys(rule.conditions.as_ref());
        table.add_row(vec![
            index.to_string(),
            rule.effect.clone(),
            approval_label(rule).to_string(),
            rule.callers.join(", "),
            rule.targets.join(", "),
            if conditions.is_empty() {
                "-".to_string()
            } else {
                conditions.join(", ")
            },
            rule.description.clone().unwrap_or_default(),
        ]);
    }
    format!("{header}\n\n{table}")
}

/// The JSON body of `apcli acl validate`.
#[must_use]
pub fn validate_payload(acl: &ACL) -> Value {
    let findings: Vec<Value> = acl
        .validate_rules()
        .iter()
        .map(|f| {
            serde_json::json!({
                "rule_index": f.rule_index,
                "condition_path": f.condition_path,
                "condition_key": f.condition_key.clone().map(Value::from).unwrap_or(Value::Null),
                "effect": f.effect,
                // The two axes MUST stay separate (spec 4.6 / PROTOCOL_SPEC
                // 6.1.3 rule 3): a finding with sync=no, async=yes is an
                // async-only handler, working under `async_check()` and
                // unevaluable under `check()`. Collapsing them into one
                // boolean loses exactly that.
                "sync_resolvable": f.sync_resolvable,
                "async_resolvable": f.async_resolvable,
            })
        })
        .collect();
    serde_json::json!({
        "count": findings.len(),
        "findings": findings,
    })
}

/// Render `apcli acl validate` as a table.
#[must_use]
pub fn render_validate_table(acl: &ACL) -> String {
    use comfy_table::{ContentArrangement, Table};

    let findings = acl.validate_rules();
    if findings.is_empty() {
        return "0 findings.".to_string();
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["Rule", "Path", "Key", "Effect", "Sync", "Async"]);
    for f in &findings {
        table.add_row(vec![
            f.rule_index.to_string(),
            f.condition_path.clone(),
            f.condition_key.clone().unwrap_or_else(|| "-".to_string()),
            f.effect.clone(),
            yes_no(f.sync_resolvable).to_string(),
            yes_no(f.async_resolvable).to_string(),
        ]);
    }
    let plural = if findings.len() == 1 {
        "finding"
    } else {
        "findings"
    };
    format!(
        "{} {plural}:\n\n{table}\n\nA finding on a `deny` rule is the consequential one: \
         that rule now denies every call it matches.",
        findings.len()
    )
}

/// The JSON body of `apcli acl status`.
#[must_use]
pub fn status_payload(state: &apcore::GovernanceState, source: Option<&str>) -> Value {
    serde_json::json!({
        "control_modules_registered": state.control_modules_registered,
        "read_modules_registered": state.read_modules_registered,
        "acl_configured": state.acl_configured,
        "acl_source": source.map(Value::from).unwrap_or(Value::Null),
        "builtin_acl_gate_wired": state.builtin_acl_gate_wired,
        "approval_handler_configured": state.approval_handler_configured,
        "builtin_approval_gate_wired": state.builtin_approval_gate_wired,
        "policy_strict": state.policy_strict,
        "all_control_modules_require_approval": state.all_control_modules_require_approval,
        "unprotected_control_surface": state.unprotected_control_surface,
    })
}

/// Render `apcli acl status` as an aligned label/value block.
#[must_use]
pub fn render_status_table(state: &apcore::GovernanceState, source: Option<&str>) -> String {
    let acl_line = match (state.acl_configured, source) {
        (true, Some(path)) => format!("yes  ({path})"),
        (true, None) => "yes".to_string(),
        (false, _) => "no".to_string(),
    };
    let rows: [(&str, String); 8] = [
        (
            "Control modules registered:",
            yes_no(state.control_modules_registered).to_string(),
        ),
        (
            "Read modules registered:",
            yes_no(state.read_modules_registered).to_string(),
        ),
        ("ACL configured:", acl_line),
        (
            "Built-in ACL gate wired:",
            yes_no(state.builtin_acl_gate_wired).to_string(),
        ),
        (
            "Approval handler configured:",
            yes_no(state.approval_handler_configured).to_string(),
        ),
        (
            "Built-in approval gate wired:",
            yes_no(state.builtin_approval_gate_wired).to_string(),
        ),
        ("Policy strict:", yes_no(state.policy_strict).to_string()),
        (
            "All control modules gated:",
            yes_no(state.all_control_modules_require_approval).to_string(),
        ),
    ];
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (label, value) in &rows {
        out.push_str(&format!("{label:<width$} {value}\n"));
    }
    out.push_str(&"-".repeat(width + 6));
    out.push('\n');
    out.push_str(&format!(
        "{:<width$} {}",
        "Unprotected control surface:",
        if state.unprotected_control_surface {
            "YES"
        } else {
            "NO"
        },
    ));
    out
}

/// Render the `check` decision as the human-readable block.
#[must_use]
pub fn render_check_table(
    target: &str,
    caller: &str,
    decision: &apcore::AccessDecision,
    rules: &[apcore::ACLRule],
) -> String {
    let matched = decision.matched_rule_index.and_then(|i| {
        rules.get(i).map(|r| match &r.description {
            Some(d) => format!("  (rule #{i}: \"{d}\")"),
            None => format!("  (rule #{i})"),
        })
    });
    format!(
        "Target:   {target}\n\
         Caller:   {caller}\n\
         Decision: {}{}\n\
         Approval: {}\n\
         Reason:   {}",
        decision.access.to_uppercase(),
        matched.unwrap_or_else(|| "  (no rule matched; default_effect)".to_string()),
        if decision.approval_required {
            "REQUIRED"
        } else {
            "NOT REQUIRED"
        },
        decision.reason,
    )
}

/// The JSON body of `apcli acl check`.
#[must_use]
pub fn check_payload(target: &str, caller: &str, decision: &apcore::AccessDecision) -> Value {
    serde_json::json!({
        "target": target,
        "caller": caller,
        "access": decision.access,
        // Authorization and approval are independent axes (PROTOCOL_SPEC
        // 6.1.6). An allow-with-approval outcome exits 0; this field carries
        // the second axis.
        "approval_required": decision.approval_required,
        "matched_rule_index": decision
            .matched_rule_index
            .map(|i| Value::from(i as u64))
            .unwrap_or(Value::Null),
        "reason": decision.reason,
    })
}

// ---------------------------------------------------------------------------
// Exit-code decisions
//
// Kept as pure functions so the exit taxonomy is testable without spawning a
// process: the dispatch arms below are the only callers and do nothing but
// hand the result to `std::process::exit`.
// ---------------------------------------------------------------------------

/// `acl check`: 0 when access is allowed, 77 when denied.
///
/// An allow-with-approval outcome exits **0**. Authorization and approval are
/// independent axes (PROTOCOL_SPEC section 6.1.6); the call *is* permitted,
/// and conflating "needs a human" with "denied" would make the exit code
/// unusable for the scripted policy checks this command exists for.
#[must_use]
pub fn check_exit_code(decision: &apcore::AccessDecision) -> i32 {
    if decision.access == "deny" {
        EXIT_ACL_DENIED
    } else {
        EXIT_SUCCESS
    }
}

/// `acl validate`: 0 with no findings, 47 with at least one.
#[must_use]
pub fn validate_exit_code(finding_count: usize) -> i32 {
    if finding_count > 0 {
        EXIT_CONFIG_NOT_FOUND
    } else {
        EXIT_SUCCESS
    }
}

/// `acl status`: 0 always, unless `--strict` is passed and the control surface
/// is unprotected.
#[must_use]
pub fn status_exit_code(state: &apcore::GovernanceState, strict: bool) -> i32 {
    if strict && state.unprotected_control_surface {
        EXIT_CONFIG_NOT_FOUND
    } else {
        EXIT_SUCCESS
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the `acl` subcommand group. Never returns -- every arm exits.
///
/// `source` is the ACL file that was actually loaded, used for display only;
/// the ACL itself is read back off the executor so `status` and `list` cannot
/// disagree about what is attached.
pub fn dispatch_acl(matches: &clap::ArgMatches, executor: &apcore::Executor, source: Option<&str>) {
    let acl: Option<&ACL> = executor.acl.as_deref();
    match matches.subcommand() {
        Some(("list", sub_m)) => {
            let fmt = resolve_acl_format(sub_m, &["table", "json", "csv", "yaml", "jsonl"]);
            println!("{}", render_list(acl, source, &fmt));
            std::process::exit(EXIT_SUCCESS);
        }
        Some(("check", sub_m)) => dispatch_check(sub_m, acl),
        Some(("validate", sub_m)) => {
            let Some(acl) = acl else {
                eprintln!("Error: No ACL configured; nothing to check.");
                std::process::exit(EXIT_CONFIG_NOT_FOUND);
            };
            let fmt = resolve_acl_format(sub_m, &["table", "json"]);
            let payload = validate_payload(acl);
            if fmt == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                println!("{}", render_validate_table(acl));
            }
            let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
            // Exiting non-zero on any finding is the strict, CI-friendly
            // default; the JSON output carries each finding's `effect` so a
            // caller that wants to gate only on `deny` rules can do so
            // (spec 4.6; `--fail-on` is deferred, section 9 question 1).
            std::process::exit(validate_exit_code(count));
        }
        Some(("status", sub_m)) => {
            let state = executor.governance_state();
            let fmt = resolve_acl_format(sub_m, &["table", "json"]);
            if fmt == "json" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status_payload(&state, source))
                        .unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                println!("{}", render_status_table(&state, source));
            }
            let strict = sub_m.get_flag("strict");
            if strict && state.unprotected_control_surface {
                // The flag exists so a deployment can fail its own startup
                // check without parsing output (spec 4.7).
                eprintln!("Error: Unprotected control surface.");
            }
            std::process::exit(status_exit_code(&state, strict));
        }
        _ => {
            eprintln!("Error: Usage: acl <list|check|validate|status>");
            std::process::exit(EXIT_INVALID_INPUT);
        }
    }
}

fn dispatch_check(sub_m: &clap::ArgMatches, acl: Option<&ACL>) {
    let Some(acl) = acl else {
        eprintln!("Error: No ACL configured; nothing to check.");
        std::process::exit(EXIT_CONFIG_NOT_FOUND);
    };
    let target = sub_m
        .get_one::<String>("target")
        .expect("target is required");
    let caller = sub_m
        .get_one::<String>("caller")
        .map(String::as_str)
        .unwrap_or(DEFAULT_CALLER)
        .to_string();

    let depth = match sub_m.get_one::<String>("depth") {
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("Error: Invalid --depth value '{raw}': expected a non-negative integer.");
                std::process::exit(EXIT_INVALID_INPUT);
            }
        },
        None => None,
    };

    let projection = match sub_m.get_one::<String>("input") {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(v) => Some(GovernanceProjection::from_arguments(&v)),
            Err(e) => {
                eprintln!("Error: Invalid --input JSON: {e}");
                std::process::exit(EXIT_INVALID_INPUT);
            }
        },
        None => None,
    };

    let ctx = build_check_context(depth, projection.is_some());
    // MUST be `check_access`, never `check`: the boolean fails closed on an
    // approval requirement, so it reports "denied" for a call the rule set in
    // fact permits (spec 4.5).
    let decision = acl.check_access(Some(&caller), target, ctx.as_ref(), projection.as_ref());

    let fmt = resolve_acl_format(sub_m, &["table", "json"]);
    if fmt == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&check_payload(target, &caller, &decision))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "{}",
            render_check_table(target, &caller, &decision, acl.rules())
        );
    }

    if decision.access == "deny" {
        eprintln!("Access denied: {caller} -> {target}");
    }
    // An allow-with-approval outcome exits 0: authorization and approval are
    // independent axes, and conflating "needs a human" with "denied" would
    // make the exit code unusable for scripted policy checks (spec 4.5).
    std::process::exit(check_exit_code(&decision));
}

/// Build the simulation context for `acl check`.
///
/// Returns `None` when no identity flag, `--depth` or `--input` was given, so
/// that conditions keyed on `roles` / `identity_types` see the genuine "no
/// context" case rather than a synthetic empty identity.
///
/// `--input` counts because apcore refuses to evaluate **any** condition
/// without a context (PROTOCOL_SPEC section 6.5: conditions require context),
/// so an `arguments` condition supplied with a projection but no context would
/// be reported unsatisfied no matter what the projection said.
fn build_check_context(depth: Option<usize>, has_projection: bool) -> Option<Context<Value>> {
    let identity = cli_identity();
    if identity.is_none() && depth.is_none() && !has_projection {
        return None;
    }
    let mut ctx = match identity {
        Some(i) => Context::<Value>::new(i.to_identity()),
        None => Context::<Value>::anonymous(),
    };
    if let Some(n) = depth {
        // `max_call_depth` compares `ctx.call_chain.len()` against the
        // threshold, so a synthetic chain of length N models a call N frames
        // deep. `caller_id` stays untouched -- see DEFAULT_CALLER.
        ctx.call_chain = (0..n).map(|i| format!("synthetic.frame.{i}")).collect();
    }
    Some(ctx)
}

fn resolve_acl_format(sub_m: &clap::ArgMatches, allowed: &[&str]) -> String {
    let explicit = sub_m.get_one::<String>("format").map(String::as_str);
    let resolved = crate::output::resolve_format(explicit);
    if allowed.contains(&resolved) {
        resolved.to_string()
    } else {
        // `resolve_format` can return a TTY-derived default outside a
        // subcommand's own value set; fall back to the first allowed value.
        allowed.first().copied().unwrap_or("table").to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use apcore::acl::{ACLRule, ApprovalRequirement};

    fn rule(callers: &[&str], targets: &[&str], effect: &str) -> ACLRule {
        ACLRule::new(
            callers.iter().map(|s| (*s).to_string()).collect(),
            targets.iter().map(|s| (*s).to_string()).collect(),
            effect,
        )
    }

    fn sample_acl() -> ACL {
        let mut deny_control = rule(&["@external"], &["system.control.*"], "deny");
        deny_control.description = Some("no external control".to_string());

        let mut migrate = rule(&["*"], &["db.migrate"], "allow");
        migrate.approval = Some(ApprovalRequirement::Required);
        migrate.description = Some("migrations need a human".to_string());
        migrate.conditions = Some(serde_json::json!({"roles": ["admin"]}));

        let read = rule(&["*"], &["db.read"], "allow");

        ACL::try_new(vec![deny_control, migrate, read], "deny", None).expect("well-formed ACL")
    }

    // ----- CliIdentity -----

    #[test]
    fn empty_identity_builds_no_context() {
        assert!(CliIdentity::default().is_empty());
    }

    #[test]
    fn identity_type_defaults_to_user() {
        let identity = CliIdentity {
            id: Some("alice".to_string()),
            identity_type: None,
            roles: vec!["admin".to_string()],
        };
        let built = identity.to_identity();
        assert_eq!(built.id(), "alice");
        assert_eq!(built.identity_type(), DEFAULT_IDENTITY_TYPE);
        assert_eq!(built.roles(), ["admin".to_string()]);
    }

    #[test]
    fn roles_only_form_gets_the_pinned_identity_sentinel() {
        // Cross-SDK pin: `@cli`, exported as DEFAULT_IDENTITY_ID. The `@`
        // prefix follows apcore's synthetic-principal convention so the value
        // cannot collide with a real user id of "cli".
        assert_eq!(DEFAULT_IDENTITY_ID, "@cli");
        let identity = CliIdentity {
            id: None,
            identity_type: None,
            roles: vec!["admin".to_string()],
        };
        let built = identity.to_identity();
        assert_eq!(built.id(), DEFAULT_IDENTITY_ID);
        assert_eq!(built.identity_type(), DEFAULT_IDENTITY_TYPE);
        assert_eq!(built.roles(), ["admin".to_string()]);
    }

    #[test]
    fn the_identity_sentinel_is_not_a_caller_id() {
        // DEFAULT_IDENTITY_ID must never be mistaken for the effective caller,
        // which is always @external for a top-level CLI invocation.
        assert_ne!(DEFAULT_IDENTITY_ID, DEFAULT_CALLER);
    }

    // ----- Command surface -----

    #[test]
    fn acl_group_registers_four_subcommands() {
        let cmd = acl_command();
        let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
        for expected in ["list", "check", "validate", "status"] {
            assert!(names.contains(&expected), "missing '{expected}': {names:?}");
        }
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn register_acl_command_attaches_the_group() {
        let cmd = register_acl_command(Command::new("apcli"));
        assert!(cmd.get_subcommands().any(|c| c.get_name() == "acl"));
    }

    #[test]
    fn check_requires_a_target() {
        let cmd = acl_command();
        let parsed = cmd.try_get_matches_from(vec!["acl", "check"]);
        assert!(parsed.is_err(), "TARGET must be required");
    }

    /// `(help, metavar)` for one argument of a command.
    fn help_and_metavar(cmd: &Command, id: &str) -> (String, String) {
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("'{id}' is not registered on '{}'", cmd.get_name()));
        (
            arg.get_help().map(ToString::to_string).unwrap_or_default(),
            arg.get_value_names()
                .and_then(|n| n.first().map(ToString::to_string))
                .unwrap_or_default(),
        )
    }

    /// The three pinned identity rows, as `(id, metavar, help)`.
    const PINNED_IDENTITY_FLAGS: [(&str, &str, &str); 3] = [
        (
            "identity-id",
            "ID",
            "Assert Identity.id for ACL conditions. \
             Unauthenticated assertion, not authentication.",
        ),
        (
            "identity-type",
            "TYPE",
            "Assert Identity.type for ACL conditions (default: user). \
             Unauthenticated assertion, not authentication.",
        ),
        (
            "role",
            "ROLE",
            "Assert an Identity role for ACL conditions. Repeatable. \
             Unauthenticated assertion, not authentication.",
        ),
    ];

    #[test]
    fn root_identity_flag_help_text_is_the_pinned_wording() {
        // Cross-SDK pin (FE-14 section 4.3). Asserted directly rather than
        // only through the apcli-visibility golden, because that fixture's
        // byte-match does not run in every SDK -- a reword would otherwise
        // pass locally and break the others.
        let cmd = apply_identity_flags(Command::new("root"));
        for (id, metavar, help) in PINNED_IDENTITY_FLAGS {
            assert_eq!(
                help_and_metavar(&cmd, id),
                (help.to_string(), metavar.to_string())
            );
            let arg = cmd
                .get_arguments()
                .find(|a| a.get_id() == id)
                .expect("registered");
            assert!(arg.is_global_set(), "{id} must be global at the root");
        }
    }

    #[test]
    fn acl_check_restates_the_identity_flags_with_identical_wording() {
        // Spec section 4.5: the same flag must not read two different ways
        // inside one CLI. Both levels are built from `identity_args`, so this
        // asserts the property the shared builder is there to guarantee.
        let check = acl_command()
            .get_subcommands()
            .find(|c| c.get_name() == "check")
            .expect("check")
            .clone();
        for (id, metavar, help) in PINNED_IDENTITY_FLAGS {
            assert_eq!(
                help_and_metavar(&check, id),
                (help.to_string(), metavar.to_string()),
                "acl check's '{id}' must be worded exactly like the root flag"
            );
        }
    }

    #[test]
    fn acl_check_only_flags_carry_the_pinned_wording() {
        let check = acl_command()
            .get_subcommands()
            .find(|c| c.get_name() == "check")
            .expect("check")
            .clone();
        assert_eq!(
            help_and_metavar(&check, "caller"),
            (
                "Simulated caller ID (default: @external). \
                 Nothing is executed, so any value is accepted."
                    .to_string(),
                "ID".to_string()
            )
        );
        assert_eq!(
            help_and_metavar(&check, "depth"),
            (
                "Simulated call-chain depth for the max_call_depth condition.".to_string(),
                "N".to_string()
            )
        );
        assert_eq!(
            help_and_metavar(&check, "input"),
            (
                "Argument map for the arguments condition. \
                 Key presence only; values are not compared."
                    .to_string(),
                "JSON".to_string()
            )
        );
    }

    #[test]
    fn defaults_are_resolved_at_the_use_site_not_by_clap() {
        // The pinned help text already states both defaults inline. A clap
        // `default_value` would make it render them twice, breaking the
        // byte-match.
        let check = acl_command()
            .get_subcommands()
            .find(|c| c.get_name() == "check")
            .expect("check")
            .clone();
        for id in ["caller", "identity-type"] {
            let arg = check
                .get_arguments()
                .find(|a| a.get_id() == id)
                .expect("registered");
            assert!(
                arg.get_default_values().is_empty(),
                "'{id}' must carry no clap default; the help text states it inline"
            );
        }
    }

    #[test]
    fn identity_flags_merge_per_field_across_the_two_levels() {
        // Spec section 4.5: a subcommand-level flag overrides only its own
        // counterpart. Restating one must NOT discard the others -- the
        // all-or-nothing form ("sub identity, else root identity") silently
        // drops a field the caller never withdrew.
        let root = apply_identity_flags(Command::new("root")).subcommand(acl_command());
        let matches = root
            .try_get_matches_from(vec![
                "root",
                "--identity-type",
                "service",
                "--role",
                "admin",
                "acl",
                "check",
                "--role",
                "guest",
                "db.read",
            ])
            .expect("parses");
        let merged = identity_from_matches(&matches);
        assert_eq!(
            merged.identity_type.as_deref(),
            Some("service"),
            "the root --identity-type was not restated, so it still applies"
        );
        assert_eq!(
            merged.roles,
            vec!["guest".to_string()],
            "the subcommand --role overrides its root counterpart"
        );
        assert_eq!(merged.id, None);
    }

    // ----- list -----

    #[test]
    fn list_json_preserves_definition_order() {
        let acl = sample_acl();
        let payload = list_payload(Some(&acl), Some("./acl/global_acl.yaml"));
        let rules = payload["rules"].as_array().expect("rules array");
        assert_eq!(rules.len(), 3);
        for (i, r) in rules.iter().enumerate() {
            assert_eq!(r["index"].as_u64(), Some(i as u64));
        }
        assert_eq!(payload["default_effect"], "deny");
        assert_eq!(payload["source"], "./acl/global_acl.yaml");
        assert_eq!(rules[0]["effect"], "deny");
        assert_eq!(rules[1]["approval"], "required");
        assert_eq!(rules[2]["approval"], "not_required");
    }

    #[test]
    fn list_json_with_no_acl_is_the_documented_empty_shape() {
        let payload = list_payload(None, None);
        assert_eq!(payload["source"], Value::Null);
        assert_eq!(payload["default_effect"], Value::Null);
        assert_eq!(payload["rules"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn list_table_with_no_acl_says_so() {
        assert_eq!(render_list(None, None, "table"), "No ACL configured.");
    }

    #[test]
    fn list_table_shows_condition_keys_only() {
        let acl = sample_acl();
        let out = render_list(Some(&acl), Some("./acl/global_acl.yaml"), "table");
        assert!(out.contains("Default effect: deny"));
        assert!(out.contains("3 rules"));
        assert!(out.contains("roles"), "condition key column: {out}");
        assert!(
            !out.contains("admin"),
            "condition bodies belong in --format json only: {out}"
        );
    }

    #[test]
    fn condition_keys_are_lexicographic() {
        let conditions = serde_json::json!({"roles": [], "$or": [], "max_call_depth": 3});
        assert_eq!(
            condition_keys(Some(&conditions)),
            vec![
                "$or".to_string(),
                "max_call_depth".to_string(),
                "roles".to_string()
            ]
        );
        assert!(condition_keys(None).is_empty());
    }

    #[test]
    fn list_csv_and_jsonl_render_the_rule_rows() {
        let acl = sample_acl();
        let csv = render_list(Some(&acl), Some("s"), "csv");
        assert!(csv.starts_with("index,effect,approval"), "{csv}");
        let jsonl = render_list(Some(&acl), Some("s"), "jsonl");
        assert_eq!(jsonl.lines().count(), 3);
    }

    // ----- check -----

    #[test]
    fn check_allow_reports_the_matched_rule() {
        let acl = sample_acl();
        let decision = acl.check_access(Some("@external"), "db.read", None, None);
        assert_eq!(decision.access, "allow");
        let payload = check_payload("db.read", "@external", &decision);
        assert_eq!(payload["access"], "allow");
        assert_eq!(payload["matched_rule_index"], 2);
        let table = render_check_table("db.read", "@external", &decision, acl.rules());
        assert!(table.contains("Decision: ALLOW"), "{table}");
        assert!(table.contains("Approval: NOT REQUIRED"), "{table}");
    }

    #[test]
    fn check_deny_is_reported_on_the_access_axis() {
        let acl = sample_acl();
        let decision = acl.check_access(Some("@external"), "system.control.disable", None, None);
        assert_eq!(decision.access, "deny");
        assert_eq!(decision.matched_rule_index, Some(0));
    }

    #[test]
    fn allow_with_approval_is_still_an_allow() {
        // T-ACL-13 discriminator: the approval axis must not be folded into
        // the access axis.
        let acl = sample_acl();
        let identity = Identity::new(
            "alice".to_string(),
            "user".to_string(),
            vec!["admin".to_string()],
            std::collections::HashMap::new(),
        );
        let ctx = Context::<Value>::new(identity);
        let decision = acl.check_access(Some("@external"), "db.migrate", Some(&ctx), None);
        assert_eq!(decision.access, "allow");
        assert!(decision.approval_required);
        let payload = check_payload("db.migrate", "@external", &decision);
        assert_eq!(payload["approval_required"], true);
    }

    #[test]
    fn missing_role_falls_through_to_the_default_effect() {
        let acl = sample_acl();
        let decision = acl.check_access(Some("@external"), "db.migrate", None, None);
        assert_eq!(decision.access, "deny");
        assert_eq!(decision.matched_rule_index, None);
    }

    #[test]
    fn depth_builds_a_synthetic_call_chain() {
        let ctx = build_check_context(Some(3), false).expect("depth alone builds a context");
        assert_eq!(ctx.call_chain.len(), 3);
        assert!(ctx.identity.is_none(), "no identity flag was given");
    }

    #[test]
    fn input_alone_builds_a_context() {
        // PROTOCOL_SPEC 6.5 refuses to evaluate any condition without a
        // context, so a projection supplied with none would be inert.
        set_cli_identity(None);
        assert!(build_check_context(None, /*has_projection*/ true).is_some());
    }

    #[test]
    fn the_delegated_gate_context_is_never_absent() {
        // Spec section 4.10. PROTOCOL_SPEC 6.5 makes every conditional rule a
        // non-match without a context, so a gate handing `None` to
        // `check_access` would leave conditional deny rules inert on the
        // delegated path while they fire in-process.
        set_cli_identity(None);
        let ctx = delegated_gate_context();
        let identity = ctx.identity.as_ref().expect("a context is always built");
        // Reproduces exactly what `Executor::call` builds for `ctx: None`, so
        // an identity_types rule behaves identically on both paths.
        assert_eq!(identity.id(), DEFAULT_CALLER);
        assert_eq!(identity.identity_type(), "external");
        assert!(identity.roles().is_empty());

        // With flags, the asserted identity is carried instead.
        set_cli_identity(Some(CliIdentity {
            id: Some("alice".to_string()),
            identity_type: Some("service".to_string()),
            roles: vec!["admin".to_string()],
        }));
        let ctx = delegated_gate_context();
        let identity = ctx.identity.as_ref().expect("identity");
        assert_eq!(identity.id(), "alice");
        assert_eq!(identity.identity_type(), "service");
        assert_eq!(identity.roles(), ["admin".to_string()]);
        set_cli_identity(None);
    }

    #[test]
    fn no_flags_means_no_context() {
        set_cli_identity(None);
        assert!(build_check_context(None, false).is_none());
    }

    // ----- validate -----

    #[test]
    fn validate_reports_zero_findings_on_a_clean_rule_set() {
        let acl = sample_acl();
        let payload = validate_payload(&acl);
        assert_eq!(payload["count"], 0);
        assert_eq!(render_validate_table(&acl), "0 findings.");
    }

    #[test]
    fn validate_names_an_unregistered_condition_key() {
        let mut bad = rule(&["*"], &["db.migrate"], "deny");
        bad.conditions = Some(serde_json::json!({"mispelled": ["x"]}));
        let acl = ACL::try_new(vec![bad], "deny", None).expect("loads with a warning");
        let payload = validate_payload(&acl);
        assert_eq!(payload["count"], 1);
        let finding = &payload["findings"][0];
        assert_eq!(finding["rule_index"], 0);
        assert_eq!(finding["condition_key"], "mispelled");
        assert_eq!(finding["effect"], "deny");
        // Sync and Async are separate columns, never one collapsed boolean.
        assert!(finding.get("sync_resolvable").is_some());
        assert!(finding.get("async_resolvable").is_some());

        let table = render_validate_table(&acl);
        assert!(table.contains("1 finding:"), "{table}");
        assert!(table.contains("mispelled"), "{table}");
        assert!(table.contains("Sync"), "{table}");
        assert!(table.contains("Async"), "{table}");
    }

    // ----- status -----

    #[test]
    fn status_renders_all_nine_observations() {
        let state = apcore::GovernanceState {
            control_modules_registered: true,
            read_modules_registered: true,
            acl_configured: true,
            builtin_acl_gate_wired: true,
            approval_handler_configured: true,
            builtin_approval_gate_wired: true,
            policy_strict: false,
            all_control_modules_require_approval: false,
            unprotected_control_surface: false,
        };
        let out = render_status_table(&state, Some("./acl/global_acl.yaml"));
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
            assert!(out.contains(label), "missing '{label}' in:\n{out}");
        }
        assert!(out.contains("yes  (./acl/global_acl.yaml)"), "{out}");
        assert!(out.ends_with("NO"), "{out}");

        let payload = status_payload(&state, Some("./acl/global_acl.yaml"));
        assert_eq!(payload["unprotected_control_surface"], false);
        assert_eq!(payload["acl_source"], "./acl/global_acl.yaml");
    }

    // ----- strategy warning -----

    #[test]
    fn strategy_warning_only_fires_with_an_acl_and_a_bypassing_strategy() {
        // Pure predicate check -- the emitting function writes to stderr, so
        // the table it consults is asserted directly.
        assert!(ACL_BYPASSING_STRATEGIES.contains(&"testing"));
        assert!(ACL_BYPASSING_STRATEGIES.contains(&"internal"));
        assert!(ACL_BYPASSING_STRATEGIES.contains(&"minimal"));
        assert!(!ACL_BYPASSING_STRATEGIES.contains(&"standard"));
        // No panic on either no-op path.
        warn_if_strategy_bypasses_acl(Some("testing"), false);
        warn_if_strategy_bypasses_acl(Some("standard"), true);
    }
}
