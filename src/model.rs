//! Domain model: tasks, config, and their JSON shapes.
//!
//! The reader is deliberately lenient (unknown fields ignored, legacy log
//! strings and `cancelled` statuses accepted) while the writer always emits
//! the clean schema. Single-user tool: one-way migration, no version gates.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Deferred,
    Open,
    Active,
    Done,
    Closed,
}

impl Status {
    /// Lenient parse: case-insensitive, plus the retired `cancelled` names.
    pub fn parse(s: &str) -> Result<Self, ModelError> {
        match s.trim().to_lowercase().as_str() {
            "deferred" => Ok(Self::Deferred),
            "open" => Ok(Self::Open),
            "active" => Ok(Self::Active),
            "done" => Ok(Self::Done),
            "closed" | "cancelled" | "canceled" => Ok(Self::Closed),
            other => Err(ModelError::BadStatus(other.to_owned())),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Closed)
    }
}

impl<'de> Deserialize<'de> for Status {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Status::parse(&s).map_err(de::Error::custom)
    }
}

impl FromStr for Status {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Status::parse(s)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Deferred => "deferred",
            Self::Open => "open",
            Self::Active => "active",
            Self::Done => "done",
            Self::Closed => "closed",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Priority {
    None = 0,
    Urgent = 1,
    High = 2,
    Medium = 3,
    Low = 4,
}

impl Priority {
    /// Accepts `0-4`, `p0-p4`, and `none/urgent/high/medium/low`.
    pub fn parse(s: &str) -> Result<Self, ModelError> {
        let t = s.trim().to_lowercase();
        if let Some(p) = Self::from_name(&t) {
            return Ok(p);
        }
        let digits = t.strip_prefix('p').unwrap_or(&t);
        match digits.parse::<u8>() {
            Ok(0) => Ok(Self::None),
            Ok(1) => Ok(Self::Urgent),
            Ok(2) => Ok(Self::High),
            Ok(3) => Ok(Self::Medium),
            Ok(4) => Ok(Self::Low),
            _ => Err(ModelError::BadPriority(s.to_owned())),
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "urgent" => Some(Self::Urgent),
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    pub fn from_u8(n: u8) -> Option<Self> {
        match n {
            0 => Some(Self::None),
            1 => Some(Self::Urgent),
            2 => Some(Self::High),
            3 => Some(Self::Medium),
            4 => Some(Self::Low),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// Short label like `p1`.
    pub fn short(self) -> String {
        format!("p{}", self as u8)
    }

    /// Sort key: 1-4 first, `none` last.
    pub fn sort_key(self) -> u8 {
        match self {
            Self::None => 5,
            p => p as u8,
        }
    }
}

impl Serialize for Priority {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u8::deserialize(d)?;
        Priority::from_u8(n).ok_or_else(|| de::Error::custom(format!("invalid priority {n}")))
    }
}

// ---------------------------------------------------------------------------
// Logs (lenient reader: structured, legacy strings, null)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub msg: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawLog {
    Structured(LogEntry),
    Legacy(String),
    Null,
}

/// `Vec` that also accepts explicit `null` (the Go writer marshals empty
/// slices as `null`, so old files are full of them).
fn null_vec<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

fn deserialize_logs<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<LogEntry>, D::Error> {
    let raw = Option::<Vec<RawLog>>::deserialize(d)?.unwrap_or_default();
    Ok(raw
        .into_iter()
        .map(|r| match r {
            RawLog::Structured(e) => e,
            RawLog::Legacy(s) => parse_legacy_log(&s),
            RawLog::Null => LogEntry::default(),
        })
        .collect())
}

/// Legacy entries were plain strings, optionally `"<timestamp>: <message>"`.
fn parse_legacy_log(value: &str) -> LogEntry {
    if is_timestamp(value) {
        return LogEntry {
            ts: value.to_owned(),
            msg: String::new(),
        };
    }
    // Find a colon that follows a complete timestamp (handles both date-only
    // and full RFC 3339 prefixes without tripping on the colons inside a
    // time component).
    for (i, c) in value.char_indices() {
        if c == ':' && is_timestamp(&value[..i]) {
            return LogEntry {
                ts: value[..i].to_owned(),
                msg: value[i + 1..].trim_start_matches([' ', '\t']).to_owned(),
            };
        }
    }
    LogEntry {
        ts: String::new(),
        msg: value.to_owned(),
    }
}

fn is_timestamp(value: &str) -> bool {
    use chrono::{DateTime, NaiveDate};
    if NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() {
        return true;
    }
    DateTime::parse_from_rfc3339(value).is_ok()
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub project: String,
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: Status,
    pub priority: Priority,
    #[serde(default, deserialize_with = "null_vec")]
    pub labels: Vec<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(default, deserialize_with = "deserialize_logs")]
    pub logs: Vec<LogEntry>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

impl Task {
    pub fn id(&self) -> String {
        format!("{}-{}", self.project, self.r#ref)
    }
}

/// Task plus computed view fields (what `--json` emits for lists/details).
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    pub id: String,
    pub blocked_by_incomplete: bool,
    pub is_overdue: bool,
    pub days_until_due: Option<i64>,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDefaults {
    #[serde(default = "default_priority")]
    pub priority: Priority,
    #[serde(default, deserialize_with = "null_vec")]
    pub labels: Vec<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub assignees: Vec<String>,
}

fn default_priority() -> Priority {
    Priority::Medium
}

impl Default for ConfigDefaults {
    fn default() -> Self {
        Self {
            priority: Priority::Medium,
            labels: Vec::new(),
            assignees: Vec::new(),
        }
    }
}

/// `clean_after` is either a day count or `false` (disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanAfter {
    pub enabled: bool,
    pub days: i64,
}

impl Serialize for CleanAfter {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.enabled {
            s.serialize_i64(self.days)
        } else {
            s.serialize_bool(false)
        }
    }
}

impl<'de> Deserialize<'de> for CleanAfter {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Days(i64),
        }
        match Raw::deserialize(d)? {
            Raw::Bool(false) => Ok(Self {
                enabled: false,
                days: 0,
            }),
            Raw::Bool(true) => Ok(Self {
                enabled: true,
                days: 14,
            }),
            Raw::Days(n) => Ok(Self {
                enabled: true,
                days: n,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: i64,
    #[serde(default = "default_project")]
    pub project: String,
    #[serde(default)]
    pub defaults: ConfigDefaults,
    #[serde(default = "default_clean_after")]
    pub clean_after: CleanAfter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<std::collections::BTreeMap<String, String>>,
}

fn default_version() -> i64 {
    1
}
fn default_project() -> String {
    "tk".to_owned()
}
fn default_clean_after() -> CleanAfter {
    CleanAfter {
        enabled: true,
        days: 14,
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            project: "tk".to_owned(),
            defaults: ConfigDefaults::default(),
            clean_after: CleanAfter {
                enabled: true,
                days: 14,
            },
            aliases: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid status {0:?}: use open, active, deferred, done, or closed")]
    BadStatus(String),
    #[error("invalid priority {0:?}: use 0-4, p0-p4, or none/urgent/high/medium/low")]
    BadPriority(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trip_and_legacy() {
        assert_eq!(Status::parse("OPEN").unwrap(), Status::Open);
        assert_eq!(Status::parse("cancelled").unwrap(), Status::Closed);
        assert_eq!(Status::parse("canceled").unwrap(), Status::Closed);
        assert!(Status::parse("bogus").is_err());
        assert!(Status::Done.is_terminal());
        assert!(!Status::Active.is_terminal());
        // JSON uses lowercase names.
        assert_eq!(
            serde_json::to_string(&Status::Active).unwrap(),
            r#""active""#
        );
        let s: Status = serde_json::from_str(r#""cancelled""#).unwrap();
        assert_eq!(s, Status::Closed);
    }

    #[test]
    fn priority_forms() {
        assert_eq!(Priority::parse("1").unwrap(), Priority::Urgent);
        assert_eq!(Priority::parse("p2").unwrap(), Priority::High);
        assert_eq!(Priority::parse("low").unwrap(), Priority::Low);
        assert_eq!(Priority::parse("P0").unwrap(), Priority::None);
        assert!(Priority::parse("9").is_err());
        assert_eq!(Priority::Urgent.short(), "p1");
        assert!(Priority::None.sort_key() > Priority::Low.sort_key());
    }

    #[test]
    fn legacy_log_strings() {
        let e = parse_legacy_log("2026-01-10: did a thing");
        assert_eq!(e.ts, "2026-01-10");
        assert_eq!(e.msg, "did a thing");
        let e = parse_legacy_log("just a note");
        assert_eq!(e.msg, "just a note");
        // Mixed array: structured + legacy + null.
        let logs: Vec<LogEntry> =
            serde_json::from_str(r#"[{"ts":"t","msg":"m"}, "2026-01-10: old", null]"#)
                .map(|v: Vec<RawLog>| {
                    v.into_iter()
                        .map(|r| match r {
                            RawLog::Structured(e) => e,
                            RawLog::Legacy(s) => parse_legacy_log(&s),
                            RawLog::Null => LogEntry::default(),
                        })
                        .collect()
                })
                .unwrap();
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[1].msg, "old");
    }

    #[test]
    fn clean_after_shapes() {
        let c: CleanAfter = serde_json::from_str("false").unwrap();
        assert!(!c.enabled);
        let c: CleanAfter = serde_json::from_str("14").unwrap();
        assert!(c.enabled && c.days == 14);
        assert_eq!(serde_json::to_string(&c).unwrap(), "14");
    }

    #[test]
    fn explicit_nulls_read_as_empty() {
        // The Go writer marshals empty slices as `null`.
        let t: Task = serde_json::from_str(
            r#"{"project":"tk","ref":"a7b3","title":"t","status":"open",
                "priority":3,"labels":null,"assignees":null,"blocked_by":null,
                "logs":null,"created_at":"x","updated_at":"y"}"#,
        )
        .unwrap();
        assert!(t.labels.is_empty());
        assert!(t.assignees.is_empty());
        assert!(t.blocked_by.is_empty());
        assert!(t.logs.is_empty());
    }

    #[test]
    fn unknown_fields_ignored() {
        // Forward-compat: extra keys from a newer writer don't break reads.
        let t: Task = serde_json::from_str(
            r#"{"project":"tk","ref":"a7b3","title":"t","status":"open",
                "priority":3,"created_at":"x","updated_at":"y","future_field":42}"#,
        )
        .unwrap();
        assert_eq!(t.id(), "tk-a7b3");
    }
}
