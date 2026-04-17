//! Module Exposure Filtering (FE-12).
//!
//! Provides declarative control over which discovered modules are exposed
//! as CLI commands. Supports three modes: all, include (whitelist), and
//! exclude (blacklist) with glob-pattern matching on module IDs.

use regex::Regex;

/// Compile a glob pattern into a [`Regex`].
///
/// - `*` matches a single dotted segment (no dots): `[^.]*`
/// - `**` matches across segments (any characters including dots): `.+`
/// - Literal text is matched exactly via regex escaping.
fn compile_pattern(pattern: &str) -> Regex {
    let sentinel = "\x00GLOB\x00";
    let escaped = pattern.replace("**", sentinel);
    let parts: Vec<&str> = escaped.split('*').collect();
    let regex_parts: Vec<String> = parts
        .iter()
        .map(|p| {
            let restored = p.replace(sentinel, "**");
            regex::escape(&restored)
        })
        .collect();
    let mut regex_str = regex_parts.join("[^.]*");
    regex_str = regex_str.replace(r"\*\*", ".+");
    Regex::new(&format!("^{regex_str}$")).expect("invalid exposure pattern regex")
}

/// Test whether a module_id matches a glob pattern.
pub fn glob_match(module_id: &str, pattern: &str) -> bool {
    compile_pattern(pattern).is_match(module_id)
}

/// Determines which modules are exposed as CLI commands.
///
/// Filtering modes:
/// - `all`: every discovered module becomes a CLI command (default).
/// - `include`: only modules matching at least one include pattern are exposed.
/// - `exclude`: all modules exposed except those matching any exclude pattern.
pub struct ExposureFilter {
    /// Filter mode: "all" | "include" | "exclude".
    pub mode: String,
    compiled_include: Vec<Regex>,
    compiled_exclude: Vec<Regex>,
}

impl Default for ExposureFilter {
    fn default() -> Self {
        Self {
            mode: "all".to_string(),
            compiled_include: Vec::new(),
            compiled_exclude: Vec::new(),
        }
    }
}

impl ExposureFilter {
    /// Create a new exposure filter.
    pub fn new(mode: &str, include: &[String], exclude: &[String]) -> Self {
        let dedup = |patterns: &[String]| -> Vec<Regex> {
            let mut seen = std::collections::HashSet::new();
            patterns
                .iter()
                .filter(|p| seen.insert((*p).clone()))
                .map(|p| compile_pattern(p))
                .collect()
        };
        Self {
            mode: mode.to_string(),
            compiled_include: dedup(include),
            compiled_exclude: dedup(exclude),
        }
    }

    /// Return true if the module should be exposed as a CLI command.
    pub fn is_exposed(&self, module_id: &str) -> bool {
        match self.mode.as_str() {
            "all" => true,
            "include" => self
                .compiled_include
                .iter()
                .any(|rx| rx.is_match(module_id)),
            "exclude" => !self
                .compiled_exclude
                .iter()
                .any(|rx| rx.is_match(module_id)),
            _ => true,
        }
    }

    /// Partition module_ids into (exposed, hidden) lists.
    pub fn filter_modules(&self, module_ids: &[String]) -> (Vec<String>, Vec<String>) {
        let mut exposed = Vec::new();
        let mut hidden = Vec::new();
        for mid in module_ids {
            if self.is_exposed(mid) {
                exposed.push(mid.clone());
            } else {
                hidden.push(mid.clone());
            }
        }
        (exposed, hidden)
    }

    /// Create from a serde_json::Value config.
    ///
    /// Expected structure:
    /// ```json
    /// { "expose": { "mode": "include", "include": ["admin.*"] } }
    /// ```
    pub fn from_config(config: &serde_json::Value) -> Result<Self, String> {
        let expose = config.get("expose").unwrap_or(&serde_json::Value::Null);
        if !expose.is_object() {
            if !expose.is_null() {
                tracing::warn!("Invalid 'expose' config (expected object), using mode: all.");
            }
            return Ok(Self::default());
        }

        let mode = expose.get("mode").and_then(|v| v.as_str()).unwrap_or("all");
        if !["all", "include", "exclude"].contains(&mode) {
            return Err(format!(
                "Invalid expose mode: '{}'. Must be one of: all, include, exclude.",
                mode
            ));
        }

        let parse_list = |key: &str| -> Vec<String> {
            match expose.get(key) {
                Some(serde_json::Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|v| {
                        let s = v.as_str().unwrap_or("");
                        if s.is_empty() {
                            tracing::warn!("Empty pattern in expose.{}, skipping.", key);
                            None
                        } else {
                            Some(s.to_string())
                        }
                    })
                    .collect(),
                Some(_) => {
                    tracing::warn!("Invalid 'expose.{}' (expected array), ignoring.", key);
                    Vec::new()
                }
                None => Vec::new(),
            }
        };

        let include = parse_list("include");
        let exclude = parse_list("exclude");
        Ok(Self::new(mode, &include, &exclude))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- glob_match tests ---

    #[test]
    fn test_exact_match() {
        assert!(glob_match("system.health", "system.health"));
    }

    #[test]
    fn test_exact_no_partial() {
        assert!(!glob_match("system.health.check", "system.health"));
    }

    #[test]
    fn test_single_star_matches_one_segment() {
        assert!(glob_match("admin.users", "admin.*"));
    }

    #[test]
    fn test_single_star_not_across_dots() {
        assert!(!glob_match("admin.users.list", "admin.*"));
    }

    #[test]
    fn test_single_star_not_prefix_only() {
        assert!(!glob_match("admin", "admin.*"));
    }

    #[test]
    fn test_star_prefix() {
        assert!(glob_match("product.get", "*.get"));
        assert!(!glob_match("product.get.all", "*.get"));
    }

    #[test]
    fn test_double_star_across_segments() {
        assert!(glob_match("admin.users", "admin.**"));
        assert!(glob_match("admin.users.list", "admin.**"));
    }

    #[test]
    fn test_double_star_not_bare_prefix() {
        assert!(!glob_match("admin", "admin.**"));
    }

    #[test]
    fn test_bare_star() {
        assert!(glob_match("standalone", "*"));
        assert!(!glob_match("a.b", "*"));
    }

    #[test]
    fn test_bare_double_star() {
        assert!(glob_match("anything", "**"));
        assert!(glob_match("a.b.c.d", "**"));
    }

    #[test]
    fn test_literal_no_glob() {
        assert!(glob_match("admin.users", "admin.users"));
        assert!(!glob_match("admin.config", "admin.users"));
    }

    // --- ExposureFilter tests ---

    #[test]
    fn test_mode_all() {
        let f = ExposureFilter::default();
        assert!(f.is_exposed("anything"));
    }

    #[test]
    fn test_mode_include() {
        let f = ExposureFilter::new("include", &["admin.*".into(), "jobs.*".into()], &[]);
        assert!(f.is_exposed("admin.users"));
        assert!(!f.is_exposed("webhooks.stripe"));
    }

    #[test]
    fn test_mode_include_empty() {
        let f = ExposureFilter::new("include", &[], &[]);
        assert!(!f.is_exposed("anything"));
    }

    #[test]
    fn test_mode_exclude() {
        let f = ExposureFilter::new("exclude", &[], &["webhooks.*".into(), "internal.*".into()]);
        assert!(f.is_exposed("admin.users"));
        assert!(!f.is_exposed("webhooks.stripe"));
    }

    #[test]
    fn test_mode_exclude_empty() {
        let f = ExposureFilter::new("exclude", &[], &[]);
        assert!(f.is_exposed("anything"));
    }

    #[test]
    fn test_filter_modules() {
        let f = ExposureFilter::new("include", &["admin.*".into()], &[]);
        let (exposed, hidden) = f.filter_modules(&[
            "admin.users".into(),
            "admin.config".into(),
            "webhooks.stripe".into(),
        ]);
        assert_eq!(exposed, vec!["admin.users", "admin.config"]);
        assert_eq!(hidden, vec!["webhooks.stripe"]);
    }

    #[test]
    fn test_from_config_include() {
        let config: serde_json::Value = serde_json::json!({
            "expose": {
                "mode": "include",
                "include": ["admin.*"]
            }
        });
        let f = ExposureFilter::from_config(&config).unwrap();
        assert_eq!(f.mode.as_str(), "include");
        assert!(f.is_exposed("admin.users"));
        assert!(!f.is_exposed("webhooks.stripe"));
    }

    #[test]
    fn test_from_config_missing() {
        let config = serde_json::json!({});
        let f = ExposureFilter::from_config(&config).unwrap();
        assert_eq!(f.mode.as_str(), "all");
    }

    #[test]
    fn test_from_config_invalid_mode() {
        let config = serde_json::json!({
            "expose": { "mode": "whitelist" }
        });
        assert!(ExposureFilter::from_config(&config).is_err());
    }
}
