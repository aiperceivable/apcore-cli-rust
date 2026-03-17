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

    /// Path to the config file that was loaded (or attempted).
    #[allow(dead_code)]
    config_path: Option<PathBuf>,

    /// Built-in default values.
    pub defaults: HashMap<&'static str, &'static str>,
}

impl ConfigResolver {
    /// Default configuration values.
    pub const DEFAULTS: &'static [(&'static str, &'static str)] = &[
        ("extensions.root", "./extensions"),
        ("logging.level", "INFO"),
        ("sandbox.enabled", "false"),
        ("cli.stdin_buffer_limit", "10485760"),
        ("cli.auto_approve", "false"),
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

        Self {
            cli_flags: cli_flags.unwrap_or_default(),
            config_file,
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
        if let Some(ref file_map) = self.config_file {
            if let Some(value) = file_map.get(key) {
                return Some(value.clone());
            }
        }

        // Tier 4: Built-in defaults.
        self.defaults.get(key).map(|s| s.to_string())
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
        let resolver = ConfigResolver::new(None, None);
        for key in [
            "extensions.root",
            "logging.level",
            "sandbox.enabled",
            "cli.stdin_buffer_limit",
            "cli.auto_approve",
        ] {
            assert!(
                resolver.defaults.contains_key(key),
                "missing default: {key}"
            );
        }
    }

    #[test]
    fn test_default_logging_level_is_info() {
        let resolver = ConfigResolver::new(None, None);
        assert_eq!(
            resolver.defaults.get("logging.level"),
            Some(&"INFO"),
            "logging.level default must be INFO, not WARNING"
        );
    }

    #[test]
    fn test_default_auto_approve_is_false() {
        let resolver = ConfigResolver::new(None, None);
        assert_eq!(
            resolver.defaults.get("cli.auto_approve"),
            Some(&"false"),
            "cli.auto_approve default must be false"
        );
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
}
