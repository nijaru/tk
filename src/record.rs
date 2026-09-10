//! Append-only task records.
//!
//! Each task is one file, `.tasks/records/<ulid>.jsonl`, holding one JSON event
//! per line. Events are immutable and the current state is a fold over the
//! lines, so:
//!
//! - history is free — nothing a writer recorded is ever overwritten;
//! - a `git merge` of two clones that touched different records never
//!   conflicts, and one that touched the same record conflicts at line
//!   granularity instead of losing a whole document;
//! - appends do not need a lock. `O_APPEND` moves the offset and writes in one
//!   syscall, so cooperating writers cannot lose each other's events even
//!   without coordination. Operations that must validate *other* records, or
//!   read-then-write one record conditionally, still take the store lock (see
//!   [`crate::store`]).
//!
//! A torn tail — a write interrupted before its newline — is detected rather
//! than parsed, and [`crate::store::check_integrity`] reports it until
//! `tk recover` truncates it.

use std::fs::{self, OpenOptions};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{LogEntry, Status, TaskState};
use crate::store::{Result, StoreError};
use crate::timeutil;

/// Event operation names. The string is the on-disk contract.
pub mod op {
    pub const CREATED: &str = "created";
    pub const PROJECT: &str = "project";
    pub const TITLE: &str = "title";
    pub const DESCRIPTION: &str = "description";
    pub const PRIORITY: &str = "priority";
    pub const STATUS: &str = "status";
    pub const DUE_DATE: &str = "due_date";
    pub const ESTIMATE: &str = "estimate";
    pub const CHECKPOINT: &str = "checkpoint";
    pub const ASSIGNEE: &str = "assignee";
    pub const ATTEMPT: &str = "attempt";
    pub const LOG: &str = "log";
    pub const LABELS_ADD: &str = "labels.add";
    pub const LABELS_REMOVE: &str = "labels.remove";
    pub const LABELS_SET: &str = "labels.set";
    pub const ASSIGNEES_ADD: &str = "assignees.add";
    pub const ASSIGNEES_REMOVE: &str = "assignees.remove";
    pub const ASSIGNEES_SET: &str = "assignees.set";
    pub const LINKS_ADD: &str = "links.add";
    pub const LINKS_REMOVE: &str = "links.remove";
    pub const LINKS_SET: &str = "links.set";
    pub const ACCEPTANCE_ADD: &str = "acceptance.add";
    pub const ACCEPTANCE_REMOVE: &str = "acceptance.remove";
    pub const ACCEPTANCE_SET: &str = "acceptance.set";
    pub const EVIDENCE_ADD: &str = "evidence.add";
    pub const EVIDENCE_REMOVE: &str = "evidence.remove";
    pub const EVIDENCE_SET: &str = "evidence.set";
    pub const BLOCK_ADD: &str = "block.add";
    pub const BLOCK_REMOVE: &str = "block.remove";
    pub const RELATED_ADD: &str = "related.add";
    pub const RELATED_REMOVE: &str = "related.remove";
    pub const PARENT_SET: &str = "parent.set";
    pub const PARENT_CLEAR: &str = "parent.clear";
    pub const ARCHIVED: &str = "archived";
    pub const UNARCHIVED: &str = "unarchived";
    /// Reserved for compaction: a full state that replaces everything before
    /// it. Accepted by the fold so a future writer can compact without a
    /// format break; nothing writes it yet.
    pub const SNAPSHOT: &str = "snapshot";
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// One line of a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// RFC3339Nano, UTC.
    pub ts: String,
    /// Which process appended it: `<pid>-<4 random chars>`.
    pub writer: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// Identity of this process, recorded on every event it appends.
///
/// Stable for the life of the process so a revision token says which writer
/// produced the last event, and different across restarts so two writers can
/// never share a token by accident.
pub fn writer_id() -> &'static str {
    static WRITER: OnceLock<String> = OnceLock::new();
    WRITER.get_or_init(|| format!("{}-{}", std::process::id(), crate::ids::new_alias()))
}

impl Event {
    pub fn new(op: &str, data: Value) -> Self {
        Self {
            ts: timeutil::now_rfc3339_nano(),
            writer: writer_id().to_owned(),
            op: op.to_owned(),
            data,
        }
    }

    /// The event as one JSON line, newline included.
    pub fn line(&self) -> String {
        let mut out = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned());
        out.push('\n');
        out
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A record's events plus everything the fold produced from them.
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub events: Vec<Event>,
    pub state: TaskState,
    /// The file ends without a newline: a write was interrupted. The partial
    /// line is ignored by the fold and reported by `check`.
    pub torn: bool,
    /// Complete lines that did not parse as events. The record still folds
    /// from the lines that did, so one bad line cannot hide a task.
    pub damaged: Vec<String>,
}

impl Record {
    /// Revision token: which writer touched the record last, how many events
    /// it has, and a fingerprint of the folded state.
    ///
    /// The event count alone detects any append, so a stale `--if-rev` is
    /// rejected; the writer and hash make the rejection explainable and catch
    /// a truncated or replaced file.
    pub fn rev(&self) -> String {
        let writer = self.last_writer();
        format!("{writer}:{}:{}", self.events.len(), hash8(&self.state))
    }

    fn last_writer(&self) -> &str {
        self.events.last().map(|e| e.writer.as_str()).unwrap_or("-")
    }

    pub fn is_clean(&self) -> bool {
        !self.torn && self.damaged.is_empty()
    }
}

/// FNV-1a 64-bit over the canonical state JSON, as 8 hex characters.
///
/// A change detector, not a security primitive.
pub fn hash8(state: &TaskState) -> String {
    let bytes = serde_json::to_vec(state).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

/// Read and fold a record.
pub fn read(path: &Path) -> Result<Record> {
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let data = fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StoreError::TaskNotFound(id.clone())
        } else {
            StoreError::Io(e)
        }
    })?;
    let (body, torn) = split_tail(&data);
    let mut events = Vec::new();
    let mut damaged = Vec::new();
    for (n, line) in body.split(|&b| b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Event>(line) {
            Ok(ev) => events.push(ev),
            Err(e) => damaged.push(format!("line {}: {e}", n + 1)),
        }
    }
    let state = fold(&events)?;
    Ok(Record {
        id,
        events,
        state,
        torn,
        damaged,
    })
}

/// Just the alias information, without folding the whole record.
///
/// Used by resolution, which must not pay for a full fold per record.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub alias: String,
    pub legacy_aliases: Vec<String>,
}

/// Read a record's identity from its `created` event.
pub fn read_identity(path: &Path) -> Result<Identity> {
    let file = fs::File::open(path).map_err(StoreError::Io)?;
    let mut first = String::new();
    BufReader::new(file)
        .read_line(&mut first)
        .map_err(StoreError::Io)?;
    let ev: Event =
        serde_json::from_str(first.trim_end()).map_err(|e| parse_err("record identity", e))?;
    if ev.op != op::CREATED {
        return Err(StoreError::Msg(format!(
            "{} does not start with a created event",
            path.display()
        )));
    }
    let state: TaskState =
        serde_json::from_value(ev.data).map_err(|e| parse_err("created event", e))?;
    Ok(Identity {
        alias: state.alias,
        legacy_aliases: state.legacy_aliases,
    })
}

/// Split a record's bytes into the complete lines and whether the tail is torn.
fn split_tail(data: &[u8]) -> (&[u8], bool) {
    if data.is_empty() || data.ends_with(b"\n") {
        return (data, false);
    }
    match data.iter().rposition(|&b| b == b'\n') {
        Some(i) => (&data[..=i], true),
        None => (&data[..0], true),
    }
}

/// Byte offset and length of a torn tail, for `tk recover`.
pub fn torn_tail(path: &Path) -> Result<Option<(u64, u64)>> {
    let data = fs::read(path).map_err(StoreError::Io)?;
    if data.is_empty() || data.ends_with(b"\n") {
        return Ok(None);
    }
    let keep = match data.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => 0,
    };
    Ok(Some((keep as u64, (data.len() - keep) as u64)))
}

// ---------------------------------------------------------------------------
// Appending
// ---------------------------------------------------------------------------

/// Append one event, durably, without taking the store lock.
///
/// `O_APPEND` (not seek-then-write) is what makes this safe against other
/// appenders: the offset update and the write are one operation, so two
/// writers cannot overwrite each other's lines.
pub fn append(path: &Path, event: &Event) -> Result<()> {
    let created = !path.exists();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(StoreError::Io)?;
    file.write_all(event.line().as_bytes())
        .map_err(StoreError::Io)?;
    file.sync_all().map_err(StoreError::Io)?;
    drop(file);
    if created {
        // Durably record the directory entry, not just the file contents.
        sync_dir(path);
    }
    Ok(())
}

/// Drop a torn tail, keeping every complete line.
pub fn truncate_torn(path: &Path) -> Result<()> {
    let Some((keep, dropped)) = torn_tail(path)? else {
        return Ok(());
    };
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(StoreError::Io)?;
    file.set_len(keep).map_err(StoreError::Io)?;
    file.sync_all().map_err(StoreError::Io)?;
    drop(file);
    sync_dir(path);
    let _ = dropped;
    Ok(())
}

fn sync_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------

/// Fold a record's events into current state.
pub fn fold(events: &[Event]) -> Result<TaskState> {
    let mut state: Option<TaskState> = None;
    for event in events {
        apply(&mut state, event)?;
    }
    state.ok_or_else(|| StoreError::Msg("record has no created event".into()))
}

fn apply(state: &mut Option<TaskState>, event: &Event) -> Result<()> {
    // `created` and `snapshot` replace the whole state; everything else
    // modifies it and needs one to exist.
    if event.op == op::CREATED || event.op == op::SNAPSHOT {
        let replaced: TaskState = serde_json::from_value(event.data.clone())
            .map_err(|e| parse_err("created state", e))?;
        *state = Some(replaced);
        return Ok(());
    }
    let Some(s) = state.as_mut() else {
        return Err(StoreError::Msg(
            "record has events before its created event".into(),
        ));
    };
    match event.op.as_str() {
        op::PROJECT => s.project = text(&event.data)?,
        op::TITLE => s.title = text(&event.data)?,
        op::DESCRIPTION => s.description = optional_text(&event.data)?,
        op::PRIORITY => s.priority = value(&event.data)?,
        op::STATUS => {
            let status: Status = value(&event.data)?;
            s.status = status;
            // Completion time is derived from the transition, not supplied by
            // the caller, so it cannot disagree with the status.
            s.completed_at = (status == Status::Done).then(|| event.ts.clone());
        }
        op::DUE_DATE => s.due_date = optional_text(&event.data)?,
        op::ESTIMATE => s.estimate = value(&event.data)?,
        op::CHECKPOINT => {
            s.checkpoint = optional_text(&event.data)?.filter(|t| !t.trim().is_empty())
        }
        op::ASSIGNEE => s.assignee = optional_text(&event.data)?,
        op::ATTEMPT => s.attempt = value(&event.data)?,
        op::LOG => {
            let entry: LogEntry = value(&event.data)?;
            s.logs.push(LogEntry {
                ts: if entry.ts.is_empty() {
                    event.ts.clone()
                } else {
                    entry.ts
                },
                msg: entry.msg,
            });
        }
        op::LABELS_ADD => add_all(&mut s.labels, &event.data)?,
        op::LABELS_REMOVE => remove_all(&mut s.labels, &event.data)?,
        op::LABELS_SET => s.labels = text_list(&event.data)?,
        op::ASSIGNEES_ADD => add_all(&mut s.assignees, &event.data)?,
        op::ASSIGNEES_REMOVE => remove_all(&mut s.assignees, &event.data)?,
        op::ASSIGNEES_SET => s.assignees = text_list(&event.data)?,
        op::LINKS_ADD => add_all(&mut s.links, &event.data)?,
        op::LINKS_REMOVE => remove_all(&mut s.links, &event.data)?,
        op::LINKS_SET => s.links = text_list(&event.data)?,
        op::ACCEPTANCE_ADD => add_all(&mut s.acceptance, &event.data)?,
        op::ACCEPTANCE_REMOVE => remove_all(&mut s.acceptance, &event.data)?,
        op::ACCEPTANCE_SET => s.acceptance = text_list(&event.data)?,
        op::EVIDENCE_ADD => add_all(&mut s.evidence, &event.data)?,
        op::EVIDENCE_REMOVE => remove_all(&mut s.evidence, &event.data)?,
        op::EVIDENCE_SET => s.evidence = text_list(&event.data)?,
        op::BLOCK_ADD => add_all(&mut s.blocked_by, &event.data)?,
        op::BLOCK_REMOVE => remove_all(&mut s.blocked_by, &event.data)?,
        op::RELATED_ADD => add_all(&mut s.related, &event.data)?,
        op::RELATED_REMOVE => remove_all(&mut s.related, &event.data)?,
        op::PARENT_SET => s.parent = Some(text(&event.data)?),
        op::PARENT_CLEAR => s.parent = None,
        op::ARCHIVED => s.archived_at = Some(event.ts.clone()),
        op::UNARCHIVED => s.archived_at = None,
        // An operation this binary does not know about is a newer writer's
        // business, not corruption.
        _ => return Ok(()),
    }
    s.updated_at = event.ts.clone();
    Ok(())
}

fn value<T: serde::de::DeserializeOwned>(data: &Value) -> Result<T> {
    serde_json::from_value(data.clone()).map_err(|e| parse_err("event data", e))
}

fn text(data: &Value) -> Result<String> {
    value(data)
}

fn optional_text(data: &Value) -> Result<Option<String>> {
    value(data)
}

fn text_list(data: &Value) -> Result<Vec<String>> {
    value(data)
}

fn add_all(target: &mut Vec<String>, data: &Value) -> Result<()> {
    for item in text_list(data)? {
        let item = item.trim().to_owned();
        if !item.is_empty() && !target.contains(&item) {
            target.push(item);
        }
    }
    Ok(())
}

fn remove_all(target: &mut Vec<String>, data: &Value) -> Result<()> {
    let doomed = text_list(data)?;
    target.retain(|item| !doomed.iter().any(|d| d.trim() == item));
    Ok(())
}

fn parse_err(what: impl Into<String>, err: impl ToString) -> StoreError {
    StoreError::Parse {
        what: what.into(),
        err: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Priority;
    use serde_json::json;

    fn created() -> Event {
        Event::new(
            op::CREATED,
            serde_json::to_value(TaskState {
                id: "01j8x0m5r7000000000000000a".to_owned(),
                alias: "a7b3".to_owned(),
                legacy_aliases: Vec::new(),
                project: "demo".to_owned(),
                title: "first".to_owned(),
                description: None,
                status: Status::Open,
                priority: Priority::Medium,
                labels: Vec::new(),
                assignees: Vec::new(),
                assignee: None,
                attempt: 0,
                parent: None,
                blocked_by: Vec::new(),
                related: Vec::new(),
                estimate: None,
                due_date: None,
                logs: Vec::new(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
                completed_at: None,
                archived_at: None,
                checkpoint: None,
                links: Vec::new(),
                acceptance: Vec::new(),
                evidence: Vec::new(),
            })
            .unwrap(),
        )
    }

    fn at(op_name: &str, data: Value, ts: &str) -> Event {
        let mut e = Event::new(op_name, data);
        e.ts = ts.to_owned();
        e
    }

    #[test]
    fn fold_applies_last_write_wins_and_deltas() {
        let events = vec![
            created(),
            at(op::TITLE, json!("second"), "2026-01-02T00:00:00Z"),
            at(op::LABELS_ADD, json!(["a", "b"]), "2026-01-03T00:00:00Z"),
            at(op::LABELS_ADD, json!(["b", "c"]), "2026-01-04T00:00:00Z"),
            at(op::LABELS_REMOVE, json!(["a"]), "2026-01-05T00:00:00Z"),
            at(op::LABELS_ADD, json!(["c"]), "2026-01-06T00:00:00Z"),
        ];
        let s = fold(&events).unwrap();
        assert_eq!(s.title, "second");
        assert_eq!(s.labels, vec!["b", "c"], "adds dedupe, order is stable");
        assert_eq!(s.updated_at, "2026-01-06T00:00:00Z");
    }

    #[test]
    fn completion_time_comes_from_the_transition() {
        let events = vec![
            created(),
            at(op::STATUS, json!("active"), "T1"),
            at(op::STATUS, json!("done"), "T2"),
        ];
        assert_eq!(fold(&events).unwrap().completed_at.as_deref(), Some("T2"));

        let mut back = events.clone();
        back.push(at(op::STATUS, json!("open"), "T3"));
        let s = fold(&back).unwrap();
        assert_eq!(s.completed_at, None, "reopening clears the completion");
    }

    #[test]
    fn archived_and_log_events_fold() {
        let events = vec![
            created(),
            at(op::STATUS, json!("closed"), "T1"),
            at(op::LOG, json!({"msg": "note"}), "T2"),
            at(op::ARCHIVED, Value::Null, "T3"),
            at(op::UNARCHIVED, Value::Null, "T4"),
        ];
        let s = fold(&events).unwrap();
        assert_eq!(s.logs.len(), 1);
        assert_eq!(s.logs[0].ts, "T2");
        assert_eq!(s.logs[0].msg, "note");
        assert_eq!(s.archived_at, None);
    }

    #[test]
    fn unknown_ops_are_ignored_not_fatal() {
        let events = vec![
            created(),
            at("teleport", json!({"to": "mars"}), "T1"),
            at(op::TITLE, json!("survived"), "T2"),
        ];
        let s = fold(&events).unwrap();
        assert_eq!(s.title, "survived");
    }

    #[test]
    fn related_edges_fold_like_other_lists() {
        let events = vec![
            created(),
            at(op::RELATED_ADD, json!(["01m25qbfqa26kxz59mxg4va3mq"]), "T1"),
            at(op::RELATED_ADD, json!(["01m25qbfqa26kxz59mxg4va3mq"]), "T2"),
            at(
                op::RELATED_REMOVE,
                json!(["01m25qbfqa26kxz59mxg4va3mq"]),
                "T3",
            ),
        ];
        assert!(fold(&events).unwrap().related.is_empty());
    }

    #[test]
    fn a_snapshot_replaces_prior_state() {
        let mut replacement = fold(&[created()]).unwrap();
        replacement.title = "compacted".to_owned();
        let events = vec![
            created(),
            at(op::TITLE, json!("ignored"), "T1"),
            at(
                op::SNAPSHOT,
                serde_json::to_value(&replacement).unwrap(),
                "T2",
            ),
            at(op::TITLE, json!("after"), "T3"),
        ];
        assert_eq!(fold(&events).unwrap().title, "after");
    }

    #[test]
    fn events_before_created_are_corruption() {
        let err = fold(&[at(op::TITLE, json!("x"), "T1")]).unwrap_err();
        assert!(
            err.to_string().contains("before its created event"),
            "{err}"
        );
        assert!(fold(&[]).is_err(), "an empty record has no state");
    }

    #[test]
    fn torn_tail_is_split_not_parsed() {
        let data = b"{\"a\":1}\n{\"b\":2}\n{\"c\":";
        let (body, torn) = split_tail(data);
        assert!(torn);
        assert_eq!(body, b"{\"a\":1}\n{\"b\":2}\n");

        let (body, torn) = split_tail(b"{\"a\":1}\n");
        assert!(!torn);
        assert_eq!(body, b"{\"a\":1}\n");

        let (body, torn) = split_tail(b"");
        assert!(!torn);
        assert!(body.is_empty());
    }

    #[test]
    fn revision_tracks_writer_lines_and_content() {
        let events = vec![created()];
        let mut state = fold(&events).unwrap();
        let base = format!("{}:1:{}", events[0].writer, hash8(&state));
        assert_eq!(
            Record {
                id: "x".into(),
                events: events.clone(),
                state: state.clone(),
                torn: false,
                damaged: Vec::new(),
            }
            .rev(),
            base
        );

        state.title = "changed".to_owned();
        assert_ne!(
            format!("{}:1:{}", events[0].writer, hash8(&state)),
            base,
            "content changes must move the revision"
        );
    }

    #[test]
    fn appends_are_durable_and_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.jsonl");
        append(&path, &created()).unwrap();
        append(&path, &at(op::TITLE, json!("second"), "T2")).unwrap();
        let record = read(&path).unwrap();
        assert!(record.is_clean());
        assert_eq!(record.events.len(), 2);
        assert_eq!(record.state.title, "second");
    }

    #[test]
    fn a_torn_tail_is_reported_and_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.jsonl");
        append(&path, &created()).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"ts\":\"T2\",\"writer\":\"w\",").unwrap();
        drop(file);

        let record = read(&path).unwrap();
        assert!(record.torn, "a partial line must be detected");
        assert_eq!(record.events.len(), 1, "the partial line is not applied");
        assert!(!record.is_clean());

        let (keep, dropped) = torn_tail(&path).unwrap().unwrap();
        assert!(dropped > 0 && keep > 0);
        truncate_torn(&path).unwrap();
        let repaired = read(&path).unwrap();
        assert!(repaired.is_clean(), "recovery must leave a clean record");
        assert_eq!(repaired.events.len(), 1);
        assert!(torn_tail(&path).unwrap().is_none());
    }

    #[test]
    fn a_bad_line_does_not_hide_the_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.jsonl");
        append(&path, &created()).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"not json\n").unwrap();
        file.write_all(at(op::TITLE, json!("still here"), "T3").line().as_bytes())
            .unwrap();
        drop(file);

        let record = read(&path).unwrap();
        assert_eq!(record.damaged.len(), 1, "{record:?}");
        assert_eq!(record.state.title, "still here");
    }
}
