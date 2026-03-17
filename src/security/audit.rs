// apcore-cli — Audit logger.
// Protocol spec: SEC-01 (AuditLogger)

use std::io::{BufWriter, Write};
use std::path::PathBuf;

use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const HASH_SALT: &str = "apcore-cli-audit-v1";

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
            }
        }
        Self { path: resolved }
    }

    /// Return the username from the environment: `USER` -> `LOGNAME` -> `"unknown"`.
    fn _get_user() -> String {
        std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "unknown".to_string())
    }

    /// Hash `input_data` with a fresh 16-byte random salt.
    ///
    /// Digest = SHA-256(`HASH_SALT` `:` stable_json(`input_data`)).
    /// Returns a lowercase hex string (64 chars).
    fn _hash_input(input_data: &Value) -> String {
        use aes_gcm::aead::rand_core::RngCore;
        use aes_gcm::aead::OsRng;

        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);

        let payload = Self::_stable_json(input_data);
        let salted = format!("{}:{}", HASH_SALT, payload);

        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update(salted.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Produce a stable (sorted-key) JSON string for `v`.
    fn _stable_json(v: &Value) -> String {
        match v {
            Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> = map.iter().collect();
                let pairs: Vec<String> = sorted
                    .iter()
                    .map(|(k, val)| {
                        format!(
                            "{}:{}",
                            serde_json::json!(k),
                            Self::_stable_json(val)
                        )
                    })
                    .collect();
                format!("{{{}}}", pairs.join(","))
            }
            other => other.to_string(),
        }
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
        let entry = json!({
            "timestamp":   timestamp,
            "user":        Self::_get_user(),
            "module_id":   module_id,
            "input_hash":  Self::_hash_input(input_data),
            "status":      status,
            "exit_code":   exit_code,
            "duration_ms": duration_ms,
        });

        let result = (|| -> std::io::Result<()> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, &entry)
                .map_err(std::io::Error::other)?;
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
        assert_eq!(entry["status"], "success");
        assert!(entry["exit_code"].is_number());
        assert!(entry["duration_ms"].is_number());
    }
}
