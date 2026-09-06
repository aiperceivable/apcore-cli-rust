// apcore-cli -- ACL root resolution and loading (FE-14 sections 4.1-4.2).
//
// The CLI has always carried the *downstream* half of apcore's access control
// (exit code 77 for ACL_DENIED, the `acl` preflight row, `acl_check` in
// `describe-pipeline`) without ever constructing an `ACL`, because it builds
// an `Executor` directly rather than going through the `APCore` bootstrap that
// performs `ACL::discover`. This module closes that loop: it resolves an ACL
// root through the FE-07 4-tier chain and delegates the parse to `ACL::load`.
//
// The CLI deliberately does NOT reimplement YAML rule parsing. Rule-key
// closure, `effect` / `approval` enum closure and pattern-array arity are
// apcore's contract and are conformance-tested there (PROTOCOL_SPEC section
// 6.2.1).

use std::path::{Path, PathBuf};

use apcore::acl::{AuditEntry, ACL};
use apcore::errors::{ErrorCode, ModuleError};

use crate::config::ConfigResolver;
use crate::security::AuditLogger;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Environment variable consulted at tier 2.
///
/// `acl.root` is an **apcore-owned** config key -- it appears in apcore's own
/// `Config` defaults -- so its environment variable follows the apcore
/// convention `APCORE_ACL_ROOT`, exactly as `extensions.root` is overridden by
/// `APCORE_EXTENSIONS_ROOT` and not by an `APCORE_CLI_*` name (spec 4.1).
pub const ACL_ROOT_ENV: &str = "APCORE_ACL_ROOT";

/// Tier-4 default, matching apcore's `Config::get_default("acl.root")`.
pub const DEFAULT_ACL_ROOT: &str = "./acl";

/// The conventional file loaded when the resolved root is a directory
/// (PROTOCOL_SPEC section 3.1, mirrored by `ACL::discover`).
pub const GLOBAL_ACL_FILENAME: &str = "global_acl.yaml";

/// Config key for the ACL root.
pub const ACL_ROOT_KEY: &str = "acl.root";

/// Config key: write ACL decisions to the FE-05 audit log (spec section 5).
pub const ACL_AUDIT_ENABLED_KEY: &str = "acl.audit.enabled";

/// Environment override for [`ACL_AUDIT_ENABLED_KEY`].
///
/// `acl.audit.*` are apcore-owned keys (they appear in apcore's own
/// `schemas/acl-config.schema.json`), so they follow the apcore variable
/// convention `APCORE_*` rather than `APCORE_CLI_*`, exactly as
/// [`ACL_ROOT_ENV`] does.
pub const ACL_AUDIT_ENABLED_ENV: &str = "APCORE_ACL_AUDIT_ENABLED";

/// Config key: whether **denied** access attempts are logged (spec section 5).
pub const ACL_AUDIT_INCLUDE_DENIED_KEY: &str = "acl.audit.include_denied";

/// Environment override for [`ACL_AUDIT_INCLUDE_DENIED_KEY`].
pub const ACL_AUDIT_INCLUDE_DENIED_ENV: &str = "APCORE_ACL_AUDIT_INCLUDE_DENIED";

// ---------------------------------------------------------------------------
// resolve_acl_root
// ---------------------------------------------------------------------------

/// Resolve the ACL root through the FE-07 4-tier precedence chain.
///
/// | Tier | Source | Notes |
/// |------|--------|-------|
/// | 1 | `--acl PATH` | standalone mode only, alongside `--extensions-dir` |
/// | 2 | `APCORE_ACL_ROOT` | apcore-owned key, apcore-prefixed variable |
/// | 3 | `acl.root` in `apcore.yaml` | the same key `ACL::discover` reads |
/// | 4 | `./acl` | matches `Config::get_default("acl.root")` |
///
/// Tiers 1-3 return the configured value verbatim, whether or not it exists on
/// disk -- a configured-but-missing root is not an error, it simply attaches
/// nothing (see [`load_cli_acl`]). Tier 4 is only reported when the default
/// path actually exists, so a project with no `acl/` directory resolves to
/// `None` and is indistinguishable from pre-FE-14 behaviour.
///
/// Raises nothing: an unresolvable root is reported as `None`.
pub fn resolve_acl_root(config: &ConfigResolver, cli_flag: Option<&str>) -> Option<String> {
    // Tier 1 -- the `--acl` flag value (or an explicit `acl=` argument).
    if let Some(value) = non_empty(cli_flag) {
        return Some(value);
    }

    // Tier 2 -- APCORE_ACL_ROOT.
    if let Ok(raw) = std::env::var(ACL_ROOT_ENV) {
        if let Some(value) = non_empty(Some(&raw)) {
            return Some(value);
        }
    }

    // Tier 3 -- `acl.root` in apcore.yaml. Read from the resolver's flattened
    // file map rather than through `resolve()`, because `resolve()` also
    // consults tier 4 and the two tiers are treated differently here.
    if let Some(value) = config
        .config_file
        .as_ref()
        .and_then(|file| file.get(ACL_ROOT_KEY))
        .and_then(|raw| non_empty(Some(raw)))
    {
        return Some(value);
    }

    // Tier 4 -- the built-in default, reported only when it exists.
    let default_root = config
        .defaults
        .get(ACL_ROOT_KEY)
        .copied()
        .unwrap_or(DEFAULT_ACL_ROOT);
    if Path::new(default_root).exists() {
        Some(default_root.to_string())
    } else {
        None
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// resolve_acl_file
// ---------------------------------------------------------------------------

/// Apply the directory convention `ACL::discover` documents and report the
/// file that would actually be loaded, or `None` when nothing would be.
///
/// 1. The resolved path does not exist -> `None`.
/// 2. The resolved path is a directory -> `<root>/global_acl.yaml`, or `None`
///    when that file is absent.
/// 3. The resolved path is a file -> the file itself.
///
/// Step 1 is a **hard invariant** (PROTOCOL_SPEC section 6.1 missing-path
/// rule): the CLI MUST NOT synthesize an empty ACL, because an empty ACL with
/// `default_effect: deny` denies every call in every project that lacks an
/// `acl/` directory.
pub fn resolve_acl_file(root: &str) -> Option<PathBuf> {
    let path = Path::new(root);
    if !path.exists() {
        return None;
    }
    if path.is_dir() {
        let candidate = path.join(GLOBAL_ACL_FILENAME);
        return candidate.is_file().then_some(candidate);
    }
    Some(path.to_path_buf())
}

// ---------------------------------------------------------------------------
// load_cli_acl
// ---------------------------------------------------------------------------

/// Load the ACL rooted at `root`, or `None` when the conventional file is
/// absent.
///
/// # Errors
///
/// * `ErrorCode::ConfigNotFound` -- the path vanished between resolution and
///   load. Exit 47.
/// * `ErrorCode::ACLRuleError` -- the file is structurally invalid (bad
///   `default_effect`, unknown rule key, malformed pattern array, non-mapping
///   `conditions`). Exit 47.
///
/// Both are produced by `ACL::load`; this function adds no validation of its
/// own.
pub fn load_cli_acl(root: &str) -> Result<Option<ACL>, ModuleError> {
    let Some(file) = resolve_acl_file(root) else {
        return Ok(None);
    };
    let path = file.to_string_lossy().to_string();
    ACL::load(&path).map(Some)
}

// ---------------------------------------------------------------------------
// Audit wiring (spec section 4.8)
// ---------------------------------------------------------------------------

/// Resolve one of the two `acl.audit.*` booleans through the FE-07 chain.
///
/// [`ConfigResolver::resolve`] already walks env -> `apcore.yaml` -> DEFAULTS,
/// and both keys carry a `"true"` default, so the `map_or` fallback is only
/// reached if the entry is somehow missing from `DEFAULTS`.
fn resolve_audit_flag(config: &ConfigResolver, key: &str, env_var: &str) -> bool {
    match config.resolve(key, None, Some(env_var)) {
        Some(raw) => parse_config_bool(&raw, key, true),
        // Unreachable while the key sits in `DEFAULTS`. The `true` is restated
        // here rather than inferred, so removing the DEFAULTS entry cannot
        // silently flip the key to off.
        None => true,
    }
}

/// Parse a config-string boolean against the spec section 4.8 spelling table.
///
/// | Value | Spellings |
/// |---|---|
/// | true  | `true`, `1`, `yes`, `on` |
/// | false | `false`, `0`, `no`, `off` |
///
/// Case-insensitive, after trimming. The table is normative and shared with
/// Python and TypeScript, so this MUST NOT delegate to `str::parse::<bool>()`
/// -- Rust's `FromStr for bool` accepts only `"true"` / `"false"` and returns
/// an **error** for `"0"`, which would leave `APCORE_ACL_AUDIT_ENABLED=0`
/// unable to switch auditing off while the same value works in the other two
/// SDKs.
///
/// An unrecognised spelling falls back to `default` -- the key's default, not
/// `false`. Both `acl.audit.*` keys default to `true`, so a typo leaves
/// auditing **on**: reading an unparseable governance value as "off" would let
/// a misspelling silently stop the audit trail, which is the failure an
/// operator is least likely to notice. The warning names the key so the typo
/// is still discoverable (parity with Python).
fn parse_config_bool(raw: &str, key: &str, default: bool) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => true,
        "false" | "0" | "no" | "off" => false,
        _ => {
            tracing::warn!(
                "Unrecognised boolean value {raw:?} for config key '{key}'; \
                 using the default ({default}). Accepted: true/1/yes/on, false/0/no/off."
            );
            default
        }
    }
}

/// Whether ACL decisions are written to the FE-05 audit log.
///
/// `acl.audit.enabled` / `APCORE_ACL_AUDIT_ENABLED`, default `true`.
pub fn acl_audit_enabled(config: &ConfigResolver) -> bool {
    resolve_audit_flag(config, ACL_AUDIT_ENABLED_KEY, ACL_AUDIT_ENABLED_ENV)
}

/// Whether **denied** decisions are written.
///
/// `acl.audit.include_denied` / `APCORE_ACL_AUDIT_INCLUDE_DENIED`, default
/// `true`. The semantics are apcore's, from
/// `schemas/acl-config.schema.json` ("Whether to log denied access
/// attempts"): `false` suppresses **deny** entries only, and allow entries
/// keep being written. It is not an inverted "log denials only" switch.
pub fn acl_audit_include_denied(config: &ConfigResolver) -> bool {
    resolve_audit_flag(
        config,
        ACL_AUDIT_INCLUDE_DENIED_KEY,
        ACL_AUDIT_INCLUDE_DENIED_ENV,
    )
}

/// The FE-14 section 4.8 audit-log record: apcore's 13 `AuditEntry` fields,
/// in apcore's own declaration order.
///
/// **Field set.** Exactly these 13, `snake_case`, none renamed and none
/// dropped -- `handler_error` and `approval_required` included. And exactly
/// these 13: the CLI MUST NOT add fields of its own, notably not the `user`
/// field FE-05 puts on *execution* records, so a consumer can read an ACL
/// record against apcore's `AuditEntry` rather than a CLI dialect of it.
///
/// **Why a struct rather than `serde_json::json!`.** The audit log is JSONL
/// and key order is normative -- the same decision must serialise to the same
/// bytes in all three SDKs. `serde_json::Map` is a `BTreeMap` unless the
/// `preserve_order` feature happens to be on somewhere in the dependency
/// graph, so a `json!` literal would emit alphabetical order on one build and
/// insertion order on another, silently. Serde writes struct fields in
/// declaration order unconditionally, which is why the wire shape is pinned by
/// this type's field order and not by a map literal.
///
/// **Why not serialise apcore's `AuditEntry` directly.** It carries
/// `#[serde(skip_serializing_if = "Option::is_none")]` on six optional fields,
/// so an entry that did not populate them would lose `matched_rule`,
/// `matched_rule_index`, `identity_type`, `call_depth`, `trace_id` and
/// `handler_error` entirely. Here an absent value is written as `null` and
/// every line carries the same key set. `AuditEntry` is also
/// `#[non_exhaustive]`, so a field apcore adds later will not appear here
/// until this record and the spec's field count are updated together --
/// deliberate, since the wire shape is a cross-SDK contract rather than a
/// mirror of whatever the runtime currently holds.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AclAuditRecord {
    pub timestamp: String,
    pub caller_id: String,
    pub target_id: String,
    pub decision: String,
    pub reason: String,
    pub matched_rule: Option<String>,
    pub matched_rule_index: Option<usize>,
    pub identity_type: Option<String>,
    pub roles: Vec<String>,
    pub call_depth: Option<usize>,
    pub trace_id: Option<String>,
    pub handler_error: Option<String>,
    pub approval_required: bool,
}

/// The normative key order of [`AclAuditRecord`], for assertions and for
/// consumers that need to check a line without reparsing it.
pub const ACL_AUDIT_FIELDS: [&str; 13] = [
    "timestamp",
    "caller_id",
    "target_id",
    "decision",
    "reason",
    "matched_rule",
    "matched_rule_index",
    "identity_type",
    "roles",
    "call_depth",
    "trace_id",
    "handler_error",
    "approval_required",
];

/// Adapt one apcore [`AuditEntry`] into the [`AclAuditRecord`] wire form.
pub fn acl_audit_record(entry: &AuditEntry) -> AclAuditRecord {
    AclAuditRecord {
        timestamp: entry.timestamp.clone(),
        caller_id: entry.caller_id.clone(),
        target_id: entry.target_id.clone(),
        decision: entry.decision.clone(),
        reason: entry.reason.clone(),
        matched_rule: entry.matched_rule.clone(),
        matched_rule_index: entry.matched_rule_index,
        identity_type: entry.identity_type.clone(),
        roles: entry.roles.clone(),
        call_depth: entry.call_depth,
        trace_id: entry.trace_id.clone(),
        handler_error: entry.handler_error.clone(),
        approval_required: entry.approval_required,
    }
}

/// Install the FE-05 audit logger as `acl`'s audit callback.
///
/// apcore emits exactly one `AuditEntry` per `check_access()` call, and only
/// through this callback; nothing in apcore wires the `acl.audit.*` keys to
/// one. The CLI is the consumer that does.
///
/// **Why the setter rather than a rebuild.** Spec section 4.8 describes
/// `ACL::new(src.rules, src.default_effect, logger)` because that is the only
/// mechanism Python and TypeScript offer, and it explicitly permits an SDK to
/// "use whichever mechanism its runtime offers". Rust has
/// `ACL::set_audit_logger`, which is strictly less lossy on two counts:
///
/// 1. The rebuild has to carry `default_effect` across by hand. Get that
///    wrong -- pass a literal `"deny"` -- and every file declaring
///    `default_effect: allow` has its governing default silently inverted for
///    each call no rule matched. The setter cannot express that mistake.
/// 2. The rebuild drops `yaml_path`, so the attached ACL loses `reload()`
///    (`ACLRuleError("Cannot reload: ACL was not loaded from a YAML file")`).
///    Nothing in the CLI calls `reload()` today, but the setter costs nothing
///    to keep it working.
///
/// The observable contract is the one section 4.8 pins for all three SDKs:
/// the attached ACL emits one entry per decision and carries the file's rules
/// and `default_effect` unchanged.
/// **A logging fault never changes an access decision.** The callback is
/// infallible by construction: building the record cannot fail, and
/// `AuditLogger::log_acl_decision` swallows its own IO errors behind a
/// one-shot warning, exactly as it does for execution records. An unwritable
/// audit log, or an `AuditLogger` with no path at all, therefore costs the
/// entry and nothing else -- `check_access` returns the same decision it would
/// have returned with auditing off.
pub fn install_acl_audit_logger(acl: &mut ACL, logger: AuditLogger, include_denied: bool) {
    acl.set_audit_logger(move |entry: &AuditEntry| {
        // `include_denied: false` suppresses denials only; allow decisions are
        // still written (spec section 4.8 table, row 3).
        if !include_denied && entry.decision == "deny" {
            return;
        }
        logger.log_acl_decision(&acl_audit_record(entry));
    });
}

/// Load the ACL rooted at `root` and wire the section 4.8 audit callback.
///
/// The load half is [`load_cli_acl`] verbatim, errors included. On top of it:
///
/// * `logger` is `None` when FE-05 auditing is off process-wide
///   (`APCORE_CLI_AUDIT_DISABLE`); no ACL callback is installed either, since
///   there is no log to write to.
/// * `acl.audit.enabled: false` attaches the `ACL::load` result directly --
///   no callback, no rebuild.
/// * Otherwise the callback is installed in place, honouring
///   `acl.audit.include_denied`.
///
/// An ACL an embedder supplied itself never reaches this function: it is
/// handed straight to `Executor::set_acl` and is attached unchanged (spec
/// section 4.2).
pub fn load_cli_acl_with_audit(
    root: &str,
    config: &ConfigResolver,
    logger: Option<AuditLogger>,
) -> Result<Option<ACL>, ModuleError> {
    let Some(mut acl) = load_cli_acl(root)? else {
        return Ok(None);
    };
    if let Some(logger) = logger {
        if acl_audit_enabled(config) {
            install_acl_audit_logger(&mut acl, logger, acl_audit_include_denied(config));
        }
    }
    Ok(Some(acl))
}

// ---------------------------------------------------------------------------
// describe_load_error
// ---------------------------------------------------------------------------

/// Render the operator-facing message for a [`load_cli_acl`] failure, per the
/// FE-14 section 6 error table.
///
/// * `ConfigNotFound` -> `ACL file not found: {path}`
/// * anything else -> `Invalid ACL configuration in {path}: {detail}`
pub fn describe_load_error(root: &str, err: &ModuleError) -> String {
    let path = resolve_acl_file(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| root.to_string());
    if err.code == ErrorCode::ConfigNotFound {
        format!("ACL file not found: {path}")
    } else {
        format!("Invalid ACL configuration in {path}: {}", err.message)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// std::env is process-global; every test touching APCORE_ACL_ROOT or the
    /// working directory serializes on this.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        previous: Option<String>,
        previous_cwd: Option<PathBuf>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let previous = std::env::var(ACL_ROOT_ENV).ok();
            // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
            unsafe {
                std::env::remove_var(ACL_ROOT_ENV);
            }
            Self {
                previous,
                previous_cwd: std::env::current_dir().ok(),
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
            unsafe {
                std::env::set_var(ACL_ROOT_ENV, value);
            }
        }

        fn chdir(&self, dir: &Path) {
            std::env::set_current_dir(dir).expect("chdir");
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
            unsafe {
                match self.previous.take() {
                    Some(v) => std::env::set_var(ACL_ROOT_ENV, v),
                    None => std::env::remove_var(ACL_ROOT_ENV),
                }
            }
            if let Some(cwd) = self.previous_cwd.take() {
                let _ = std::env::set_current_dir(cwd);
            }
        }
    }

    fn resolver_with_file(entries: &[(&str, &str)]) -> ConfigResolver {
        let mut r = ConfigResolver::new(None, None);
        let mut map: HashMap<String, String> = HashMap::new();
        for (k, v) in entries {
            map.insert((*k).to_string(), (*v).to_string());
        }
        r.config_file = Some(map);
        r
    }

    const MINIMAL_ACL: &str =
        "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: allow\n";

    // ----- resolve_acl_root -----

    #[test]
    fn tier1_cli_flag_wins_over_everything() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = EnvGuard::new();
        guard.set("./from-env");
        let resolver = resolver_with_file(&[("acl.root", "./from-yaml")]);
        assert_eq!(
            resolve_acl_root(&resolver, Some("./custom.yaml")),
            Some("./custom.yaml".to_string())
        );
    }

    #[test]
    fn tier2_env_wins_over_yaml() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = EnvGuard::new();
        guard.set("./other");
        let resolver = resolver_with_file(&[("acl.root", "./from-yaml")]);
        assert_eq!(
            resolve_acl_root(&resolver, None),
            Some("./other".to_string())
        );
    }

    #[test]
    fn tier3_yaml_used_when_no_flag_or_env() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::new();
        let resolver = resolver_with_file(&[("acl.root", "./from-yaml")]);
        assert_eq!(
            resolve_acl_root(&resolver, None),
            Some("./from-yaml".to_string())
        );
    }

    #[test]
    fn tier4_default_is_none_when_absent() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = EnvGuard::new();
        let dir = tempfile::tempdir().expect("tempdir");
        guard.chdir(dir.path());
        let resolver = ConfigResolver::new(None, None);
        assert_eq!(resolve_acl_root(&resolver, None), None);
    }

    #[test]
    fn tier4_default_is_reported_when_present() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = EnvGuard::new();
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("acl")).expect("mkdir acl");
        guard.chdir(dir.path());
        let resolver = ConfigResolver::new(None, None);
        assert_eq!(
            resolve_acl_root(&resolver, None),
            Some(DEFAULT_ACL_ROOT.to_string())
        );
    }

    #[test]
    fn empty_values_fall_through_to_the_next_tier() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = EnvGuard::new();
        guard.set("   ");
        let resolver = resolver_with_file(&[("acl.root", "./from-yaml")]);
        assert_eq!(
            resolve_acl_root(&resolver, Some("")),
            Some("./from-yaml".to_string())
        );
    }

    // ----- resolve_acl_file / load_cli_acl -----

    #[test]
    fn missing_root_attaches_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        assert_eq!(resolve_acl_file(missing.to_str().unwrap()), None);
        assert!(load_cli_acl(missing.to_str().unwrap())
            .expect("missing root is not an error")
            .is_none());
    }

    #[test]
    fn directory_without_global_acl_attaches_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let acl_dir = dir.path().join("acl");
        std::fs::create_dir(&acl_dir).expect("mkdir");
        assert_eq!(resolve_acl_file(acl_dir.to_str().unwrap()), None);
        assert!(load_cli_acl(acl_dir.to_str().unwrap())
            .expect("no conventional file is not an error")
            .is_none());
    }

    #[test]
    fn directory_with_global_acl_is_loaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let acl_dir = dir.path().join("acl");
        std::fs::create_dir(&acl_dir).expect("mkdir");
        std::fs::write(acl_dir.join(GLOBAL_ACL_FILENAME), MINIMAL_ACL).expect("write");
        let acl = load_cli_acl(acl_dir.to_str().unwrap())
            .expect("well-formed ACL loads")
            .expect("an ACL is attached");
        assert_eq!(acl.default_effect(), "deny");
        assert_eq!(acl.rules().len(), 1);
    }

    #[test]
    fn file_root_is_loaded_directly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("custom.yaml");
        std::fs::write(&file, MINIMAL_ACL).expect("write");
        let acl = load_cli_acl(file.to_str().unwrap())
            .expect("well-formed ACL loads")
            .expect("an ACL is attached");
        assert_eq!(acl.rules().len(), 1);
    }

    #[test]
    fn structurally_invalid_acl_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("bad.yaml");
        std::fs::write(
            &file,
            "default_effect: deny\nrules:\n  - callers: ['*']\n    targets: ['*']\n    effect: permit\n",
        )
        .expect("write");
        let err = load_cli_acl(file.to_str().unwrap()).expect_err("effect enum is closed");
        assert_eq!(err.code, ErrorCode::ACLRuleError);
        let msg = describe_load_error(file.to_str().unwrap(), &err);
        assert!(
            msg.starts_with("Invalid ACL configuration in "),
            "unexpected message: {msg}"
        );
    }

    #[test]
    fn describe_load_error_names_a_missing_file() {
        let err = ModuleError::new(ErrorCode::ConfigNotFound, "gone".to_string());
        assert_eq!(
            describe_load_error("/nope/acl.yaml", &err),
            "ACL file not found: /nope/acl.yaml"
        );
    }

    // ----- audit config resolution (spec section 5) -----

    /// Clears both `acl.audit.*` variables for the duration of a test and
    /// restores whatever the ambient environment had. Serialized on
    /// `ENV_MUTEX` alongside the `APCORE_ACL_ROOT` tests.
    struct AuditEnvGuard {
        previous: Vec<(&'static str, Option<String>)>,
    }

    impl AuditEnvGuard {
        fn new() -> Self {
            let vars = [ACL_AUDIT_ENABLED_ENV, ACL_AUDIT_INCLUDE_DENIED_ENV];
            let previous = vars
                .iter()
                .map(|name| {
                    let prior = std::env::var(name).ok();
                    // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
                    unsafe {
                        std::env::remove_var(name);
                    }
                    (*name, prior)
                })
                .collect();
            Self { previous }
        }

        fn set(&self, name: &str, value: &str) {
            // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
            unsafe {
                std::env::set_var(name, value);
            }
        }
    }

    impl Drop for AuditEnvGuard {
        fn drop(&mut self) {
            for (name, prior) in self.previous.drain(..) {
                // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
                unsafe {
                    match prior {
                        Some(v) => std::env::set_var(name, v),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    #[test]
    fn audit_flags_default_to_true() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = AuditEnvGuard::new();
        let resolver = ConfigResolver::new(None, None);
        assert!(acl_audit_enabled(&resolver));
        assert!(acl_audit_include_denied(&resolver));
    }

    #[test]
    fn audit_flags_read_the_environment() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = AuditEnvGuard::new();
        guard.set(ACL_AUDIT_ENABLED_ENV, "false");
        guard.set(ACL_AUDIT_INCLUDE_DENIED_ENV, "0");
        let resolver = ConfigResolver::new(None, None);
        assert!(!acl_audit_enabled(&resolver));
        assert!(!acl_audit_include_denied(&resolver));
    }

    #[test]
    fn audit_flags_read_apcore_yaml() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = AuditEnvGuard::new();
        let resolver = resolver_with_file(&[
            (ACL_AUDIT_ENABLED_KEY, "false"),
            (ACL_AUDIT_INCLUDE_DENIED_KEY, "false"),
        ]);
        assert!(!acl_audit_enabled(&resolver));
        assert!(!acl_audit_include_denied(&resolver));
    }

    #[test]
    fn audit_flag_env_beats_apcore_yaml() {
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = AuditEnvGuard::new();
        guard.set(ACL_AUDIT_ENABLED_ENV, "true");
        let resolver = resolver_with_file(&[(ACL_AUDIT_ENABLED_KEY, "false")]);
        assert!(acl_audit_enabled(&resolver));
    }

    #[test]
    fn an_unparseable_audit_flag_keeps_the_default() {
        // Fail towards auditing: a typo must not silently switch the ACL
        // audit trail off.
        let _g = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let guard = AuditEnvGuard::new();
        guard.set(ACL_AUDIT_ENABLED_ENV, "maybe");
        let resolver = ConfigResolver::new(None, None);
        assert!(acl_audit_enabled(&resolver));
    }

    #[test]
    fn parse_config_bool_accepts_the_section_4_8_spelling_table() {
        for raw in ["true", "TRUE", " 1 ", "yes", "On"] {
            assert!(
                parse_config_bool(raw, "acl.audit.enabled", false),
                "should parse true: {raw:?}"
            );
        }
        for raw in ["false", "FALSE", " 0 ", "no", "Off"] {
            assert!(
                !parse_config_bool(raw, "acl.audit.enabled", true),
                "should parse false: {raw:?}"
            );
        }
    }

    #[test]
    fn zero_switches_a_flag_off_even_though_rust_cannot_parse_it_as_bool() {
        // The discriminating case for NOT delegating to `str::parse::<bool>()`:
        // Rust's `FromStr for bool` errors on "0", so a naive implementation
        // would leave APCORE_ACL_AUDIT_ENABLED=0 unable to switch auditing off
        // while the same value works in Python and TypeScript.
        assert!("0".parse::<bool>().is_err(), "premise of this test");
        assert!(!parse_config_bool("0", "acl.audit.enabled", true));
        assert!(parse_config_bool("1", "acl.audit.enabled", false));
    }

    // ----- audit record shape (spec section 4.8) -----

    #[test]
    fn the_audit_record_serialises_the_13_fields_in_declaration_order() {
        // Key order is normative: the log is JSONL, so an unspecified order
        // makes the same decision serialise to different bytes per SDK.
        // Asserted on the raw text, not on a parsed `Value` -- `serde_json`'s
        // object map re-sorts keys unless `preserve_order` happens to be
        // enabled in the dependency graph, so parsing would test the reader
        // rather than the writer.
        let line = serde_json::to_string(&acl_audit_record(&AuditEntry::default()))
            .expect("the record serialises");
        assert_eq!(
            ordered_keys(&line),
            ACL_AUDIT_FIELDS,
            "13 fields, apcore's AuditEntry declaration order, snake_case, no extras"
        );
        // Specifically: no `user`. That field belongs to FE-05 *execution*
        // records; adding it here would make the ACL record a CLI dialect that
        // no longer reads against apcore's `AuditEntry`.
        assert!(!ordered_keys(&line).contains(&"user".to_string()));
    }

    #[test]
    fn the_audit_record_writes_absent_optionals_as_null() {
        // apcore's `AuditEntry` carries `skip_serializing_if =
        // "Option::is_none"` on six optionals, so serialising it directly
        // would emit seven keys for a default entry. Section 4.8 forbids
        // dropping a field: an absent value is `null`, never a missing key.
        let record = acl_audit_record(&AuditEntry::default());
        let value = serde_json::to_value(&record).expect("serialises");
        for key in [
            "matched_rule",
            "matched_rule_index",
            "identity_type",
            "call_depth",
            "trace_id",
            "handler_error",
        ] {
            assert!(value[key].is_null(), "{key} must be null, not missing");
        }
        assert_eq!(value["approval_required"], json!(false));
        assert_eq!(value["roles"], json!([]));
    }

    #[test]
    fn the_audit_record_copies_values_verbatim() {
        let mut entry = AuditEntry::default();
        entry.timestamp = "2026-09-06T00:00:00+00:00".to_string();
        entry.caller_id = "@external".to_string();
        entry.target_id = "system.control.disable".to_string();
        entry.decision = "deny".to_string();
        entry.reason = "rule_match".to_string();
        entry.matched_rule = Some("no external control".to_string());
        entry.matched_rule_index = Some(0);
        entry.identity_type = Some("user".to_string());
        entry.roles = vec!["admin".to_string()];
        entry.call_depth = Some(2);
        entry.trace_id = Some("trace-1".to_string());
        entry.handler_error = Some("boom".to_string());
        entry.approval_required = true;

        let value = serde_json::to_value(acl_audit_record(&entry)).expect("serialises");
        assert_eq!(value["timestamp"], "2026-09-06T00:00:00+00:00");
        assert_eq!(value["caller_id"], "@external");
        assert_eq!(value["target_id"], "system.control.disable");
        assert_eq!(value["decision"], "deny");
        assert_eq!(value["reason"], "rule_match");
        assert_eq!(value["matched_rule"], "no external control");
        assert_eq!(value["matched_rule_index"], 0);
        assert_eq!(value["identity_type"], "user");
        assert_eq!(value["roles"], json!(["admin"]));
        assert_eq!(value["call_depth"], 2);
        assert_eq!(value["trace_id"], "trace-1");
        assert_eq!(value["handler_error"], "boom");
        assert_eq!(value["approval_required"], json!(true));
    }

    /// Top-level object keys of one JSON line, in the order they were written.
    ///
    /// Shared with `tests/test_acl_cmd.rs` in spirit: section 4.8 pins key
    /// order, and `serde_json::Value` cannot report it portably, so both the
    /// unit and the integration assertion read it off the raw text.
    fn ordered_keys(line: &str) -> Vec<String> {
        let mut keys = Vec::new();
        let mut pending: Option<String> = None;
        let mut depth: i32 = 0;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    // Consume the whole string so a colon inside a value
                    // (an RFC 3339 timestamp, say) is never read as a
                    // key/value separator.
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
    fn ordered_keys_reads_order_off_the_raw_text() {
        // The helper the order assertion leans on, pinned in turn: nested
        // objects, arrays and colons inside string values must not confuse it.
        let line = r#"{"b":"12:00","a":{"z":1},"c":["x:y"],"d":null}"#;
        assert_eq!(ordered_keys(line), vec!["b", "a", "c", "d"]);
    }
}
