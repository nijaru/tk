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
use crate::ops::{self, Edit};
use crate::store::{Result, StoreError, Txn};

/// One requested change.
///
/// Serialized as `{"op": "...", ...}`. An unknown `op` is refused by name; an
/// unknown field inside a known op is ignored, because serde cannot reject
/// unknown fields in an internally tagged enum. A misspelled *required* field
/// fails loudly, which is the case that matters.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
// `Edit` carries every field any operation can set, so it is much the largest
// variant. Boxing it would not make the intent easier to read or to construct.
#[allow(clippy::large_enum_variant)]
pub enum Intent {
    /// Create an entry, then apply anything else it carries.
    Add {
        title: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        blocked_by: Vec<String>,
    },
    Note {
        #[serde(rename = "ref")]
        r#ref: String,
        message: String,
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
        labels: Option<Vec<String>>,
        #[serde(default)]
        add_labels: Vec<String>,
        #[serde(default)]
        remove_labels: Vec<String>,
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
            Self::State { .. } => "state",
            Self::Edit { .. } => "edit",
            Self::Block { .. } => "block",
            Self::Unblock { .. } => "unblock",
            Self::Label { .. } => "label",
            Self::Purge { .. } => "purge",
        }
    }

    /// What this intent is about, for an error message.
    fn subject(&self) -> String {
        match self {
            Self::Add { title, .. } => format!("{title:?}"),
            Self::Note { r#ref, .. }
            | Self::State { r#ref, .. }
            | Self::Edit { r#ref, .. }
            | Self::Block { r#ref, .. }
            | Self::Unblock { r#ref, .. }
            | Self::Label { r#ref, .. }
            | Self::Purge { r#ref } => r#ref.clone(),
        }
    }

    fn to_edit(&self) -> Option<Edit> {
        match self {
            Self::Edit {
                title,
                slug,
                labels,
                add_labels,
                remove_labels,
                blocked_by,
                add_blockers,
                remove_blockers,
                note,
                ..
            } => Some(Edit {
                title: title.clone(),
                slug: slug.clone(),
                labels: labels.clone(),
                add_labels: add_labels.clone(),
                remove_labels: remove_labels.clone(),
                blockers: blocked_by.clone(),
                add_blockers: add_blockers.clone(),
                remove_blockers: remove_blockers.clone(),
                note: note.clone(),
            }),
            _ => None,
        }
    }
}

/// A batch: `{"intents": [...]}`.
///
/// Parsed by hand rather than with `#[serde(untagged)]`, because that form
/// reports every inner failure as "data did not match any variant", which hides
/// the field an agent actually got wrong. One shape only — a second accepted
/// spelling is a second thing to document, test, and get wrong.
#[derive(Debug)]
pub struct Batch {
    pub intents: Vec<Intent>,
}

impl Batch {
    pub fn parse(input: &str) -> Result<Self> {
        if input.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "no batch on stdin: pipe {\"intents\": [...]}".into(),
            ));
        }
        let value: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| StoreError::InvalidInput(format!("batch is not valid JSON: {e}")))?;
        let serde_json::Value::Object(mut map) = value else {
            return Err(StoreError::InvalidInput(
                "a batch is a JSON object with an \"intents\" array".into(),
            ));
        };
        let intents = map
            .remove("intents")
            .ok_or_else(|| StoreError::InvalidInput("a batch needs an \"intents\" array".into()))?;
        Ok(Self {
            intents: parse_intents(intents)?,
        })
    }

    pub fn intents(&self) -> &[Intent] {
        &self.intents
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
/// A batch that cannot be parsed is refused before anything runs: an intent that
/// could never run (unknown op, missing field) should not cost half a batch's
/// worth of writes.
pub fn run(txn: &Txn<'_>, batch: &Batch, dry_run: bool, now: &str) -> Result<Report> {
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
            // Nothing here can check a constraint that depends on an earlier
            // intent in the same batch, because nothing is written.
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
            blocked_by,
        } => {
            let mut view = ops::create(txn, title, now)?;
            if !labels.is_empty() {
                let edit = Edit {
                    labels: Some(labels.clone()),
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
        Intent::State { r#ref, state } => ops::set_state(txn, r#ref, *state, now),
        Intent::Edit { r#ref, .. } => {
            let edit = intent.to_edit().expect("edit intent");
            ops::apply_edit(txn, r#ref, &edit, now)
        }
        Intent::Block { r#ref, blocker } => ops::add_blocker(txn, r#ref, blocker, now),
        Intent::Unblock { r#ref, blocker } => ops::remove_blocker(txn, r#ref, blocker, now),
        Intent::Label { r#ref, changes } => ops::edit_labels(txn, r#ref, changes, now),
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
    fn a_batch_has_one_shape() {
        let parsed = batch(r#"{"intents": [{"op": "add", "title": "Alpha"}]}"#);
        assert_eq!(parsed.intents().len(), 1);
        // A bare array is not a batch: one spelling, one thing to document.
        let error = Batch::parse(r#"[{"op": "add", "title": "Alpha"}]"#).unwrap_err();
        assert!(format!("{error}").contains("intents"), "{error}");
    }

    #[test]
    fn a_batch_that_is_not_a_batch_is_refused_with_a_reason() {
        for bad in [
            "",
            "   ",
            "{not json",
            r#"{"ints": []}"#,
            r#"{"intents": [{"op": "nope"}]}"#,
            // Ops that no longer exist must be refused by name, not accepted.
            r#"{"intents": [{"op": "status", "ref": "a7b3", "text": "x"}]}"#,
            r#"{"intents": [{"op": "accept", "ref": "a7b3", "criteria": ["x"]}]}"#,
        ] {
            let error = Batch::parse(bad).unwrap_err();
            assert!(
                matches!(error, StoreError::InvalidInput(_)),
                "{bad:?} gave {error:?}"
            );
        }
    }

    #[test]
    fn a_missing_field_names_the_intent_that_is_incomplete() {
        let error = Batch::parse(r#"{"intents": [{"op": "note", "ref": "a7b3"}]}"#).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("message"), "{message}");
    }

    #[test]
    fn intents_run_in_order() {
        let (_dir, ctx) = store();
        let text = r#"{"intents": [
            {"op": "add", "title": "Alpha", "labels": ["backend"]},
            {"op": "note", "ref": "alpha", "message": "started"},
            {"op": "edit", "ref": "alpha", "title": "Alpha, revised"},
            {"op": "label", "ref": "alpha", "changes": ["+urgent"]}
        ]}"#;
        let txn = ctx.txn().unwrap();
        let report = run(&txn, &batch(text), false, NOW).unwrap();
        assert!(report.failed.is_none());
        assert_eq!(report.applied.len(), 4);
        assert_eq!(report.not_attempted, 0);

        let entry = report.applied.last().unwrap().entry.clone();
        assert_eq!(entry.entry.title, "Alpha, revised");
        assert_eq!(entry.entry.labels, ["backend", "urgent"]);
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
             "add_labels": ["backend"], "note": "started"},
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
                add_labels: vec!["backend".into()],
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
            &batch(r#"{"intents": [{"op": "note", "ref": "nope", "message": "x"}]}"#),
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
