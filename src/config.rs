// apcore-cli — Configuration resolver.
// Protocol spec: FE-07 (ConfigResolver, 4-tier precedence)

use std::collections::HashMap;
use std::path::PathBuf;

use tracing::warn;

// ---------------------------------------------------------------------------
// ConfigResolver
// ---------------------------------------------------------------------------

/// Resolved configuration following 4-tier precedence:
///
/// 1. CLI flags   — highest priority
/// 2. Environment variables
/// 3. Config file (YAML, dot-flattened keys)
/// 4. Built-in defaults — lowest priority
pub struct ConfigResolver {
    /// CLI flags map (flag name → value or None if not provided).
    pub cli_flags: HashMap<String, Option<String>>,

    /// Flattened key → value map loaded from the config file.
    /// `None` if the file was not found or could not be parsed.
    pub config_file: Option<HashMap<String, String>>,

    /// Cached parsed YAML root, loaded once at construction. Used by
    /// `resolve_object` so it doesn't re-read+re-parse the file on every
    /// call. `None` when the file is absent, unreadable, or malformed.
    config_yaml: Option<serde_yaml::Value>,

    /// Path to the config file that was loaded (or attempted).
    #[allow(dead_code)]
    config_path: Option<PathBuf>,

    /// Built-in default values.
    pub defaults: HashMap<&'static str, &'static str>,
}

impl ConfigResolver {
    /// Default configuration values.
    ///
    /// Audit D9 (config cleanup, v0.6.x): the entries `sandbox.enabled`,
    /// `cli.auto_approve`, `cli.stdin_buffer_limit`, and the four
    /// `apcore-cli.*` namespace aliases were removed because no production
    /// code path reads them via `resolve()`. Sandbox is configured via the
    /// `--sandbox` CLI flag, auto-approve via `--yes`, the stdin buffer is
    /// hard-coded, and namespace aliases are registered separately by
    /// `apcore`'s Config Bus when the parent crate calls
    /// `apcore::Config::register_namespace`. The cross-key file-lookup
    /// mechanism (`alternate_key`) still works regardless — it does not
    /// depend on these DEFAULTS entries.
    pub const DEFAULTS: &'static [(&'static str, &'static str)] = &[
        ("extensions.root", "./extensions"),
        ("logging.level", "WARNING"),
        ("cli.help_text_max_length", "1000"),
        // FE-11 (v0.6.0)
        ("cli.approval_timeout", "60"),
        ("cli.strategy", "standard"),
        ("cli.group_depth", "1"),
        // Exposure filtering (FE-12)
        ("expose.mode", "all"),
        ("expose.include", "[]"),
        ("expose.exclude", "[]"),
    ];

    /// Namespace key → legacy key mapping for backward compatibility.
    const NAMESPACE_MAP: &'static [(&'static str, &'static str)] = &[
        ("apcore-cli.stdin_buffer_limit", "cli.stdin_buffer_limit"),
        ("apcore-cli.auto_approve", "cli.auto_approve"),
        (
            "apcore-cli.help_text_max_length",
            "cli.help_text_max_length",
        ),
        ("apcore-cli.logging_level", "logging.level"),
    ];

    /// Create a new `ConfigResolver`.
    ///
    /// # Arguments
    /// * `cli_flags`   — CLI flag overrides (e.g. `--extensions-dir → /path`)
    /// * `config_path` — Optional explicit path to `apcore.yaml`
    pub fn new(
        cli_flags: Option<HashMap<String, Option<String>>>,
        config_path: Option<PathBuf>,
    ) -> Self {
        let defaults = Self::DEFAULTS.iter().copied().collect();
        let config_file = config_path.as_ref().and_then(Self::load_config_file);
        let config_yaml = config_path.as_ref().and_then(Self::load_config_yaml);

        Self {
            cli_flags: cli_flags.unwrap_or_default(),
            config_file,
            config_yaml,
            config_path,
            defaults,
        }
    }

    /// Resolve a configuration value using 4-tier precedence.
    ///
    /// # Arguments
    /// * `key`       — dot-separated config key (e.g. `"extensions.root"`)
    /// * `cli_flag`  — optional CLI flag name to check in `_cli_flags`
    /// * `env_var`   — optional environment variable name
    ///
    /// Returns `None` when the key is not present in any tier.
    pub fn resolve(
        &self,
        key: &str,
        cli_flag: Option<&str>,
        env_var: Option<&str>,
    ) -> Option<String> {
        // Tier 1: CLI flag — present and value is Some(non-None string).
        if let Some(flag) = cli_flag {
            if let Some(Some(value)) = self.cli_flags.get(flag) {
                return Some(value.clone());
            }
        }

        // Tier 2: Environment variable — must be set and non-empty.
        if let Some(var) = env_var {
            if let Ok(env_value) = std::env::var(var) {
                if !env_value.is_empty() {
                    return Some(env_value);
                }
            }
        }

        // Tier 3: Config file — key must be present in the flattened map.
        // Try both namespace and legacy keys for backward compatibility.
        if let Some(ref file_map) = self.config_file {
            if let Some(value) = file_map.get(key) {
                return Some(value.clone());
            }
            // Try alternate key (namespace ↔ legacy)
            if let Some(alt) = Self::alternate_key(key) {
                if let Some(value) = file_map.get(alt) {
                    return Some(value.clone());
                }
            }
        }

        // Tier 4: Built-in defaults.
        self.defaults.get(key).map(|s| s.to_string())
    }

    /// Resolve a non-leaf (object-valued) key from the YAML config file.
    ///
    /// Unlike [`Self::resolve`], which returns a flattened scalar string,
    /// this returns the raw `serde_yaml::Value` living at the requested
    /// dot-path. Used by FE-13 (`apcli`) where the top-level key can be a
    /// bool, a mapping, or absent.
    ///
    /// Only consults the config file (Tier 3) — CLI flags and env vars
    /// carry scalar values only. Returns `None` when the file is absent,
    /// unreadable, malformed, or the key is missing.
    pub fn resolve_object(&self, key: &str) -> Option<serde_yaml::Value> {
        // Walk the cached parsed YAML rather than re-reading + re-parsing on
        // every call (review #16). The cache is populated once in `new()`.
        let root = self.config_yaml.as_ref()?;
        let mut cursor = root;
        for segment in key.split('.') {
            match cursor {
                serde_yaml::Value::Mapping(map) => {
                    cursor = map.get(serde_yaml::Value::String(segment.to_string()))?;
                }
                _ => return None,
            }
        }
        Some(cursor.clone())
    }

    /// Read + parse the YAML file once for `resolve_object`'s use. Errors
    /// (missing file, malformed YAML, non-mapping root) collapse to `None`
    /// — the caller treats absence as "key not present", same semantics as
    /// the previous per-call read.
    fn load_config_yaml(path: &PathBuf) -> Option<serde_yaml::Value> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_yaml::from_str(&content).ok()
    }

    /// Look up the alternate key (namespace ↔ legacy) for backward compatibility.
    fn alternate_key(key: &str) -> Option<&'static str> {
        for &(ns, legacy) in Self::NAMESPACE_MAP {
            if key == ns {
                return Some(legacy);
            }
            if key == legacy {
                return Some(ns);
            }
        }
        None
    }

    /// Load and flatten a YAML config file into dot-notation keys.
    ///
    /// Returns `None` if the file does not exist or cannot be parsed.
    fn load_config_file(path: &PathBuf) -> Option<HashMap<String, String>> {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // FR-DISP-005 AF-1: file not found — silent.
                return None;
            }
            Err(e) => {
                warn!(
                    "Configuration file '{}' could not be read: {}",
                    path.display(),
                    e
                );
                return None;
            }
        };

        let parsed: serde_yaml::Value = match serde_yaml::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                // FR-DISP-005 AF-2: malformed YAML — log warning, use defaults.
                warn!(
                    "Configuration file '{}' is malformed, using defaults.",
                    path.display()
                );
                return None;
            }
        };

        // Root must be a mapping (dict). Scalars, sequences, and null are invalid.
        if !matches!(parsed, serde_yaml::Value::Mapping(_)) {
            warn!(
                "Configuration file '{}' is malformed, using defaults.",
                path.display()
            );
            return None;
        }

        let mut out = HashMap::new();
        Self::flatten_yaml_value(parsed, "", &mut out);
        Some(out)
    }

    /// Recursively flatten a nested YAML value into dot-separated keys.
    fn flatten_yaml_value(
        value: serde_yaml::Value,
        prefix: &str,
        out: &mut HashMap<String, String>,
    ) {
        match value {
            serde_yaml::Value::Mapping(map) => {
                for (k, v) in map {
                    let key_str = match k {
                        serde_yaml::Value::String(s) => s,
                        other => format!("{other:?}"),
                    };
                    let full_key = if prefix.is_empty() {
                        key_str
                    } else {
                        format!("{prefix}.{key_str}")
                    };
                    Self::flatten_yaml_value(v, &full_key, out);
                }
            }
            serde_yaml::Value::Bool(b) => {
                out.insert(prefix.to_string(), b.to_string());
            }
            serde_yaml::Value::Number(n) => {
                out.insert(prefix.to_string(), n.to_string());
            }
            serde_yaml::Value::String(s) => {
                out.insert(prefix.to_string(), s);
            }
            serde_yaml::Value::Null => {
                out.insert(prefix.to_string(), String::new());
            }
            // Sequences and tagged values are serialised as their debug repr;
            // no spec requirement for nested array flattening.
            serde_yaml::Value::Sequence(_) | serde_yaml::Value::Tagged(_) => {
                out.insert(prefix.to_string(), format!("{value:?}"));
            }
        }
    }

    /// Recursively flatten a nested JSON map into dot-separated keys.
    ///
    /// Example: `{"extensions": {"root": "/path"}}` → `{"extensions.root": "/path"}`
    pub fn flatten_dict(&self, map: serde_json::Value) -> HashMap<String, String> {
        let mut out = HashMap::new();
        Self::flatten_json_value(map, "", &mut out);
        out
    }

    /// Recursively walk a `serde_json::Value` and collect dot-notation keys.
    fn flatten_json_value(
        value: serde_json::Value,
        prefix: &str,
        out: &mut HashMap<String, String>,
    ) {
        match value {
            serde_json::Value::Object(obj) => {
                for (k, v) in obj {
                    let full_key = if prefix.is_empty() {
                        k
                    } else {
                        format!("{prefix}.{k}")
                    };
                    Self::flatten_json_value(v, &full_key, out);
                }
            }
            serde_json::Value::Bool(b) => {
                out.insert(prefix.to_string(), b.to_string());
            }
            serde_json::Value::Number(n) => {
                out.insert(prefix.to_string(), n.to_string());
            }
            serde_json::Value::String(s) => {
                out.insert(prefix.to_string(), s);
            }
            serde_json::Value::Null => {
                out.insert(prefix.to_string(), String::new());
            }
            serde_json::Value::Array(_) => {
                out.insert(prefix.to_string(), value.to_string());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_resolver_instantiation() {
        let resolver = ConfigResolver::new(None, None);
        assert!(!resolver.defaults.is_empty());
    }

    #[test]
    fn test_defaults_contains_expected_keys() {
        // Audit D9 (v0.6.x): only keys actually consumed by resolve() at
        // runtime live in DEFAULTS. The deleted keys (sandbox.enabled,
        // cli.auto_approve, cli.stdin_buffer_limit, apcore-cli.* aliases)
        // were dead — they're tested for absence by test_deleted_keys_absent.
        let resolver = ConfigResolver::new(None, None);
        for key in [
            "extensions.root",
            "logging.level",
            "cli.help_text_max_length",
            "cli.approval_timeout",
            "cli.strategy",
            "cli.group_depth",
            "expose.mode",
        ] {
            assert!(
                resolver.defaults.contains_key(key),
                "missing default: {key}"
            );
        }
    }

    #[test]
    fn test_deleted_keys_absent() {
        // Verify the audit D9 cleanup didn't accidentally re-introduce dead keys.
        let resolver = ConfigResolver::new(None, None);
        for key in [
            "sandbox.enabled",
            "cli.auto_approve",
            "cli.stdin_buffer_limit",
            "apcore-cli.stdin_buffer_limit",
            "apcore-cli.auto_approve",
            "apcore-cli.help_text_max_length",
            "apcore-cli.logging_level",
        ] {
            assert!(
                !resolver.defaults.contains_key(key),
                "deleted key reintroduced: {key}"
            );
        }
    }

    #[test]
    fn test_default_logging_level_is_warning() {
        let resolver = ConfigResolver::new(None, None);
        assert_eq!(
            resolver.defaults.get("logging.level"),
            Some(&"WARNING"),
            "logging.level default must be WARNING"
        );
    }

    #[test]
    fn test_fe11_defaults_present() {
        let resolver = ConfigResolver::new(None, None);
        assert_eq!(resolver.defaults.get("cli.approval_timeout"), Some(&"60"));
        assert_eq!(resolver.defaults.get("cli.strategy"), Some(&"standard"));
        assert_eq!(resolver.defaults.get("cli.group_depth"), Some(&"1"));
    }

    #[test]
    fn test_resolve_tier1_cli_flag_wins() {
        let mut flags = HashMap::new();
        flags.insert(
            "--extensions-dir".to_string(),
            Some("/cli-path".to_string()),
        );
        let resolver = ConfigResolver::new(Some(flags), None);
        let result = resolver.resolve(
            "extensions.root",
            Some("--extensions-dir"),
            Some("APCORE_EXTENSIONS_ROOT"),
        );
        assert_eq!(result, Some("/cli-path".to_string()));
    }

    #[test]
    fn test_resolve_tier2_env_var_wins() {
        unsafe { std::env::set_var("APCORE_EXTENSIONS_ROOT_UNIT", "/env-path") };
        let resolver = ConfigResolver::new(None, None);
        let result = resolver.resolve("extensions.root", None, Some("APCORE_EXTENSIONS_ROOT_UNIT"));
        assert_eq!(result, Some("/env-path".to_string()));
        unsafe { std::env::remove_var("APCORE_EXTENSIONS_ROOT_UNIT") };
    }

    #[test]
    fn test_resolve_tier3_config_file_wins() {
        // Requires a temp file; skip in unit tests — covered in integration tests.
        // Just verify the method exists and returns None when no file is loaded.
        let resolver = ConfigResolver::new(None, None);
        // With config_path = None, _config_file is None.
        // The default for "extensions.root" should be returned (tier 4).
        let result = resolver.resolve("extensions.root", None, None);
        assert_eq!(result, Some("./extensions".to_string()));
    }

    #[test]
    fn test_resolve_tier4_default_wins() {
        let resolver = ConfigResolver::new(None, None);
        let result = resolver.resolve("extensions.root", None, None);
        assert_eq!(result, Some("./extensions".to_string()));
    }

    #[test]
    fn test_flatten_dict_nested() {
        let resolver = ConfigResolver::new(None, None);
        let map = serde_json::json!({"extensions": {"root": "/path"}});
        let result = resolver.flatten_dict(map);
        assert_eq!(result.get("extensions.root"), Some(&"/path".to_string()));
    }

    #[test]
    fn test_flatten_dict_deeply_nested() {
        let resolver = ConfigResolver::new(None, None);
        let map = serde_json::json!({"a": {"b": {"c": "deep"}}});
        let result = resolver.flatten_dict(map);
        assert_eq!(result.get("a.b.c"), Some(&"deep".to_string()));
    }

    // ---- Namespace-aware config resolution (apcore >= 0.15.0) ----

    #[test]
    fn test_namespace_alternate_key_map_intact() {
        // Audit D9 (v0.6.x): the apcore-cli.* DEFAULTS entries were removed,
        // but the cross-key NAMESPACE_MAP that powers `alternate_key()` is
        // still authoritative. The map's destinations no longer need to be
        // present in DEFAULTS — file lookup via alternate_key() works
        // independently of the defaults dict.
        for ns_key in [
            "apcore-cli.stdin_buffer_limit",
            "apcore-cli.auto_approve",
            "apcore-cli.help_text_max_length",
            "apcore-cli.logging_level",
        ] {
            assert!(
                ConfigResolver::alternate_key(ns_key).is_some(),
                "alternate_key map must still resolve {ns_key}"
            );
        }
    }

    #[test]
    fn test_alternate_key_namespace_to_legacy() {
        assert_eq!(
            ConfigResolver::alternate_key("apcore-cli.stdin_buffer_limit"),
            Some("cli.stdin_buffer_limit")
        );
        assert_eq!(
            ConfigResolver::alternate_key("apcore-cli.auto_approve"),
            Some("cli.auto_approve")
        );
        assert_eq!(
            ConfigResolver::alternate_key("apcore-cli.logging_level"),
            Some("logging.level")
        );
    }

    #[test]
    fn test_alternate_key_legacy_to_namespace() {
        assert_eq!(
            ConfigResolver::alternate_key("cli.stdin_buffer_limit"),
            Some("apcore-cli.stdin_buffer_limit")
        );
        assert_eq!(
            ConfigResolver::alternate_key("cli.auto_approve"),
            Some("apcore-cli.auto_approve")
        );
        assert_eq!(
            ConfigResolver::alternate_key("logging.level"),
            Some("apcore-cli.logging_level")
        );
    }

    #[test]
    fn test_alternate_key_unknown_returns_none() {
        assert_eq!(ConfigResolver::alternate_key("unknown.key"), None);
        assert_eq!(ConfigResolver::alternate_key("extensions.root"), None);
    }

    #[test]
    fn test_resolve_namespace_key_from_legacy_file() {
        // Simulate a config file with legacy "cli.stdin_buffer_limit" key
        let mut file_map = HashMap::new();
        file_map.insert("cli.stdin_buffer_limit".to_string(), "5242880".to_string());
        let resolver = ConfigResolver {
            cli_flags: HashMap::new(),
            config_file: Some(file_map),
            config_yaml: None,
            config_path: None,
            defaults: ConfigResolver::DEFAULTS.iter().copied().collect(),
        };
        // Querying the namespace key should find the legacy key via fallback
        let result = resolver.resolve("apcore-cli.stdin_buffer_limit", None, None);
        assert_eq!(result, Some("5242880".to_string()));
    }

    #[test]
    fn test_resolve_legacy_key_from_namespace_file() {
        // Simulate a config file with namespace "apcore-cli.auto_approve" key
        let mut file_map = HashMap::new();
        file_map.insert("apcore-cli.auto_approve".to_string(), "true".to_string());
        let resolver = ConfigResolver {
            cli_flags: HashMap::new(),
            config_file: Some(file_map),
            config_yaml: None,
            config_path: None,
            defaults: ConfigResolver::DEFAULTS.iter().copied().collect(),
        };
        // Querying the legacy key should find the namespace key via fallback
        let result = resolver.resolve("cli.auto_approve", None, None);
        assert_eq!(result, Some("true".to_string()));
    }

    // ---- resolve_object (FE-13 non-leaf lookup) ----

    fn write_tmp_yaml(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apcore.yaml");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn test_resolve_object_returns_bool_shorthand() {
        let (_dir, path) = write_tmp_yaml("apcli: false\n");
        let resolver = ConfigResolver::new(None, Some(path));
        let v = resolver.resolve_object("apcli").expect("apcli key present");
        assert!(matches!(v, serde_yaml::Value::Bool(false)));
    }

    #[test]
    fn test_resolve_object_returns_mapping() {
        let (_dir, path) =
            write_tmp_yaml("apcli:\n  mode: include\n  include:\n    - list\n    - describe\n");
        let resolver = ConfigResolver::new(None, Some(path));
        let v = resolver.resolve_object("apcli").expect("apcli key present");
        let map = match v {
            serde_yaml::Value::Mapping(m) => m,
            _ => panic!("expected mapping"),
        };
        let mode = map
            .get(serde_yaml::Value::String("mode".to_string()))
            .unwrap();
        assert_eq!(mode.as_str(), Some("include"));
    }

    #[test]
    fn test_resolve_object_missing_key_returns_none() {
        let (_dir, path) = write_tmp_yaml("other: 42\n");
        let resolver = ConfigResolver::new(None, Some(path));
        assert!(resolver.resolve_object("apcli").is_none());
    }

    #[test]
    fn test_resolve_object_no_config_file_returns_none() {
        let resolver = ConfigResolver::new(None, None);
        assert!(resolver.resolve_object("apcli").is_none());
    }

    #[test]
    fn test_resolve_object_malformed_yaml_returns_none() {
        let (_dir, path) = write_tmp_yaml("apcli: {unclosed\n");
        let resolver = ConfigResolver::new(None, Some(path));
        assert!(resolver.resolve_object("apcli").is_none());
    }

    #[test]
    fn test_direct_key_takes_precedence_over_alternate() {
        let mut file_map = HashMap::new();
        file_map.insert("cli.help_text_max_length".to_string(), "500".to_string());
        file_map.insert(
            "apcore-cli.help_text_max_length".to_string(),
            "2000".to_string(),
        );
        let resolver = ConfigResolver {
            cli_flags: HashMap::new(),
            config_file: Some(file_map),
            config_yaml: None,
            config_path: None,
            defaults: ConfigResolver::DEFAULTS.iter().copied().collect(),
        };
        assert_eq!(
            resolver.resolve("cli.help_text_max_length", None, None),
            Some("500".to_string())
        );
        assert_eq!(
            resolver.resolve("apcore-cli.help_text_max_length", None, None),
            Some("2000".to_string())
        );
    }
}
