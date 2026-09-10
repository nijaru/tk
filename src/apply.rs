//! `tk apply`: a batch of intents from stdin, executed in order under one lock.
//!
//! This is a dispatcher, not a second implementation. Every intent calls the
//! same [`crate::ops`] function the corresponding command calls, so a batch and a
//! sequence of commands cannot disagree — which is exactly what happened in v1,
//! where `apply` had its own copy of all seventeen operations and the two had
//! already drifted on field order.
//!
//! Honest about what it is: intents are written one at a time, in order. A
//! failure stops the batch and the intents before it stay applied. It is not a
//! transaction and nothing is rolled back.

use serde::{Deserialize, Serialize};

use crate::model::{EntryView, State};
use crate::ops::{self, AcceptanceChange, Edit};
use crate::store::{Result, StoreError, Txn};

/// One requested change.
///
/// Serialized as `{"op": "...", ...}`. Unknown `op` values are refused by name;
/// an unknown field inside a known op is ignored, which is why every optional
/// field has a name an agent would guess.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
// `Edit` carries every field any op can set, so it is much the largest variant.
// Boxing it would not make the intent easier to read or to construct.
#[allow(clippy::large_enum_variant)]
pub enum Intent {
    /// Create an entry, then apply anything else it carries.
    Add {
        title: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        acceptance: Vec<String>,
        #[serde(default)]
        blocked_by: Vec<String>,
        #[serde(default)]
        status: Option<String>,
    },
    Note {
        #[serde(rename = "ref")]
        r#ref: String,
        message: String,
    },
    Status {
        #[serde(rename = "ref")]
        r#ref: String,
        #[serde(default)]
        text: Option<String>,
        /// Drop the status entirely.
        #[serde(default)]
        clear: bool,
    },
    State {
        #[serde(rename = "ref")]
        r#ref: String,
        state: State,
    },
    Edit {
        #[serde(rename = "ref")]
        r#ref: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        slug: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        clear_status: bool,
        #[serde(default)]
        labels: Option<Vec<String>>,
        #[serde(default)]
        add_labels: Vec<String>,
        #[serde(default)]
        remove_labels: Vec<String>,
        #[serde(default)]
        acceptance: Option<Vec<String>>,
        #[serde(default)]
        add_acceptance: Vec<String>,
        #[serde(default)]
        remove_acceptance: Vec<String>,
        #[serde(default)]
        clear_acceptance: bool,
        #[serde(default)]
        blocked_by: Option<Vec<String>>,
        #[serde(default)]
        add_blockers: Vec<String>,
        #[serde(default)]
        remove_blockers: Vec<String>,
        #[serde(default)]
        note: Option<String>,
    },
    Block {
        #[serde(rename = "ref")]
        r#ref: String,
        blocker: String,
    },
    Unblock {
        #[serde(rename = "ref")]
        r#ref: String,
        blocker: String,
    },
    Label {
        #[serde(rename = "ref")]
        r#ref: String,
        changes: Vec<String>,
    },
    Accept {
        #[serde(rename = "ref")]
        r#ref: String,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
        #[serde(default)]
        set: Option<Vec<String>>,
        #[serde(default)]
        clear: bool,
    },
    Purge {
        #[serde(rename = "ref")]
        r#ref: String,
    },
}

impl Intent {
    fn op(&self) -> &'static str {
        match self {
            Self::Add { .. } => "add",
            Self::Note { .. } => "note",
            Self::Status { .. } => "status",
            Self::State { .. } => "state",
            Self::Edit { .. } => "edit",
            Self::Block { .. } => "block",
            Self::Unblock { .. } => "unblock",
            Self::Label { .. } => "label",
            Self::Accept { .. } => "accept",
            Self::Purge { .. } => "purge",
        }
    }

    /// The ref this intent names, or the title for `add`.
    fn subject(&self) -> String {
        match self {
            Self::Add { title, .. } => format!("{title:?}"),
            Self::Note { r#ref, .. }
            | Self::Status { r#ref, .. }
            | Self::State { r#ref, .. }
            | Self::Edit { r#ref, .. }
            | Self::Block { r#ref, .. }
            | Self::Unblock { r#ref, .. }
            | Self::Label { r#ref, .. }
            | Self::Accept { r#ref, .. }
            | Self::Purge { r#ref } => r#ref.clone(),
        }
    }

    fn to_edit(&self) -> Option<Edit> {
        match self {
            Self::Edit {
                title,
                slug,
                status,
                clear_status,
                labels,
                add_labels,
                remove_labels,
                acceptance,
                add_acceptance,
                remove_acceptance,
                clear_acceptance,
                blocked_by,
                add_blockers,
                remove_blockers,
                note,
                ..
            } => Some(Edit {
                title: title.clone(),
                slug: slug.clone(),
                status: match (clear_status, status) {
                    (true, _) => Some(None),
                    (false, Some(text)) => Some(Some(text.clone())),
                    (false, None) => None,
                },
                labels: labels.clone(),
                add_labels: add_labels.clone(),
                remove_labels: remove_labels.clone(),
                acceptance: AcceptanceChange {
                    set: acceptance.clone(),
                    add: add_acceptance.clone(),
                    remove: remove_acceptance.clone(),
                    clear: *clear_acceptance,
                },
                blockers: blocked_by.clone(),
                add_blockers: add_blockers.clone(),
                remove_blockers: remove_blockers.clone(),
                note: note.clone(),
            }),
            _ => None,
        }
    }
}

/// A batch as it arrives: either `{"intents": [...]}` or a bare `[...]`.
///
/// Parsed by hand rather than with `#[serde(untagged)]`, because that form
/// reports every inner failure as "data did not match any variant", which hides
/// the field an agent actually got wrong.
#[derive(Debug)]
pub enum Batch {
    Wrapped { intents: Vec<Intent>, dry_run: bool },
    Bare(Vec<Intent>),
}

impl Batch {
    pub fn parse(input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "no batch on stdin: pipe {\"intents\": [...]} or a JSON array".into(),
            ));
        }
        let value: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| StoreError::InvalidInput(format!("batch is not valid JSON: {e}")))?;
        match value {
            serde_json::Value::Array(_) => {
                let intents = parse_intents(value)?;
                Ok(Self::Bare(intents))
            }
            serde_json::Value::Object(mut map) => {
                let intents = map.remove("intents").ok_or_else(|| {
                    StoreError::InvalidInput(
                        "a batch object needs an \"intents\" array, or pass a bare array".into(),
                    )
                })?;
                let dry_run = map
                    .remove("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(Self::Wrapped {
                    intents: parse_intents(intents)?,
                    dry_run,
                })
            }
            _ => Err(StoreError::InvalidInput(
                "a batch is a JSON object with \"intents\", or an array of intents".into(),
            )),
        }
    }

    pub fn intents(&self) -> &[Intent] {
        match self {
            Self::Wrapped { intents, .. } => intents,
            Self::Bare(intents) => intents,
        }
    }

    /// `--dry-run` on the command line, or `dry_run` inside the batch.
    pub fn dry_run(&self, flag: bool) -> bool {
        flag || matches!(self, Self::Wrapped { dry_run: true, .. })
    }
}

fn parse_intents(value: serde_json::Value) -> Result<Vec<Intent>> {
    serde_json::from_value(value)
        .map_err(|e| StoreError::InvalidInput(format!("intent is not valid: {e}")))
}

/// What one intent did.
#[derive(Debug, Serialize)]
pub struct Applied {
    pub index: usize,
    pub op: &'static str,
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub entry: EntryView,
}

/// Why the batch stopped.
#[derive(Debug, Serialize)]
pub struct Failed {
    pub index: usize,
    pub op: &'static str,
    pub subject: String,
    pub error_code: String,
    pub message: String,
}

/// The whole result.
#[derive(Debug, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub applied: Vec<Applied>,
    pub failed: Option<Failed>,
    /// Intents after the failure, which were not attempted.
    pub not_attempted: usize,
    /// What a caller must not assume. Printed with the JSON too, because it is
    /// the part that bites.
    pub note: &'static str,
}

pub const NOT_A_TRANSACTION: &str = "intents are applied in order under one lock; a failure stops the \
     batch and earlier intents stay applied — this is not a transaction and nothing rolls back";

/// Run a batch.
///
/// Structural validation happens first and refuses the whole batch: an intent
/// that could never run (unknown op, missing field) should not cost half a
/// batch's worth of writes.
pub fn run(txn: &Txn<'_>, batch: &Batch, dry_run: bool, now: &str) -> Result<Report> {
    let dry_run = batch.dry_run(dry_run);
    let intents = batch.intents();
    if intents.is_empty() {
        return Err(StoreError::InvalidInput("the batch is empty".into()));
    }

    let mut report = Report {
        dry_run,
        applied: Vec::new(),
        failed: None,
        not_attempted: 0,
        note: NOT_A_TRANSACTION,
    };

    for (index, intent) in intents.iter().enumerate() {
        if dry_run {
            // Resolve what must already exist, and say what would be written.
            // Nothing here can check a loop that depends on an earlier intent in
            // the same batch, because nothing is written.
            let entry = dry_run_entry(txn, intent, now)?;
            report.applied.push(Applied {
                index,
                op: intent.op(),
                r#ref: entry.entry.r#ref.clone(),
                entry,
            });
            continue;
        }
        match run_one(txn, intent, now) {
            Ok(entry) => report.applied.push(Applied {
                index,
                op: intent.op(),
                r#ref: entry.entry.r#ref.clone(),
                entry,
            }),
            Err(error) => {
                report.failed = Some(Failed {
                    index,
                    op: intent.op(),
                    subject: intent.subject(),
                    error_code: error.code().to_owned(),
                    message: format!("{error}"),
                });
                report.not_attempted = intents.len() - index - 1;
                return Ok(report);
            }
        }
    }
    Ok(report)
}

/// One intent, through the shared operations.
fn run_one(txn: &Txn<'_>, intent: &Intent, now: &str) -> Result<EntryView> {
    match intent {
        Intent::Add {
            title,
            labels,
            acceptance,
            blocked_by,
            status,
        } => {
            let mut view = ops::create(txn, title, now)?;
            if !labels.is_empty() || !acceptance.is_empty() || status.is_some() {
                let edit = Edit {
                    labels: if labels.is_empty() {
                        None
                    } else {
                        Some(labels.clone())
                    },
                    status: status.clone().map(Some),
                    acceptance: AcceptanceChange {
                        set: if acceptance.is_empty() {
                            None
                        } else {
                            Some(acceptance.clone())
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                };
                view = ops::apply_edit(txn, &view.entry.r#ref, &edit, now)?;
            }
            for blocker in blocked_by {
                view = ops::add_blocker(txn, &view.entry.r#ref, blocker, now)?;
            }
            Ok(view)
        }
        Intent::Note { r#ref, message } => ops::add_log(txn, r#ref, message, now),
        Intent::Status { r#ref, text, clear } => {
            let text = if *clear { None } else { text.as_deref() };
            ops::set_status(txn, r#ref, text, now)
        }
        Intent::State { r#ref, state } => ops::set_state(txn, r#ref, *state, now),
        Intent::Edit { r#ref, .. } => {
            let edit = intent.to_edit().expect("edit intent");
            ops::apply_edit(txn, r#ref, &edit, now)
        }
        Intent::Block { r#ref, blocker } => ops::add_blocker(txn, r#ref, blocker, now),
        Intent::Unblock { r#ref, blocker } => ops::remove_blocker(txn, r#ref, blocker, now),
        Intent::Label { r#ref, changes } => ops::edit_labels(txn, r#ref, changes, now),
        Intent::Accept {
            r#ref,
            add,
            remove,
            set,
            clear,
        } => ops::edit_acceptance(
            txn,
            r#ref,
            &AcceptanceChange {
                set: set.clone(),
                add: add.clone(),
                remove: remove.clone(),
                clear: *clear,
            },
            now,
        ),
        Intent::Purge { r#ref } => Ok(ops::purge(txn, r#ref)?.deleted),
    }
}

/// What a dry run reports: the entry as it is now, or as it would be created.
fn dry_run_entry(txn: &Txn<'_>, intent: &Intent, now: &str) -> Result<EntryView> {
    use crate::model::Entry;
    match intent {
        Intent::Add { title, .. } => {
            if title.trim().is_empty() {
                return Err(StoreError::EmptyTitle);
            }
            let candidate = crate::ids::new_ref();
            let entry = Entry::new(candidate, title.trim().to_owned(), now.to_owned());
            let known = txn.known()?;
            Ok(crate::store::view_of(
                &entry,
                &crate::ids::file_name_of(&entry.r#ref, &crate::ids::slug(title)),
                &known,
            ))
        }
        Intent::Purge { r#ref } => txn.store().get(r#ref),
        _ => {
            let subject = intent.subject();
            txn.store().get(&subject)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Entry;
    use crate::store::{Ctx, Filter, TASKS_DIR};

    const NOW: &str = "2026-01-10T12:00:00Z";

    fn store() -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Ctx::at_tasks_dir(dir.path().join(TASKS_DIR));
        ctx.txn_init().unwrap();
        (dir, ctx)
    }

    fn batch(json: &str) -> Batch {
        Batch::parse(json).expect("parses")
    }

    #[test]
    fn a_wrapped_batch_and_a_bare_array_are_the_same_batch() {
        let wrapped = batch(r#"{"intents": [{"op": "add", "title": "Alpha"}]}"#);
        let bare = batch(r#"[{"op": "add", "title": "Alpha"}]"#);
        assert_eq!(wrapped.intents().len(), 1);
        assert_eq!(bare.intents().len(), 1);
        assert!(!wrapped.dry_run(false));
        assert!(batch(r#"{"intents": [], "dry_run": true}"#).dry_run(false));
        assert!(bare.dry_run(true), "the command-line flag wins");
    }

    #[test]
    fn a_batch_that_is_not_json_is_refused_with_a_reason() {
        for bad in ["", "   ", "{not json", r#"{"intents": [{"op": "nope"}]}"#] {
            let error = Batch::parse(bad).unwrap_err();
            assert!(
                matches!(error, StoreError::InvalidInput(_)),
                "{bad:?} gave {error:?}"
            );
        }
    }

    #[test]
    fn a_missing_field_names_the_intent_that_is_incomplete() {
        let error = Batch::parse(r#"[{"op": "note", "ref": "a7b3"}]"#).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("message"), "{message}");
    }

    #[test]
    fn intents_run_in_order_and_write_once_each() {
        let (_dir, ctx) = store();
        let text = r#"{"intents": [
            {"op": "add", "title": "Alpha", "labels": ["backend"], "acceptance": ["works"]},
            {"op": "note", "ref": "alpha", "message": "started"},
            {"op": "status", "ref": "alpha", "text": "halfway"},
            {"op": "label", "ref": "alpha", "changes": ["+urgent"]}
        ]}"#;
        let txn = ctx.txn().unwrap();
        let report = run(&txn, &batch(text), false, NOW).unwrap();
        assert!(report.failed.is_none());
        assert_eq!(report.applied.len(), 4);
        assert_eq!(report.not_attempted, 0);

        let entry = report.applied.last().unwrap().entry.clone();
        assert_eq!(entry.entry.title, "Alpha");
        assert_eq!(entry.entry.labels, ["backend", "urgent"]);
        assert_eq!(entry.entry.status.as_deref(), Some("halfway"));
        assert_eq!(entry.entry.acceptance, ["works"]);
        assert_eq!(entry.entry.log.len(), 1);
        assert_eq!(entry.entry.log[0].msg, "started");
        assert_eq!(entry.entry.updated, NOW);
    }

    #[test]
    fn a_batch_and_the_equivalent_commands_agree() {
        // The test that caught real drift in v1: the same changes made two ways
        // must produce the same document, field for field.
        let (_dir, batch_ctx) = store();
        let (_dir2, cmd_ctx) = store();

        let text = r#"{"intents": [
            {"op": "add", "title": "Rewrite the auth layer"},
            {"op": "add", "title": "Write the parser"},
            {"op": "edit", "ref": "auth", "title": "Rewrite the auth layer, properly",
             "status": "halfway", "add_labels": ["backend"],
             "add_acceptance": ["parity test passes"], "note": "started"},
            {"op": "block", "ref": "parser", "blocker": "auth"},
            {"op": "state", "ref": "parser", "state": "dropped"}
        ]}"#;
        let txn = batch_ctx.txn().unwrap();
        run(&txn, &batch(text), false, NOW).unwrap();

        let txn = cmd_ctx.txn().unwrap();
        let auth = ops::create(&txn, "Rewrite the auth layer", NOW).unwrap();
        let parser = ops::create(&txn, "Write the parser", NOW).unwrap();
        ops::apply_edit(
            &txn,
            &auth.entry.r#ref,
            &Edit {
                title: Some("Rewrite the auth layer, properly".into()),
                status: Some(Some("halfway".into())),
                add_labels: vec!["backend".into()],
                acceptance: AcceptanceChange {
                    add: vec!["parity test passes".into()],
                    ..Default::default()
                },
                note: Some("started".into()),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        ops::add_blocker(&txn, &parser.entry.r#ref, &auth.entry.r#ref, NOW).unwrap();
        ops::set_state(&txn, &parser.entry.r#ref, State::Dropped, NOW).unwrap();

        let everything = Filter {
            include_closed: true,
            ..Default::default()
        };
        let (batch_views, _) = batch_ctx.store().unwrap().list(&everything).unwrap();
        let (cmd_views, _) = cmd_ctx.store().unwrap().list(&everything).unwrap();
        assert_eq!(batch_views.len(), 2);
        assert_eq!(cmd_views.len(), 2);
        assert_eq!(
            batch_ctx
                .store()
                .unwrap()
                .list(&Default::default())
                .unwrap()
                .0
                .len(),
            1,
            "the dropped entry is hidden by default",
        );

        // Compare the documents, ignoring the refs (allocated per store) and the
        // blockers that point at them.
        let strip = |views: &[EntryView]| -> Vec<Entry> {
            let auth = views
                .iter()
                .find(|v| v.entry.title.starts_with("Rewrite"))
                .expect("the auth entry")
                .entry
                .r#ref
                .clone();
            let mut out: Vec<Entry> = views
                .iter()
                .map(|v| {
                    let mut entry = v.entry.clone();
                    entry.r#ref = String::new();
                    entry.blocked_by = entry
                        .blocked_by
                        .iter()
                        .map(|b| {
                            if *b == auth {
                                "AUTH".to_owned()
                            } else {
                                b.clone()
                            }
                        })
                        .collect();
                    entry
                })
                .collect();
            out.sort_by(|a, b| a.title.cmp(&b.title));
            out
        };
        assert_eq!(strip(&batch_views), strip(&cmd_views));
    }

    #[test]
    fn a_failure_stops_the_batch_and_says_what_was_not_attempted() {
        let (_dir, ctx) = store();
        let text = r#"{"intents": [
            {"op": "add", "title": "Alpha"},
            {"op": "note", "ref": "nope", "message": "x"},
            {"op": "add", "title": "Beta"}
        ]}"#;
        let txn = ctx.txn().unwrap();
        let report = run(&txn, &batch(text), false, NOW).unwrap();
        assert_eq!(report.applied.len(), 1, "the first intent was applied");
        let failure = report.failed.as_ref().expect("a failure");
        assert_eq!(failure.index, 1);
        assert_eq!(failure.op, "note");
        assert_eq!(failure.error_code, "not_found");
        assert_eq!(report.not_attempted, 1, "Beta was not attempted");

        // The applied intent stayed applied: no rollback, and the CLI says so.
        let store = ctx.store().unwrap();
        assert_eq!(store.list(&Default::default()).unwrap().0.len(), 1);
        assert!(report.note.contains("not a transaction"));
    }

    #[test]
    fn a_refused_cycle_is_a_failed_intent_not_a_broken_store() {
        let (_dir, ctx) = store();
        let text = r#"{"intents": [
            {"op": "add", "title": "Alpha"},
            {"op": "add", "title": "Beta"},
            {"op": "block", "ref": "beta", "blocker": "alpha"},
            {"op": "block", "ref": "alpha", "blocker": "beta"}
        ]}"#;
        let txn = ctx.txn().unwrap();
        let report = run(&txn, &batch(text), false, NOW).unwrap();
        let failure = report.failed.as_ref().expect("a failure");
        assert_eq!(failure.index, 3);
        assert_eq!(failure.error_code, "invalid_input");
        assert!(failure.message.contains("loop"), "{}", failure.message);
        // The three that did run are intact and consistent.
        assert!(crate::store::check_integrity(&ctx).unwrap().is_empty());
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let (_dir, ctx) = store();
        let text = r#"{"intents": [
            {"op": "add", "title": "Alpha"},
            {"op": "add", "title": "Beta", "labels": ["x"]}
        ]}"#;
        let txn = ctx.txn().unwrap();
        let report = run(&txn, &batch(text), true, NOW).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.applied.len(), 2);
        assert!(report.applied[0].entry.entry.r#ref.len() == 4);
        // Nothing on disk.
        assert!(
            ctx.store()
                .unwrap()
                .list(&Default::default())
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            std::fs::read_dir(&ctx.tasks_dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .filter(|e| e.file_name() != crate::store::STORE_FILE)
                .count(),
            0
        );
    }

    #[test]
    fn a_dry_run_still_reports_a_ref_it_cannot_resolve() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let error = run(
            &txn,
            &batch(r#"[{"op": "note", "ref": "nope", "message": "x"}]"#),
            true,
            NOW,
        )
        .unwrap_err();
        assert!(
            matches!(error, StoreError::Id(crate::ids::IdError::NotFound(_))),
            "{error:?}"
        );
    }

    #[test]
    fn an_empty_batch_is_refused() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        assert!(matches!(
            run(&txn, &batch(r#"{"intents": []}"#), false, NOW),
            Err(StoreError::InvalidInput(_))
        ));
    }

    #[test]
    fn purge_through_a_batch_unblocks_what_it_blocked() {
        let (_dir, ctx) = store();
        let text = r#"{"intents": [
            {"op": "add", "title": "Alpha"},
            {"op": "add", "title": "Beta"},
            {"op": "block", "ref": "beta", "blocker": "alpha"},
            {"op": "purge", "ref": "alpha"}
        ]}"#;
        let txn = ctx.txn().unwrap();
        let report = run(&txn, &batch(text), false, NOW).unwrap();
        assert!(report.failed.is_none());
        let store = ctx.store().unwrap();
        let (views, _) = store.list(&Default::default()).unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].entry.title, "Beta");
        assert!(views[0].is_ready(), "the dangling blocker was dropped");
    }
}
