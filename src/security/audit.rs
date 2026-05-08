// apcore-cli — Audit logger.
// Protocol spec: SEC-01 (AuditLogger)

use std::io::{BufWriter, Write};
use std::path::PathBuf;

use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Serialize `v` to a compact, deterministically sorted JSON string.
///
/// Spec: each language serializes to sorted-key JSON before hashing so that
/// equivalent input dicts produce the same hash regardless of insertion order.
///
/// Recursion is required at every level — both inside object values and inside
/// array elements — so that nested objects with reordered keys hash to the
/// same digest as their canonical form (audit D11-002, 2026-05-08).
fn sorted_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            // Recurse into each value, then re-emit with keys sorted lexicographically.
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let pairs: Vec<String> = entries
                .iter()
                .map(|(k, val)| format!("{}:{}", serde_json::json!(k), sorted_json(val)))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
        Value::Array(arr) => {
            // Recurse into each element so nested objects inside arrays are
            // canonicalised. Element order itself is preserved (arrays are
            // ordered by spec).
            let parts: Vec<String> = arr.iter().map(sorted_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// AuditLogger
// ---------------------------------------------------------------------------

/// Append-only audit logger that records each module execution to a JSONL file.
///
/// When constructed with `path = None`, logging is a no-op (disabled).
#[derive(Debug, Clone)]
pub struct AuditLogger {
    path: Option<PathBuf>,
}

impl AuditLogger {
    /// Return the default path: `~/.apcore-cli/audit.jsonl`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".apcore-cli").join("audit.jsonl"))
    }

    /// Create a new `AuditLogger`.
    ///
    /// # Arguments
    /// * `path` — path to the JSONL audit log file; `None` uses the default
    ///   path `~/.apcore-cli/audit.jsonl`.
    pub fn new(path: Option<PathBuf>) -> Self {
        let resolved = path.or_else(Self::default_path);
        if let Some(ref p) = resolved {
            if let Some(parent) = p.parent() {
                // Best-effort; failure is silent.
                let _ = std::fs::create_dir_all(parent);
                // Restrict the parent dir to owner-only on Unix so audit-log
                // entries are not enumerable by other local UIDs on shared
                // systems.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
                }
            }
        }
        Self { path: resolved }
    }

    /// Resolve the username for an audit log entry.
    ///
    /// Spec (SEC-01): canonical resolution chain is
    ///   `getlogin → getpwuid(geteuid) → USER → LOGNAME → USERNAME → "unknown"`.
    ///
    /// `getlogin()` returns the controlling-terminal owner; `getpwuid` looks up
    /// the effective UID in the password database. Either of those wins over
    /// env vars, which can be spoofed or stale in container/su scenarios.
    fn get_user() -> String {
        // 1. getlogin() — fastest path; queries the controlling terminal.
        #[cfg(unix)]
        {
            // SAFETY: getlogin() returns either NULL or a pointer to a
            // statically allocated, NUL-terminated string owned by libc. We
            // copy it into an owned String before returning so we never hold
            // the libc-owned pointer past this block.
            unsafe {
                let raw = libc::getlogin();
                if !raw.is_null() {
                    let cstr = std::ffi::CStr::from_ptr(raw);
                    let name = cstr.to_string_lossy().into_owned();
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
            // 2. pwd lookup by effective UID.
            let euid = nix::unistd::geteuid();
            if let Ok(Some(user)) = nix::unistd::User::from_uid(euid) {
                if !user.name.is_empty() {
                    return user.name;
                }
            }
        }
        // 3. Fall back to env vars: USER → LOGNAME → USERNAME → "unknown".
        Self::resolve_user_from_env(&|k| std::env::var(k).ok())
    }

    /// Pure env-fallback helper, separated for unit testing the priority chain.
    ///
    /// Walks USER → LOGNAME → USERNAME → "unknown". Returns the first non-empty
    /// value the lookup function yields.
    fn resolve_user_from_env<F>(env_lookup: &F) -> String
    where
        F: Fn(&str) -> Option<String>,
    {
        for key in ["USER", "LOGNAME", "USERNAME"] {
            if let Some(v) = env_lookup(key) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
        "unknown".to_string()
    }

    /// Hash `input_data` with a fresh 16-byte random salt.
    ///
    /// Digest = SHA-256(salt(16) || sorted_json(`input_data`)).
    /// Returns hex-encoded SHA-256 hash (64 chars).
    ///
    /// Spec (SEC-03): hash = sha256(salt + json.dumps(input, sort_keys=True)).
    /// A fresh per-call salt prevents cross-invocation input correlation.
    fn hash_input(input_data: &Value) -> String {
        use aes_gcm::aead::rand_core::RngCore;
        use aes_gcm::aead::OsRng;

        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);

        let payload = sorted_json(input_data);
        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update(payload.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Log a single module execution event.
    ///
    /// Appends one JSON line to the audit log. IO failures emit a
    /// `tracing::warn!` and are otherwise ignored — this method never panics
    /// or propagates an error.
    ///
    /// # Fields written
    /// * `timestamp`   — ISO 8601 UTC timestamp
    /// * `user`        — username from `USER`/`LOGNAME`
    /// * `module_id`   — the executed module's identifier
    /// * `input_salt`  — 16-byte hex salt fed into the hash (persists so a
    ///   verifier can reproduce the digest from a known input)
    /// * `input_hash`  — salted SHA-256 of the JSON-serialised input
    /// * `status`      — `"success"` or `"error"`
    /// * `exit_code`   — process exit code
    /// * `duration_ms` — wall-clock execution time in milliseconds
    pub fn log_execution(
        &self,
        module_id: &str,
        input_data: &Value,
        status: &str,
        exit_code: i32,
        duration_ms: u64,
    ) {
        let Some(ref path) = self.path else {
            return; // logging disabled
        };

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let input_hash = Self::hash_input(input_data);
        let entry = json!({
            "timestamp":   timestamp,
            "user":        Self::get_user(),
            "module_id":   module_id,
            "input_hash":  input_hash,
            "status":      status,
            "exit_code":   exit_code,
            "duration_ms": duration_ms,
        });

        let result = (|| -> std::io::Result<()> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            // Restrict to owner read/write on Unix so audit entries are not
            // readable by other local UIDs on shared systems. set_permissions
            // is idempotent across appends; a no-op on subsequent writes.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, &entry).map_err(std::io::Error::other)?;
            writeln!(writer)?;
            writer.flush()?;
            Ok(())
        })();

        if let Err(e) = result {
            tracing::warn!("Could not write audit log: {e}");
        }
    }
}

/// Errors produced by the audit logger (reserved for future use).
#[derive(Debug, Error)]
pub enum AuditLogError {
    #[error("failed to write audit log: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to serialise audit record: {0}")]
    Serialise(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_audit_logger_disabled_no_op() {
        // AuditLogger with path=None must not write any files.
        let logger = AuditLogger { path: None };
        // Should not panic even with no path.
        logger.log_execution("mod.test", &json!({}), "success", 0, 1);
    }

    #[test]
    fn test_audit_logger_writes_jsonl_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone()));
        logger.log_execution("math.add", &json!({"a": 1}), "success", 0, 42);
        let content = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["module_id"], "math.add");
        assert_eq!(entry["status"], "success");
        assert_eq!(entry["exit_code"], 0);
        assert_eq!(entry["duration_ms"], 42);
    }

    #[test]
    fn test_audit_logger_appends_multiple_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone()));
        logger.log_execution("a.b", &json!({}), "success", 0, 1);
        logger.log_execution("c.d", &json!({}), "error", 1, 2);
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_audit_logger_record_contains_required_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone()));
        logger.log_execution("x.y", &json!({"k": "v"}), "success", 0, 10);
        let raw = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
        assert!(entry["timestamp"].as_str().unwrap().ends_with('Z'));
        assert!(entry["user"].is_string());
        assert_eq!(entry["module_id"], "x.y");
        assert!(entry["input_hash"].as_str().unwrap().len() == 64); // hex SHA-256
                                                                    // input_salt is NOT persisted per spec (A-D-007 fix)
        assert!(entry.get("input_salt").is_none());
        assert_eq!(entry["status"], "success");
        assert!(entry["exit_code"].is_number());
        assert!(entry["duration_ms"].is_number());
    }

    #[test]
    fn test_audit_logger_different_inputs_produce_different_hashes() {
        // Even without a persisted salt, different inputs must produce different
        // hashes in practice (different sorted JSON payload).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone()));
        logger.log_execution("x.y", &json!({"a": 1}), "success", 0, 0);
        logger.log_execution("x.y", &json!({"a": 2}), "success", 0, 0);
        let lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        let h0 = serde_json::from_str::<serde_json::Value>(&lines[0]).unwrap()["input_hash"]
            .as_str()
            .unwrap()
            .to_string();
        let h1 = serde_json::from_str::<serde_json::Value>(&lines[1]).unwrap()["input_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(h0, h1, "different inputs must produce different hashes");
    }

    #[test]
    fn test_audit_logger_same_input_different_hash_per_call() {
        // Each invocation uses a fresh random salt, so two calls with the same
        // input must produce different hash values.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone()));
        logger.log_execution("u.v", &json!({}), "success", 0, 0);
        logger.log_execution("u.v", &json!({}), "success", 0, 0);
        let lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        let h0 = serde_json::from_str::<serde_json::Value>(&lines[0]).unwrap()["input_hash"]
            .as_str()
            .unwrap()
            .to_string();
        let h1 = serde_json::from_str::<serde_json::Value>(&lines[1]).unwrap()["input_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(
            h0, h1,
            "same input across calls must produce different hashes (random salt)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_audit_logger_file_mode_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone()));
        logger.log_execution("perm.test", &json!({}), "success", 0, 0);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "audit log must be 0600; got {:o}", mode);
    }

    #[cfg(unix)]
    #[test]
    fn test_audit_logger_parent_dir_mode_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested-audit-dir");
        let path = nested.join("audit.jsonl");
        let _logger = AuditLogger::new(Some(path));
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "parent dir must be 0700; got {:o}", mode);
    }

    /// D11-002 (2026-05-08): `sorted_json` must canonicalise objects nested
    /// inside object values and inside arrays. Previously the function only
    /// sorted top-level keys, so two semantically equal payloads with
    /// different nested key orderings serialised to different strings and
    /// therefore hashed differently.
    #[test]
    fn test_sorted_json_recurses_into_nested_objects() {
        let a = json!({ "outer": { "y": 1, "x": 2 } });
        let b = json!({ "outer": { "x": 2, "y": 1 } });
        assert_eq!(
            super::sorted_json(&a),
            super::sorted_json(&b),
            "nested objects with reordered keys must canonicalise identically"
        );
    }

    #[test]
    fn test_sorted_json_recurses_into_arrays_of_objects() {
        let a = json!({ "items": [ { "y": 1, "x": 2 }, { "b": 4, "a": 3 } ] });
        let b = json!({ "items": [ { "x": 2, "y": 1 }, { "a": 3, "b": 4 } ] });
        assert_eq!(
            super::sorted_json(&a),
            super::sorted_json(&b),
            "objects nested inside arrays must canonicalise identically"
        );
    }

    #[test]
    fn test_sorted_json_preserves_array_element_order() {
        // Element order is data, not formatting — must NOT be sorted.
        let a = json!([3, 1, 2]);
        let b = json!([1, 2, 3]);
        assert_ne!(
            super::sorted_json(&a),
            super::sorted_json(&b),
            "array element order must be preserved (it is part of the value)"
        );
    }

    /// D10-007: env-fallback helper must walk USER -> LOGNAME -> USERNAME -> "unknown".
    #[test]
    fn test_resolve_user_from_env_priority_chain() {
        // USER takes precedence over LOGNAME/USERNAME.
        let env = |k: &str| -> Option<String> {
            match k {
                "USER" => Some("user_val".to_string()),
                "LOGNAME" => Some("logname_val".to_string()),
                "USERNAME" => Some("username_val".to_string()),
                _ => None,
            }
        };
        assert_eq!(AuditLogger::resolve_user_from_env(&env), "user_val");

        // LOGNAME wins when USER is unset.
        let env = |k: &str| -> Option<String> {
            match k {
                "LOGNAME" => Some("logname_val".to_string()),
                "USERNAME" => Some("username_val".to_string()),
                _ => None,
            }
        };
        assert_eq!(AuditLogger::resolve_user_from_env(&env), "logname_val");

        // USERNAME wins when USER and LOGNAME are unset (Windows-style).
        let env = |k: &str| -> Option<String> {
            match k {
                "USERNAME" => Some("username_val".to_string()),
                _ => None,
            }
        };
        assert_eq!(AuditLogger::resolve_user_from_env(&env), "username_val");

        // All unset → "unknown".
        let env = |_: &str| -> Option<String> { None };
        assert_eq!(AuditLogger::resolve_user_from_env(&env), "unknown");
    }

    /// D10-007: get_user prefers system identity (getlogin/getpwuid) over env vars.
    /// On Unix dev hosts, getpwuid(geteuid()) always succeeds, so even if USER and
    /// LOGNAME are set to sentinel values, get_user must NOT return those sentinels.
    #[cfg(unix)]
    #[test]
    fn test_get_user_prefers_system_identity_over_env() {
        // SAFETY: This test sets process-wide env vars. Other tests in the same
        // binary that read USER/LOGNAME could observe these values; we restore
        // them at the end. The critical assertion (system identity beats env) is
        // independent of pre-existing values.
        let prev_user = std::env::var("USER").ok();
        let prev_logname = std::env::var("LOGNAME").ok();
        std::env::set_var("USER", "sentinel_user_d10_007");
        std::env::set_var("LOGNAME", "sentinel_logname_d10_007");

        let resolved = AuditLogger::get_user();

        // Restore env first so a panic in the assertions below leaves a clean state.
        match prev_user {
            Some(v) => std::env::set_var("USER", v),
            None => std::env::remove_var("USER"),
        }
        match prev_logname {
            Some(v) => std::env::set_var("LOGNAME", v),
            None => std::env::remove_var("LOGNAME"),
        }

        assert_ne!(
            resolved, "sentinel_user_d10_007",
            "get_user must consult getlogin/getpwuid before USER env var"
        );
        assert_ne!(
            resolved, "sentinel_logname_d10_007",
            "get_user must consult getlogin/getpwuid before LOGNAME env var"
        );
        assert!(!resolved.is_empty(), "get_user must never return empty");
    }
}
