//! `system.usage` aggregator — reads `~/.apcore-cli/audit.jsonl` and groups
//! by `module_id`.
//!
//! Implements the `system.usage.summary` data pipeline described in
//! aiperceivable/apcore-cli#17:
//!
//! ```text
//! Read audit.jsonl
//!   -> filter by time period (timestamp >= now - period; default 24h)
//!   -> group by module_id
//!   -> aggregate: calls, errors, avg latency_ms
//! ```
//!
//! Module-protocol registration of `system.usage.summary` and
//! `system.usage.module` as registry-callable built-ins is tracked as a
//! follow-up; today the readers are invoked directly by the discovery layer.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePeriod {
    Hour1,
    Hours24,
    Days7,
    Days30,
}

impl UsagePeriod {
    fn delta(self) -> Duration {
        match self {
            UsagePeriod::Hour1 => Duration::hours(1),
            UsagePeriod::Hours24 => Duration::hours(24),
            UsagePeriod::Days7 => Duration::days(7),
            UsagePeriod::Days30 => Duration::days(30),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "1h" => UsagePeriod::Hour1,
            "7d" => UsagePeriod::Days7,
            "30d" => UsagePeriod::Days30,
            _ => UsagePeriod::Hours24,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageSummary {
    pub module_id: String,
    pub calls: u64,
    pub errors: u64,
    pub latency_ms: f64,
}

/// The audit log this aggregator reads when no explicit path is given.
///
/// Delegates to [`crate::security::AuditLogger::default_path`] rather than
/// re-deriving `~/.apcore-cli/audit.jsonl`, so the reader and the writer
/// cannot disagree about which file is the audit log. In production both
/// resolve the same home path, exactly as before; under `test-support` both
/// follow the same redirect, so a test reading a summary sees what the tests
/// wrote rather than whatever the developer's real log happens to hold.
pub(crate) fn default_audit_path() -> Option<PathBuf> {
    crate::security::AuditLogger::default_path()
}

/// Aggregate the audit log per-module over the given period.
///
/// Returns an empty map when the log is missing or empty — callers are
/// expected to fall back to id-sort with a visible message in that case.
pub fn compute_summary(
    audit_path: Option<&Path>,
    period: UsagePeriod,
    now: DateTime<Utc>,
) -> HashMap<String, UsageSummary> {
    let path: PathBuf = match audit_path {
        Some(p) => p.to_path_buf(),
        None => match default_audit_path() {
            Some(p) => p,
            None => return HashMap::new(),
        },
    };
    if !path.exists() {
        return HashMap::new();
    }
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };

    let cutoff = now - period.delta();

    let mut counts: HashMap<String, u64> = HashMap::new();
    let mut errors: HashMap<String, u64> = HashMap::new();
    let mut latency_sum: HashMap<String, u64> = HashMap::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Tolerate a partial last line from a crashed write rather than
        // failing the sort (audit log is append-only JSONL).
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ts_str = match entry.get("timestamp").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let ts = match DateTime::parse_from_rfc3339(ts_str) {
            Ok(t) => t.with_timezone(&Utc),
            Err(_) => continue,
        };
        if ts < cutoff {
            continue;
        }
        let module_id = match entry.get("module_id").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        *counts.entry(module_id.clone()).or_insert(0) += 1;
        if entry.get("status").and_then(|v| v.as_str()) == Some("error") {
            *errors.entry(module_id.clone()).or_insert(0) += 1;
        }
        let duration = entry
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        *latency_sum.entry(module_id).or_insert(0) += duration;
    }

    let mut out = HashMap::new();
    for (id, calls) in counts {
        let err_count = errors.get(&id).copied().unwrap_or(0);
        let lat_total = latency_sum.get(&id).copied().unwrap_or(0);
        let avg = if calls > 0 {
            lat_total as f64 / calls as f64
        } else {
            0.0
        };
        out.insert(
            id.clone(),
            UsageSummary {
                module_id: id,
                calls,
                errors: err_count,
                latency_ms: avg,
            },
        );
    }
    out
}

/// Sort `modules` (a slice of serde_json::Value with a `"module_id"` field)
/// by `field` (`"calls"`, `"errors"`, or `"latency"`) using audit-log data.
///
/// Returns whether real usage data was used. When false, modules are sorted
/// by id and callers should surface a visible message that explains the
/// fallback (issue #17 AC).
pub fn sort_modules_by_usage(modules: &mut [Value], field: &str, reverse: bool) -> bool {
    let summary = compute_summary(None, UsagePeriod::Hours24, Utc::now());
    if summary.is_empty() {
        modules.sort_by(|a, b| {
            let aid = a.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
            let bid = b.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
            aid.cmp(bid)
        });
        if reverse {
            modules.reverse();
        }
        return false;
    }

    let key = |m: &Value| -> f64 {
        let id = m.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
        match summary.get(id) {
            Some(s) => match field {
                "calls" => s.calls as f64,
                "errors" => s.errors as f64,
                "latency" => s.latency_ms,
                _ => 0.0,
            },
            None => 0.0,
        }
    };

    modules.sort_by(|a, b| {
        let ka = key(a);
        let kb = key(b);
        let primary = ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal);
        let primary = if reverse { primary.reverse() } else { primary };
        if primary != std::cmp::Ordering::Equal {
            return primary;
        }
        // Stable secondary sort by id for deterministic output.
        let aid = a.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
        let bid = b.get("module_id").and_then(|v| v.as_str()).unwrap_or("");
        aid.cmp(bid)
    });
    true
}
