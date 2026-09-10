//! Domain model: task state, config, and their JSON shapes.
//!
//! A task's current state is a projection over its record's events (see
//! [`crate::record`]). This module owns the projected shape and the value
//! types; it knows nothing about files or events.
//!
//! The reader stays lenient where leniency cannot hide a mistake: unknown
//! fields are ignored and explicit `null` reads as an empty list, so a record
//! written by a migrated or newer writer still loads.

use std::collections::BTreeMap;
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
    /// Lenient parse: case-insensitive, plus the retired `cancelled` names
    /// (migration and hand-written stores still contain them).
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
// Logs
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

/// `Vec` that also accepts explicit `null`, which the Go writer emitted for
/// empty slices and older writers still produce.
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
pub fn parse_legacy_log(value: &str) -> LogEntry {
    if is_timestamp(value) {
        return LogEntry {
            ts: value.to_owned(),
            msg: String::new(),
        };
    }
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
// Task state
// ---------------------------------------------------------------------------

/// The current state of one task: everything that is true about it now.
///
/// Written verbatim as the `created` event's payload, so a record is
/// self-describing from its first line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    /// ULID. Immutable.
    pub id: String,
    /// Short handle. Immutable; assigned once at creation.
    pub alias: String,
    /// Aliases from a migrated store, kept resolvable during cutover.
    #[serde(default, deserialize_with = "null_vec")]
    pub legacy_aliases: Vec<String>,
    /// Display grouping. Mutable; never part of identity.
    pub project: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: Status,
    pub priority: Priority,
    #[serde(default, deserialize_with = "null_vec")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub blocked_by: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_logs")]
    pub logs: Vec<LogEntry>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    /// Replaceable summary of where the work stands: result, blocker, next
    /// action, and verification references. Not a second status store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// References to research, decisions, or source locations relevant here.
    #[serde(default, deserialize_with = "null_vec")]
    pub links: Vec<String>,
    /// What must be true for this task to count as done.
    #[serde(default, deserialize_with = "null_vec")]
    pub acceptance: Vec<String>,
    /// How completion was verified (commands, paths, commit SHAs).
    #[serde(default, deserialize_with = "null_vec")]
    pub evidence: Vec<String>,
}

impl TaskState {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn is_done(&self) -> bool {
        self.status.is_terminal()
    }
}

/// Task state plus computed view fields (what `--json` emits).
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: TaskState,
    /// Revision token for the record as read. Pass it back as `--if-rev` to
    /// reject a write prepared against a stale read.
    pub rev: String,
    /// A blocker is incomplete, or refers to a task that no longer resolves.
    /// Unresolved blockers count as blocking, not as completed.
    pub blocked_by_incomplete: bool,
    /// Blockers that do not resolve to a record in the store.
    #[serde(default)]
    pub unresolved_blockers: Vec<String>,
    /// `blocked_by` rendered the way a person would type it: the blocker's
    /// alias when it resolves, otherwise a shortened ID.
    #[serde(default)]
    pub blocker_refs: Vec<String>,
    /// `parent` rendered as an alias, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<String>,
}

impl TaskView {
    pub fn id(&self) -> &str {
        &self.task.id
    }
}

// ---------------------------------------------------------------------------
// Config (`store.json`)
// ---------------------------------------------------------------------------

/// On-disk layout version. v1 requires exactly this.
pub const FORMAT: i64 = 2;

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

/// `.tasks/store.json`. Carries the format gate that keeps a v1 binary from
/// interpreting a v0 store (`config.json` + one JSON file per task).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// On-disk layout version; [`FORMAT`] for this binary.
    pub format: i64,
    #[serde(default = "default_version")]
    pub version: i64,
    #[serde(default = "default_project")]
    pub project: String,
    #[serde(default)]
    pub defaults: ConfigDefaults,
    #[serde(default = "default_clean_after")]
    pub clean_after: CleanAfter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<BTreeMap<String, String>>,
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
            format: FORMAT,
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

impl miette::Diagnostic for ModelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trip_and_legacy() {
        assert_eq!(Status::parse("OPEN").unwrap(), Status::Open);
        assert_eq!(Status::parse("cancelled").unwrap(), Status::Closed);
        assert!(Status::parse("bogus").is_err());
        assert!(Status::Done.is_terminal());
        assert!(!Status::Active.is_terminal());
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
    }

    #[test]
    fn clean_after_shapes() {
        let c: CleanAfter = serde_json::from_str("false").unwrap();
        assert!(!c.enabled);
        let c: CleanAfter = serde_json::from_str("14").unwrap();
        assert!(c.enabled && c.days == 14);
        assert_eq!(serde_json::to_string(&c).unwrap(), "14");
    }

    fn created_json() -> &'static str {
        r#"{"id":"01j8x0m5r7000000000000000a","alias":"a7b3","project":"tk",
            "title":"t","status":"open","priority":3,"created_at":"x","updated_at":"y"}"#
    }

    #[test]
    fn explicit_nulls_and_missing_fields_read_as_empty() {
        let s: TaskState = serde_json::from_str(
            r#"{"id":"01j8x0m5r7000000000000000a","alias":"a7b3","project":"tk",
                "title":"t","status":"open","priority":3,"labels":null,
                "blocked_by":null,"logs":null,"links":null,
                "created_at":"x","updated_at":"y"}"#,
        )
        .unwrap();
        assert!(s.labels.is_empty());
        assert!(s.blocked_by.is_empty());
        assert!(s.logs.is_empty());
        assert!(s.links.is_empty());
        assert!(s.legacy_aliases.is_empty());
        assert!(!s.is_archived());
    }

    #[test]
    fn round_trip_keeps_the_list_fields() {
        let s: TaskState = serde_json::from_str(created_json()).unwrap();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""links":[]"#), "{json}");
        assert!(json.contains(r#""blocked_by":[]"#), "{json}");
        assert!(!json.contains("checkpoint"), "{json}");
        let back: TaskState = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn a_field_from_a_future_writer_is_ignored_not_fatal() {
        // Forward compatibility is what makes reserved fields unnecessary: a
        // newer writer's field or event is additive, and this reader ignores
        // it rather than failing.
        let raw = created_json().replace("}", r#","assignee":"nick","future_field":42}"#);
        let s: TaskState = serde_json::from_str(&raw).unwrap();
        assert_eq!(s.alias, "a7b3");
    }

    #[test]
    fn config_requires_a_format_and_defaults_to_the_current_one() {
        let c: Config = serde_json::from_str(r#"{"format":2,"project":"demo"}"#).unwrap();
        assert_eq!(c.format, FORMAT);
        assert_eq!(c.project, "demo");
        assert!(c.clean_after.enabled);
        // A v0 config has no `format`: reading it as a v1 config must fail.
        assert!(serde_json::from_str::<Config>(r#"{"version":1,"project":"demo"}"#).is_err());
    }
}
