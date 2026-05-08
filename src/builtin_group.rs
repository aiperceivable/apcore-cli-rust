//! Built-in Command Group (FE-13).
//!
//! Encapsulates visibility resolution and subcommand filtering for the
//! reserved `apcli` group. Instantiated once by the CLI bootstrap and
//! attached to the root command.
//!
//! Shape mirrors [`crate::exposure::ExposureFilter`]: a private constructor
//! with named factories and a small set of predicate methods.
//!
//! Protocol spec: FE-13 — see `../apcore-cli/docs/features/builtin-group.md`
//! sections §4.2–§4.7 and §4.14 for authoritative semantics.

use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

use regex::Regex;
use thiserror::Error;

use crate::EXIT_INVALID_INPUT;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Canonical set of apcli subcommand names.
///
/// Declarative mirror of the registration table wired in `main.rs`. Used by
/// the internal list normalizer to warn on unknown entries in include/exclude
/// lists (spec §7 error table / T-APCLI-25). Keep in sync if subcommands are
/// added or removed.
pub const APCLI_SUBCOMMAND_NAMES: &[&str] = &[
    "list",
    "describe",
    "exec",
    "validate",
    "init",
    "health",
    "usage",
    "enable",
    "disable",
    "reload",
    "config",
    "completion",
    "describe-pipeline",
];

/// Default name of the built-in command group.
///
/// Overridable per [`ApcliGroup`] instance via the `name` parameter on the
/// factories. Cross-SDK parity with Python `DEFAULT_BUILTIN_GROUP_NAME` and
/// TypeScript `DEFAULT_BUILTIN_GROUP_NAME` (2026-05-08).
pub const DEFAULT_BUILTIN_GROUP_NAME: &str = "apcli";

/// Group names reserved by apcore-cli when no rename is configured (default
/// list).
///
/// When [`set_reserved_group_names`] has been called the live reserved set is
/// the value passed there; downstream collision checks should consult
/// [`effective_reserved_group_names`] rather than this constant directly so
/// that a rename ([`DEFAULT_BUILTIN_GROUP_NAME`] → custom) is honored.
pub const RESERVED_GROUP_NAMES: &[&str] = &[DEFAULT_BUILTIN_GROUP_NAME];

/// Mutable cache of the currently effective reserved-group set, updated by
/// [`set_reserved_group_names`] from the binary entry-point once the
/// resolved built-in-group name is known. Defaults to a singleton set
/// containing [`DEFAULT_BUILTIN_GROUP_NAME`].
///
/// Cross-SDK parity (2026-05-08): mirrors the module-level
/// `_effectiveReservedNames` cell in TypeScript `builtin-group.ts`. The Rust
/// port keeps the data at module scope because the collision check in
/// `cli.rs::build_module_command_with_limit` is a free function rather than a
/// method on a per-instance group object.
static EFFECTIVE_RESERVED: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

fn effective_reserved_cell() -> &'static RwLock<HashSet<String>> {
    EFFECTIVE_RESERVED.get_or_init(|| {
        let mut s = HashSet::new();
        s.insert(DEFAULT_BUILTIN_GROUP_NAME.to_string());
        RwLock::new(s)
    })
}

/// Return a snapshot of the currently effective reserved-group set.
pub fn effective_reserved_group_names() -> HashSet<String> {
    effective_reserved_cell()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| {
            let mut s = HashSet::new();
            s.insert(DEFAULT_BUILTIN_GROUP_NAME.to_string());
            s
        })
}

/// True iff `name` is in the currently effective reserved-group set.
pub fn is_reserved_group_name(name: &str) -> bool {
    effective_reserved_cell()
        .read()
        .map(|guard| guard.contains(name))
        .unwrap_or(false)
}

/// Replace the effective reserved-group set with `names`. Called once from
/// the binary entry-point after the [`ApcliGroup`] is built so the
/// `cli.rs` collision check sees the renamed group's name.
pub fn set_reserved_group_names(names: HashSet<String>) {
    if let Ok(mut guard) = effective_reserved_cell().write() {
        *guard = names;
    }
}

/// Compiled regex for [`validate_builtin_group_name`]. Matches the same
/// shell-safe shape that downstream business-module group names must satisfy.
fn name_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("^[a-z][a-z0-9_-]*$").expect("static regex compiles"))
}

/// Validate a candidate built-in-group name. Returns
/// [`ApcliGroupError::InvalidName`] on rejection.
///
/// Cross-SDK parity: mirrors Python `_NAME_REGEX` and TypeScript
/// `_validateBuiltinGroupName` (2026-05-08).
pub fn validate_builtin_group_name(name: &str) -> Result<(), ApcliGroupError> {
    if name.is_empty() || !name_regex().is_match(name) {
        return Err(ApcliGroupError::InvalidName(name.to_string()));
    }
    Ok(())
}

// Valid user-supplied mode strings. Note: `"auto"` is an internal sentinel
// and is rejected from user-supplied config.
const VALID_USER_MODES: &[&str] = &["all", "none", "include", "exclude"];

// ---------------------------------------------------------------------------
// Public config types (spec §4.14)
// ---------------------------------------------------------------------------

/// User-facing apcli visibility mode.
///
/// The `Auto` variant is an internal sentinel meaning "fall through to
/// auto-detect"; it is never returned from [`ApcliGroup::resolve_visibility`]
/// and is rejected when supplied via user config.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ApcliMode {
    /// Default: auto-detect based on registry-injection (Tier 4).
    #[default]
    Auto,
    /// Show all apcli subcommands.
    All,
    /// Hide the entire apcli group.
    None,
    /// Whitelist of subcommand names to expose.
    Include(Vec<String>),
    /// Blacklist of subcommand names to hide.
    Exclude(Vec<String>),
}

/// User-facing apcli config consumed by [`ApcliGroup::from_cli_config`].
///
/// Boolean shorthand (handled at the yaml/builder layer) maps to
/// `mode = All` / `mode = None`.
#[derive(Clone, Debug, Default)]
pub struct ApcliConfig {
    /// Visibility mode.
    pub mode: ApcliMode,
    /// When true, the `APCORE_CLI_APCLI` env var (Tier 2) is ignored.
    pub disable_env: bool,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors surfaced by the fallible `from_yaml` builder.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ApcliGroupError {
    /// apcli config was neither a boolean nor a mapping.
    #[error("Error: apcli config must be a boolean or object; got {0}.")]
    InvalidShape(String),

    /// `mode` was not a string.
    #[error(
        "Error: apcli.mode must be a string; got {0}. \
         Expected one of all|none|include|exclude."
    )]
    ModeNotString(String),

    /// `mode` was a string but not one of the allowed values.
    #[error(
        "Error: apcli.mode '{0}' is invalid. \
         Expected one of all|none|include|exclude."
    )]
    ModeInvalid(String),

    /// Built-in group `name` failed the shell-safe regex check.
    #[error(
        "builtin_group_name '{0}' must match /^[a-z][a-z0-9_-]*$/ \
         (non-empty, lowercase, alphanumeric + '_' / '-', leading letter)."
    )]
    InvalidName(String),
}

// ---------------------------------------------------------------------------
// ApcliGroup
// ---------------------------------------------------------------------------

/// Visibility configuration for the built-in `apcli` command group.
///
/// Instantiated via [`ApcliGroup::from_cli_config`] (Tier 1) or
/// [`ApcliGroup::from_yaml`] (Tier 3). The constructor is private to
/// preserve the Tier-1-vs-Tier-3 flag distinction.
#[derive(Debug)]
pub struct ApcliGroup {
    mode: InternalMode,
    include: Vec<String>,
    exclude: Vec<String>,
    disable_env: bool,
    registry_injected: bool,
    from_cli_config: bool,
    name: String,
}

/// Internal flat mode sentinel. Unlike [`ApcliMode`], this does not carry
/// the list payload — `include` / `exclude` vectors live on the struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InternalMode {
    Auto,
    All,
    None,
    Include,
    Exclude,
}

impl InternalMode {
    fn as_str(self) -> &'static str {
        match self {
            InternalMode::Auto => "auto",
            InternalMode::All => "all",
            InternalMode::None => "none",
            InternalMode::Include => "include",
            InternalMode::Exclude => "exclude",
        }
    }
}

impl ApcliGroup {
    // -------------------------------------------------------------------------
    // Factories
    // -------------------------------------------------------------------------

    /// Tier 1 constructor — config came from a programmatic embedder
    /// (i.e. an `ApcliConfig` value passed to a future embedding API).
    ///
    /// A non-auto mode from this tier wins over env var and yaml. Because
    /// `ApcliConfig` is strongly typed, no validation is needed here. The
    /// resolved built-in-group name defaults to [`DEFAULT_BUILTIN_GROUP_NAME`];
    /// pass [`Self::from_cli_config_with_name`] for a custom name.
    pub fn from_cli_config(config: Option<ApcliConfig>, registry_injected: bool) -> Self {
        Self::from_cli_config_with_name(config, registry_injected, None)
            .expect("DEFAULT_BUILTIN_GROUP_NAME passes the regex check")
    }

    /// Tier 1 constructor with an explicit built-in-group name.
    ///
    /// `name = None` falls back to [`DEFAULT_BUILTIN_GROUP_NAME`]. Returns
    /// [`ApcliGroupError::InvalidName`] if the supplied name fails the
    /// shell-safe regex check (cross-SDK parity with Python `name=` kwarg
    /// and TypeScript `opts.name`, 2026-05-08).
    pub fn from_cli_config_with_name(
        config: Option<ApcliConfig>,
        registry_injected: bool,
        name: Option<String>,
    ) -> Result<Self, ApcliGroupError> {
        let resolved_name = name.unwrap_or_else(|| DEFAULT_BUILTIN_GROUP_NAME.to_string());
        validate_builtin_group_name(&resolved_name)?;

        let (mode, include, exclude, disable_env) = match config {
            None => (InternalMode::Auto, Vec::new(), Vec::new(), false),
            Some(cfg) => {
                let disable_env = cfg.disable_env;
                match cfg.mode {
                    ApcliMode::Auto => (InternalMode::Auto, Vec::new(), Vec::new(), disable_env),
                    ApcliMode::All => (InternalMode::All, Vec::new(), Vec::new(), disable_env),
                    ApcliMode::None => (InternalMode::None, Vec::new(), Vec::new(), disable_env),
                    ApcliMode::Include(list) => {
                        Self::warn_unknown_entries(&list, "include");
                        (InternalMode::Include, list, Vec::new(), disable_env)
                    }
                    ApcliMode::Exclude(list) => {
                        Self::warn_unknown_entries(&list, "exclude");
                        (InternalMode::Exclude, Vec::new(), list, disable_env)
                    }
                }
            }
        };

        Ok(Self {
            mode,
            include,
            exclude,
            disable_env,
            registry_injected,
            from_cli_config: true,
            name: resolved_name,
        })
    }

    /// Tier 3 constructor — config came from `apcore.yaml`.
    ///
    /// Env var (Tier 2) may override the yaml-supplied mode unless
    /// `disable_env` is true. On validation error, prints a message to stderr
    /// and calls [`std::process::exit`] with [`EXIT_INVALID_INPUT`]; the
    /// fallible variant [`ApcliGroup::try_from_yaml`] is available for tests.
    pub fn from_yaml(yaml_value: Option<serde_yaml_ng::Value>, registry_injected: bool) -> Self {
        Self::from_yaml_with_name(yaml_value, registry_injected, None)
    }

    /// Tier 3 constructor with an explicit built-in-group name. On
    /// validation failure (mode shape or invalid name) prints the error
    /// and calls [`std::process::exit`] with [`EXIT_INVALID_INPUT`].
    pub fn from_yaml_with_name(
        yaml_value: Option<serde_yaml_ng::Value>,
        registry_injected: bool,
        name: Option<String>,
    ) -> Self {
        match Self::try_from_yaml_with_name(yaml_value, registry_injected, name) {
            Ok(group) => group,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(EXIT_INVALID_INPUT);
            }
        }
    }

    /// Fallible sibling of [`ApcliGroup::from_yaml`]. Used by tests; the
    /// production wrapper prints the error and exits.
    pub fn try_from_yaml(
        yaml_value: Option<serde_yaml_ng::Value>,
        registry_injected: bool,
    ) -> Result<Self, ApcliGroupError> {
        Self::try_from_yaml_with_name(yaml_value, registry_injected, None)
    }

    /// Fallible Tier 3 constructor with explicit name.
    pub fn try_from_yaml_with_name(
        yaml_value: Option<serde_yaml_ng::Value>,
        registry_injected: bool,
        name: Option<String>,
    ) -> Result<Self, ApcliGroupError> {
        use serde_yaml_ng::Value;

        let resolved_name = name.unwrap_or_else(|| DEFAULT_BUILTIN_GROUP_NAME.to_string());
        validate_builtin_group_name(&resolved_name)?;

        // Missing / null → auto.
        let value = match yaml_value {
            None => return Ok(Self::auto(registry_injected, false, resolved_name)),
            Some(v) => v,
        };

        match value {
            Value::Null => Ok(Self::auto(registry_injected, false, resolved_name)),
            Value::Bool(true) => Ok(Self {
                mode: InternalMode::All,
                include: Vec::new(),
                exclude: Vec::new(),
                disable_env: false,
                registry_injected,
                from_cli_config: false,
                name: resolved_name,
            }),
            Value::Bool(false) => Ok(Self {
                mode: InternalMode::None,
                include: Vec::new(),
                exclude: Vec::new(),
                disable_env: false,
                registry_injected,
                from_cli_config: false,
                name: resolved_name,
            }),
            Value::Mapping(map) => Self::build_from_mapping(map, registry_injected, resolved_name),
            Value::Sequence(_) => Err(ApcliGroupError::InvalidShape("array".to_string())),
            Value::String(_) => Err(ApcliGroupError::InvalidShape("string".to_string())),
            Value::Number(_) => Err(ApcliGroupError::InvalidShape("number".to_string())),
            Value::Tagged(_) => Err(ApcliGroupError::InvalidShape("tagged".to_string())),
        }
    }

    fn auto(registry_injected: bool, from_cli_config: bool, name: String) -> Self {
        Self {
            mode: InternalMode::Auto,
            include: Vec::new(),
            exclude: Vec::new(),
            disable_env: false,
            registry_injected,
            from_cli_config,
            name,
        }
    }

    fn build_from_mapping(
        map: serde_yaml_ng::Mapping,
        registry_injected: bool,
        name: String,
    ) -> Result<Self, ApcliGroupError> {
        use serde_yaml_ng::Value;

        // Look up by string key. Skip (with warning) keys that are not
        // scalar strings — uncommon in yaml but technically legal.
        let get = |name: &str| -> Option<Value> {
            for (k, v) in &map {
                match k {
                    Value::String(s) if s == name => return Some(v.clone()),
                    _ => continue,
                }
            }
            None
        };

        // Warn once if there are any non-string keys.
        for (k, _) in &map {
            if !matches!(k, Value::String(_)) {
                tracing::warn!("apcli config has a non-string key; ignoring.");
                break;
            }
        }

        // Mode. Missing/null → Auto. Non-string or unknown string → error.
        let mode = match get("mode") {
            None | Some(Value::Null) => InternalMode::Auto,
            Some(Value::String(s)) => {
                if !VALID_USER_MODES.contains(&s.as_str()) {
                    return Err(ApcliGroupError::ModeInvalid(s));
                }
                match s.as_str() {
                    "all" => InternalMode::All,
                    "none" => InternalMode::None,
                    "include" => InternalMode::Include,
                    "exclude" => InternalMode::Exclude,
                    _ => unreachable!("VALID_USER_MODES check above"),
                }
            }
            Some(other) => {
                return Err(ApcliGroupError::ModeNotString(
                    yaml_type_name(&other).into(),
                ));
            }
        };

        let include = Self::normalize_list(get("include"), "include");
        let exclude = Self::normalize_list(get("exclude"), "exclude");

        // disable_env accepts both snake_case and camelCase.
        let raw_disable_env = get("disable_env").or_else(|| get("disableEnv"));
        let disable_env = match raw_disable_env {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => b,
            Some(other) => {
                tracing::warn!(
                    "apcli.disable_env must be boolean; got {}. Treating as false.",
                    yaml_type_name(&other)
                );
                false
            }
        };

        Ok(Self {
            mode,
            include,
            exclude,
            disable_env,
            registry_injected,
            from_cli_config: false,
            name,
        })
    }

    /// Normalize a yaml include/exclude list. Non-array → warn + empty.
    /// Unknown-but-well-formed entries emit a warning but are retained for
    /// forward compatibility (spec §7 / T-APCLI-25).
    fn normalize_list(raw: Option<serde_yaml_ng::Value>, label: &str) -> Vec<String> {
        use serde_yaml_ng::Value;
        let raw = match raw {
            None | Some(Value::Null) => return Vec::new(),
            Some(v) => v,
        };
        let seq = match raw {
            Value::Sequence(s) => s,
            other => {
                tracing::warn!(
                    "apcli.{} must be a list; got {}. Ignoring.",
                    label,
                    yaml_type_name(&other)
                );
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(seq.len());
        for entry in seq {
            match entry {
                Value::String(s) if !s.is_empty() => {
                    if !APCLI_SUBCOMMAND_NAMES.contains(&s.as_str()) {
                        tracing::warn!(
                            "Unknown apcli subcommand '{}' in {} list -- ignoring.",
                            s,
                            label
                        );
                    }
                    out.push(s);
                }
                _ => {
                    tracing::warn!("apcli.{} contains non-string entry; skipping.", label);
                }
            }
        }
        out
    }

    /// Emit the unknown-subcommand warnings for a strongly-typed list from
    /// `ApcliConfig` (Tier 1). Mirrors the yaml-path warnings so behaviour
    /// is identical regardless of where the config originated.
    fn warn_unknown_entries(list: &[String], label: &str) {
        for entry in list {
            if !APCLI_SUBCOMMAND_NAMES.contains(&entry.as_str()) {
                tracing::warn!(
                    "Unknown apcli subcommand '{}' in {} list -- ignoring.",
                    entry,
                    label
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // Public API
    // -------------------------------------------------------------------------

    /// Resolve effective visibility after applying the four-tier precedence.
    ///
    /// Returns one of `"all" | "none" | "include" | "exclude"` — never
    /// `"auto"`. Tier order (spec §4.4):
    ///
    /// 1. `from_cli_config` with a non-auto mode wins outright.
    /// 2. `APCORE_CLI_APCLI` env var (unless sealed by `disable_env`).
    /// 3. yaml non-auto mode.
    /// 4. Auto-detect: `registry_injected ? "none" : "all"`.
    pub fn resolve_visibility(&self) -> &'static str {
        // Tier 1 — programmatic embedder config (non-auto).
        if self.from_cli_config && self.mode != InternalMode::Auto {
            return self.mode.as_str();
        }

        // Tier 2 — env var (unless sealed).
        if !self.disable_env {
            if let Some(env_mode) = Self::parse_env(std::env::var("APCORE_CLI_APCLI").ok()) {
                return env_mode;
            }
        }

        // Tier 3 — yaml non-auto.
        if self.mode != InternalMode::Auto {
            return self.mode.as_str();
        }

        // Tier 4 — auto-detect.
        if self.registry_injected {
            "none"
        } else {
            "all"
        }
    }

    /// Return true iff `subcommand` passes the include/exclude filter.
    ///
    /// Callers MUST first check [`ApcliGroup::resolve_visibility`] — this
    /// method panics if called under modes `"all"` or `"none"` (caller bug
    /// per spec §4.6).
    pub fn is_subcommand_included(&self, subcommand: &str) -> bool {
        match self.resolve_visibility() {
            "include" => self.include.iter().any(|s| s == subcommand),
            "exclude" => !self.exclude.iter().any(|s| s == subcommand),
            other => unreachable!(
                "is_subcommand_included called under mode '{other}'; caller should bypass."
            ),
        }
    }

    /// True iff the `apcli` group itself should appear in root `--help`.
    pub fn is_group_visible(&self) -> bool {
        self.resolve_visibility() != "none"
    }

    /// Enumerate the effective include list. Empty unless resolved mode is
    /// `"include"`.
    pub fn include(&self) -> &[String] {
        &self.include
    }

    /// Enumerate the effective exclude list. Empty unless resolved mode is
    /// `"exclude"`.
    pub fn exclude(&self) -> &[String] {
        &self.exclude
    }

    /// True iff Tier 2 env-var lookup is sealed.
    pub fn disable_env(&self) -> bool {
        self.disable_env
    }

    /// Resolved name for the built-in command group (default
    /// [`DEFAULT_BUILTIN_GROUP_NAME`]). Cross-SDK parity with Python
    /// `ApcliGroup.name` and TypeScript `ApcliGroup#name` (2026-05-08).
    pub fn name(&self) -> &str {
        &self.name
    }

    // -------------------------------------------------------------------------
    // Env parser (Tier 2) — co-located per spec §4.4
    // -------------------------------------------------------------------------

    /// Parse `APCORE_CLI_APCLI`. Case-insensitive and trim-on-read
    /// (spec invariant 2 — `apcore-cli/docs/features/builtin-group.md`).
    ///
    /// - `show` / `1` / `true` → `Some("all")`
    /// - `hide` / `0` / `false` → `Some("none")`
    /// - Empty / whitespace-only / unset → `None`
    /// - Anything else → warn and return `None`
    fn parse_env(raw: Option<String>) -> Option<&'static str> {
        let raw = raw?;
        if raw.is_empty() {
            return None;
        }
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() {
            return None;
        }
        match normalized.as_str() {
            "show" | "1" | "true" => Some("all"),
            "hide" | "0" | "false" => Some("none"),
            _ => {
                tracing::warn!(
                    "Unknown APCORE_CLI_APCLI value '{}', ignoring. \
                     Expected: show, hide, 1, 0, true, false.",
                    raw
                );
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn yaml_type_name(v: &serde_yaml_ng::Value) -> &'static str {
    use serde_yaml_ng::Value;
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "array",
        Value::Mapping(_) => "object",
        Value::Tagged(_) => "tagged",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml_ng::Value;
    use std::sync::Mutex;

    /// Serializes tests that set/unset `APCORE_CLI_APCLI`. Same pattern as
    /// the `resolve_log_level` tests in `main.rs`.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn clear_env() {
        // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
        unsafe {
            std::env::remove_var("APCORE_CLI_APCLI");
        }
    }

    fn set_env(val: &str) {
        // SAFETY: test-only env manipulation, serialized via ENV_MUTEX.
        unsafe {
            std::env::set_var("APCORE_CLI_APCLI", val);
        }
    }

    // ----- Constants -----

    #[test]
    fn apcli_subcommand_names_has_13_entries() {
        assert_eq!(APCLI_SUBCOMMAND_NAMES.len(), 13);
    }

    #[test]
    fn apcli_subcommand_names_contents() {
        for expected in &[
            "list",
            "describe",
            "exec",
            "validate",
            "init",
            "health",
            "usage",
            "enable",
            "disable",
            "reload",
            "config",
            "completion",
            "describe-pipeline",
        ] {
            assert!(
                APCLI_SUBCOMMAND_NAMES.contains(expected),
                "missing: {expected}"
            );
        }
    }

    #[test]
    fn reserved_group_names_contents() {
        assert_eq!(RESERVED_GROUP_NAMES, &["apcli"]);
    }

    // ----- Tier 1: from_cli_config -----

    #[test]
    fn from_cli_config_all_wins_in_embedded() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_cli_config(
            Some(ApcliConfig {
                mode: ApcliMode::All,
                disable_env: false,
            }),
            /*registry_injected*/ true,
        );
        assert_eq!(group.resolve_visibility(), "all");
    }

    #[test]
    fn from_cli_config_none_default_standalone_autodetect_all() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_cli_config(None, /*registry_injected*/ false);
        assert_eq!(group.resolve_visibility(), "all");
    }

    #[test]
    fn from_cli_config_none_default_embedded_autodetect_none() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_cli_config(None, /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "none");
    }

    #[test]
    fn from_cli_config_none_mode_beats_env_show() {
        // Tier 1 (None) > Tier 2 (env=show). Even without disable_env, an
        // explicit Tier-1 (programmatic) mode wins — defence in depth.
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("show");
        let group = ApcliGroup::from_cli_config(
            Some(ApcliConfig {
                mode: ApcliMode::None,
                disable_env: false,
            }),
            /*registry_injected*/ false,
        );
        assert_eq!(group.resolve_visibility(), "none");
        clear_env();
    }

    // ----- Tier 3: from_yaml (bool shorthand) -----

    #[test]
    fn from_yaml_bool_true_embedded_all() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_yaml(Some(Value::Bool(true)), /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "all");
    }

    #[test]
    fn from_yaml_bool_false_standalone_none() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group =
            ApcliGroup::from_yaml(Some(Value::Bool(false)), /*registry_injected*/ false);
        assert_eq!(group.resolve_visibility(), "none");
    }

    #[test]
    fn from_yaml_null_value_auto() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_yaml(Some(Value::Null), /*registry_injected*/ false);
        assert_eq!(group.resolve_visibility(), "all");
    }

    #[test]
    fn from_yaml_none_auto() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_yaml(None, /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "none");
    }

    // ----- Tier 2: env var overrides -----

    #[test]
    fn from_yaml_null_env_show_all() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("show");
        let group = ApcliGroup::from_yaml(None, /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "all");
        clear_env();
    }

    #[test]
    fn from_yaml_null_env_hide_none() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("hide");
        let group = ApcliGroup::from_yaml(None, /*registry_injected*/ false);
        assert_eq!(group.resolve_visibility(), "none");
        clear_env();
    }

    #[test]
    fn from_yaml_mode_none_env_show_env_wins() {
        // Tier 2 > Tier 3 when disable_env is false.
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("show");
        let yaml: Value = serde_yaml_ng::from_str("mode: none").unwrap();
        let group = ApcliGroup::from_yaml(Some(yaml), /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "all");
        clear_env();
    }

    #[test]
    fn from_yaml_mode_none_disable_env_env_show_yaml_wins() {
        // disable_env: true seals Tier 2 — yaml mode:none wins.
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("show");
        let yaml: Value = serde_yaml_ng::from_str("mode: none\ndisable_env: true").unwrap();
        let group = ApcliGroup::from_yaml(Some(yaml), /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "none");
        clear_env();
    }

    #[test]
    fn from_yaml_disable_env_camel_case_also_accepted() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("show");
        let yaml: Value = serde_yaml_ng::from_str("mode: none\ndisableEnv: true").unwrap();
        let group = ApcliGroup::from_yaml(Some(yaml), /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "none");
        clear_env();
    }

    #[test]
    fn env_case_insensitive_show() {
        let _g = ENV_MUTEX.lock().unwrap();
        for raw in &["SHOW", "Show", "sHoW"] {
            set_env(raw);
            let group = ApcliGroup::from_yaml(None, true);
            assert_eq!(group.resolve_visibility(), "all", "raw={raw}");
        }
        clear_env();
    }

    #[test]
    fn env_case_insensitive_true_hide_false_numeric() {
        let _g = ENV_MUTEX.lock().unwrap();
        for (raw, expected) in &[
            ("True", "all"),
            ("TRUE", "all"),
            ("HIDE", "none"),
            ("False", "none"),
            ("1", "all"),
            ("0", "none"),
        ] {
            set_env(raw);
            let group = ApcliGroup::from_yaml(None, true);
            assert_eq!(group.resolve_visibility(), *expected, "raw={raw}");
        }
        clear_env();
    }

    #[test]
    fn env_unknown_value_falls_through() {
        // parse_env with a bogus value returns None (after warning) — so
        // Tier 4 auto-detect takes over.
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("bogus");
        let group = ApcliGroup::from_yaml(None, /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "none");
        clear_env();
    }

    #[test]
    fn env_empty_string_treated_as_unset() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("");
        let group = ApcliGroup::from_yaml(None, /*registry_injected*/ false);
        assert_eq!(group.resolve_visibility(), "all");
        clear_env();
    }

    #[test]
    fn env_value_is_trimmed_on_read() {
        // Spec invariant 2 — env-var parser is case-insensitive AND trim-on-read.
        // Surrounding whitespace must not flip a recognized value into the
        // "unknown" warn-and-fallthrough branch. Cross-SDK parity with Python
        // and TypeScript (D10-info-1).
        let _g = ENV_MUTEX.lock().unwrap();
        for (raw, expected) in &[
            ("  show  ", "all"),
            ("\tshow\n", "all"),
            (" hide ", "none"),
            ("  TRUE  ", "all"),
            ("  false  ", "none"),
            ("  1  ", "all"),
            ("  0  ", "none"),
        ] {
            set_env(raw);
            let group = ApcliGroup::from_yaml(None, /*registry_injected*/ true);
            assert_eq!(group.resolve_visibility(), *expected, "raw={raw:?}");
        }
        clear_env();
    }

    #[test]
    fn env_whitespace_only_treated_as_unset() {
        // After trim, "   " collapses to empty, which must be treated like
        // "unset" rather than warned-as-unknown.
        let _g = ENV_MUTEX.lock().unwrap();
        set_env("   ");
        let group = ApcliGroup::from_yaml(None, /*registry_injected*/ false);
        // No env contribution → Tier 4 standalone auto-detect → "all".
        assert_eq!(group.resolve_visibility(), "all");
        clear_env();
    }

    // ----- Include / Exclude semantics -----

    #[test]
    fn include_mode_filters_correctly() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let yaml: Value =
            serde_yaml_ng::from_str("mode: include\ninclude:\n  - list\n  - describe").unwrap();
        let group = ApcliGroup::from_yaml(Some(yaml), /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "include");
        assert!(group.is_subcommand_included("list"));
        assert!(group.is_subcommand_included("describe"));
        assert!(!group.is_subcommand_included("init"));
        assert!(!group.is_subcommand_included("exec"));
        assert_eq!(group.include(), &["list", "describe"]);
        assert!(group.exclude().is_empty());
    }

    #[test]
    fn exclude_mode_filters_correctly() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let yaml: Value = serde_yaml_ng::from_str("mode: exclude\nexclude:\n  - init").unwrap();
        let group = ApcliGroup::from_yaml(Some(yaml), /*registry_injected*/ true);
        assert_eq!(group.resolve_visibility(), "exclude");
        assert!(!group.is_subcommand_included("init"));
        assert!(group.is_subcommand_included("list"));
        assert!(group.is_subcommand_included("describe"));
        assert!(group.include().is_empty());
        assert_eq!(group.exclude(), &["init"]);
    }

    #[test]
    fn from_cli_config_include_variant_filters_correctly() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let group = ApcliGroup::from_cli_config(
            Some(ApcliConfig {
                mode: ApcliMode::Include(vec!["list".into(), "describe".into()]),
                disable_env: false,
            }),
            /*registry_injected*/ true,
        );
        assert_eq!(group.resolve_visibility(), "include");
        assert!(group.is_subcommand_included("list"));
        assert!(!group.is_subcommand_included("init"));
    }

    // ----- Group visibility -----

    #[test]
    fn is_group_visible_false_only_for_none_mode() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let hidden = ApcliGroup::from_cli_config(
            Some(ApcliConfig {
                mode: ApcliMode::None,
                disable_env: false,
            }),
            true,
        );
        assert!(!hidden.is_group_visible());

        let shown = ApcliGroup::from_cli_config(
            Some(ApcliConfig {
                mode: ApcliMode::All,
                disable_env: false,
            }),
            true,
        );
        assert!(shown.is_group_visible());

        let include = ApcliGroup::from_cli_config(
            Some(ApcliConfig {
                mode: ApcliMode::Include(vec!["list".into()]),
                disable_env: false,
            }),
            true,
        );
        assert!(include.is_group_visible());
    }

    // ----- Validation errors -----

    #[test]
    fn try_from_yaml_rejects_mode_auto() {
        // Even though Auto is the internal default, user-supplied "auto"
        // is rejected per spec §4.2.
        let yaml: Value = serde_yaml_ng::from_str("mode: auto").unwrap();
        let err = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap_err();
        assert!(matches!(err, ApcliGroupError::ModeInvalid(ref s) if s == "auto"));
    }

    #[test]
    fn try_from_yaml_rejects_unknown_mode() {
        let yaml: Value = serde_yaml_ng::from_str("mode: whitelist").unwrap();
        let err = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap_err();
        assert!(matches!(err, ApcliGroupError::ModeInvalid(_)));
    }

    #[test]
    fn try_from_yaml_rejects_non_string_mode() {
        let yaml: Value = serde_yaml_ng::from_str("mode: 42").unwrap();
        let err = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap_err();
        assert!(matches!(err, ApcliGroupError::ModeNotString(_)));
    }

    #[test]
    fn try_from_yaml_rejects_array_shape() {
        let yaml: Value = serde_yaml_ng::from_str("- a\n- b").unwrap();
        let err = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap_err();
        assert!(matches!(err, ApcliGroupError::InvalidShape(ref s) if s == "array"));
    }

    #[test]
    fn try_from_yaml_rejects_string_shape() {
        let yaml = Value::String("oops".into());
        let err = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap_err();
        assert!(matches!(err, ApcliGroupError::InvalidShape(ref s) if s == "string"));
    }

    // ----- Tier-3 object form with extras -----

    #[test]
    fn try_from_yaml_object_without_mode_is_auto() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let yaml: Value = serde_yaml_ng::from_str("disable_env: true").unwrap();
        let group = ApcliGroup::try_from_yaml(Some(yaml), /*registry_injected*/ false).unwrap();
        // Tier 3 falls to Tier 4 auto-detect → standalone → "all".
        assert_eq!(group.resolve_visibility(), "all");
        assert!(group.disable_env());
    }

    #[test]
    fn try_from_yaml_include_non_array_warns_and_empty() {
        let _g = ENV_MUTEX.lock().unwrap();
        clear_env();
        let yaml: Value = serde_yaml_ng::from_str("mode: include\ninclude: not-a-list").unwrap();
        let group = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap();
        assert_eq!(group.resolve_visibility(), "include");
        assert!(group.include().is_empty());
    }

    #[test]
    fn try_from_yaml_unknown_include_entry_retained() {
        let yaml: Value =
            serde_yaml_ng::from_str("mode: include\ninclude:\n  - list\n  - bogus").unwrap();
        let group = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap();
        // Unknown entry is retained for forward-compat.
        assert_eq!(group.include(), &["list", "bogus"]);
    }

    #[test]
    fn try_from_yaml_disable_env_non_bool_treated_as_false() {
        let yaml: Value =
            serde_yaml_ng::from_str("mode: none\ndisable_env: \"yes-please\"").unwrap();
        let group = ApcliGroup::try_from_yaml(Some(yaml), true).unwrap();
        assert!(!group.disable_env());
    }
}
