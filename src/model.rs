//! The record: one entry, one JSON document, and nothing else.
//!
//! Measurement decided this shape. The content of a real store is short
//! single-line strings — descriptions median 196 characters with 0% containing a
//! newline, log messages median 326 with 1 in 337 containing one — so the format
//! that handles long strings correctly and needs no parser of its own wins over
//! the one that is nicer for prose nobody writes.
//!
//! The vocabulary is measured too. Three states, no priority, no project: across
//! 169 real entries `priority` was never set to anything but its default,
//! `project` was always the store's own name, and of five statuses two were used
//! twice and one (`active`) was a claim the tool cannot enforce. Labels carry
//! urgency; `created` carries order.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Done,
    Dropped,
}

impl State {
    pub fn parse(value: &str) -> Result<Self, ModelError> {
        match value.trim().to_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "done" => Ok(Self::Done),
            "dropped" | "closed" | "cancelled" | "canceled" => Ok(Self::Dropped),
            other => Err(ModelError::BadState(other.to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }

    /// Done or dropped: finished with, one way or another.
    pub fn is_closed(self) -> bool {
        !matches!(self, Self::Open)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        State::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// One log line: what happened, and when it was said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default)]
    pub ts: String,
    pub msg: String,
}

/// `Vec` that also accepts an explicit `null`, which readers of older files and
/// hand-edited documents both produce.
fn null_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// One entry, exactly as it appears on disk.
///
/// Field order is the file's key order, and the fields always written are
/// written even when empty, so the document has one shape rather than several.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub title: String,
    pub state: State,
    #[serde(default, deserialize_with = "null_vec")]
    pub labels: Vec<String>,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub done: Option<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub blocked_by: Vec<String>,
    /// Replaceable summary of where things stand. The log is the history; this
    /// is the current state of play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// What must be true for this to count as done.
    #[serde(default, deserialize_with = "null_vec")]
    pub acceptance: Vec<String>,
    /// What was done about it, oldest first.
    #[serde(default, deserialize_with = "null_vec")]
    pub log: Vec<LogEntry>,
    /// Keys tk does not know. Preserved rather than dropped, so a field a human
    /// added by hand survives the next write.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Entry {
    pub fn new(r#ref: String, title: String, created: String) -> Self {
        Self {
            r#ref,
            title,
            state: State::Open,
            labels: Vec::new(),
            created: created.clone(),
            updated: created,
            done: None,
            blocked_by: Vec::new(),
            status: None,
            acceptance: Vec::new(),
            log: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

/// An entry plus what a reader needs to know about its references.
#[derive(Debug, Clone, Serialize)]
pub struct EntryView {
    #[serde(flatten)]
    pub entry: Entry,
    /// Content fingerprint of the document as read, for `--if-rev`.
    pub rev: String,
    /// Blockers naming no entry in the store. Unresolved counts as blocking,
    /// never as done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_blockers: Vec<String>,
    /// Blockers that are still in the way: unresolved ones, and ones whose entry
    /// is still open. A blocker that is done or dropped is no longer a blocker,
    /// which is what makes `ready` mean "can be started now".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking: Vec<String>,
    /// The file this entry lives in.
    pub file: String,
}

impl EntryView {
    /// Waiting: something it names is unfinished. True even for a done entry
    /// that still names an open blocker, which is why display and filters ask
    /// [`Self::is_waiting`] instead.
    pub fn is_blocked(&self) -> bool {
        !self.blocking.is_empty()
    }

    /// Blocked *and* still to do, which is what a `blocked` marker means: a
    /// finished entry is not waiting on anything.
    pub fn is_waiting(&self) -> bool {
        self.entry.state == State::Open && self.is_blocked()
    }
    pub fn is_ready(&self) -> bool {
        self.entry.state == State::Open && !self.is_blocked()
    }
}

/// The settings in `.tk.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// On-disk layout version; this binary reads and writes [`crate::store::FORMAT`].
    pub format: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<std::collections::BTreeMap<String, String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            format: crate::store::FORMAT,
            aliases: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid state {0:?}: use open, done, or dropped")]
    BadState(String),
}

impl miette::Diagnostic for ModelError {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        Some(Box::new(crate::output::code::INVALID_INPUT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Entry {
        Entry {
            r#ref: "a7b3".into(),
            title: "Rewrite the auth layer".into(),
            state: State::Open,
            labels: vec!["backend".into()],
            created: "2026-01-10T12:00:00Z".into(),
            updated: "2026-02-01T09:30:00Z".into(),
            done: None,
            blocked_by: vec!["b7c4".into()],
            status: Some("Halfway".into()),
            acceptance: vec!["parity test passes".into()],
            log: vec![LogEntry {
                ts: "2026-01-10T09:00:00Z".into(),
                msg: "Started with the JWT approach.".into(),
            }],
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn states_parse_including_legacy_names() {
        assert_eq!(State::parse("open").unwrap(), State::Open);
        assert_eq!(State::parse("DONE").unwrap(), State::Done);
        assert_eq!(State::parse("dropped").unwrap(), State::Dropped);
        // Names from older layouts still read.
        assert_eq!(State::parse("closed").unwrap(), State::Dropped);
        assert_eq!(State::parse("cancelled").unwrap(), State::Dropped);
        assert!(State::parse("active").is_err(), "active is not a state");
        assert!(State::parse("deferred").is_err());
        assert!(State::parse("").is_err());
    }

    #[test]
    fn the_document_has_one_shape() {
        let json = serde_json::to_string_pretty(&sample()).unwrap();
        // Every field a reader may look for is present, even when empty.
        for key in [
            "ref",
            "title",
            "state",
            "labels",
            "created",
            "updated",
            "done",
            "blocked_by",
            "status",
            "acceptance",
            "log",
        ] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "{key} missing from {json}"
            );
        }
        // Top-level keys only: nested keys and values sit at a deeper indent.
        let keys: Vec<&str> = json
            .lines()
            .filter_map(|line| line.strip_prefix("  \""))
            .filter_map(|line| line.split('"').next())
            .collect();
        assert_eq!(
            keys,
            vec![
                "ref",
                "title",
                "state",
                "labels",
                "created",
                "updated",
                "done",
                "blocked_by",
                "status",
                "acceptance",
                "log"
            ],
            "key order is the file's shape"
        );
        assert_eq!(serde_json::from_str::<Entry>(&json).unwrap(), sample());
    }

    #[test]
    fn an_entry_with_nothing_optional_still_reads_and_writes() {
        let minimal = serde_json::json!({
            "ref": "a7b3",
            "title": "t",
            "state": "open",
            "labels": [],
            "created": "c",
            "updated": "u",
            "done": null,
            "blocked_by": null,
            "acceptance": null,
            "log": null
        });
        let entry: Entry = serde_json::from_value(minimal).unwrap();
        assert!(entry.blocked_by.is_empty());
        assert!(entry.log.is_empty());
        assert!(entry.status.is_none());
        let json = serde_json::to_string(&entry).unwrap();
        assert!(
            !json.contains("status"),
            "an unset status is omitted: {json}"
        );
    }

    #[test]
    fn closed_entries_and_readiness() {
        assert!(State::Done.is_closed());
        assert!(State::Dropped.is_closed());
        assert!(!State::Open.is_closed());

        let view = EntryView {
            entry: sample(),
            rev: "abc".into(),
            unresolved_blockers: vec!["b7c4".into()],
            blocking: vec!["b7c4".into()],
            file: "a7b3-rewrite-the-auth-layer.json".into(),
        };
        assert!(view.is_blocked());
        assert!(view.is_waiting());
        assert!(!view.is_ready(), "a blocked entry is never ready");

        let mut done = view.clone();
        done.entry.state = State::Done;
        assert!(done.is_blocked(), "it still names an unfinished blocker");
        assert!(!done.is_waiting(), "but it is not waiting on anything");
        assert!(!done.is_ready());
    }

    #[test]
    fn unknown_fields_are_preserved_not_dropped() {
        // A field a human or a newer writer added survives a read/write cycle,
        // which is what makes reserved fields unnecessary.
        let raw = serde_json::json!({
            "ref": "a7b3", "title": "t", "state": "open", "labels": [], "created": "c",
            "updated": "u", "done": null, "blocked_by": [], "acceptance": [], "log": [],
            "assignee": "nick", "future_field": 42
        });
        let entry: Entry = serde_json::from_value(raw).unwrap();
        assert_eq!(entry.r#ref, "a7b3");
        assert_eq!(
            entry.extra.get("assignee").and_then(|v| v.as_str()),
            Some("nick")
        );
        let round_tripped = serde_json::to_value(&entry).unwrap();
        assert_eq!(round_tripped["future_field"], serde_json::json!(42));
    }
}
