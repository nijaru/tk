//! Operations: the one implementation of everything tk does to an entry.
//!
//! Commands resolve a ref, call one operation, and print what it returns;
//! `apply` calls the same functions. There is deliberately no second path,
//! because v1 had one and the two drifted — the multi-field edit ordered its
//! writes differently in the two implementations, and only a test that ran both
//! and compared the results caught it.
//!
//! v1 split operations into lock-free (append) and locked (read-then-write).
//! That distinction is gone: with one document per entry, every change is a
//! whole-file rewrite and every one of them takes the store lock, so an enum
//! with one meaningful arm would be decoration. Every function here takes a
//! [`Txn`], and a `Txn` cannot be constructed without the lock.

use std::path::PathBuf;

use crate::ids;
use crate::model::{Entry, EntryView, LogEntry, State};
use crate::store::{Result, StoreError, Txn};

/// One entry, resolved and loaded, that an operation is about to change.
struct Change {
    r#ref: String,
    path: PathBuf,
    entry: Entry,
    /// A new slug, when the operation is renaming the file.
    slug: Option<String>,
}

impl Change {
    /// Resolve, check the caller's revision, load.
    fn open(txn: &Txn<'_>, input: &str, rev: Option<&str>) -> Result<Self> {
        let r#ref = txn.resolve(input)?;
        txn.store().check_rev(&r#ref, rev)?;
        let (path, entry) = txn.load(&r#ref)?;
        Ok(Self {
            r#ref,
            path,
            entry,
            slug: None,
        })
    }

    /// Stamp and write. The one place a mutation reaches the disk.
    fn save(&mut self, txn: &Txn<'_>, now: &str) -> Result<EntryView> {
        Txn::touch(&mut self.entry, now);
        let path = match &self.slug {
            Some(slug) => txn.write_as(&self.entry, slug, Some(&self.path))?,
            None => txn.write(&self.entry, Some(&self.path))?,
        };
        let known = txn.known()?;
        Ok(crate::store::view_of(
            &self.entry,
            &crate::store::file_name(&path),
            &known,
        ))
    }
}

/// Resolve a ref and refuse a stale revision, before a write.
///
/// Callers use this inside a transaction, so nothing can change between the
/// check and the write it protects.
pub fn resolve_rev(txn: &Txn<'_>, input: &str, rev: Option<&str>) -> Result<String> {
    let r#ref = txn.resolve(input)?;
    txn.store().check_rev(&r#ref, rev)?;
    Ok(r#ref)
}

/// Start a new entry.
pub fn create(txn: &Txn<'_>, title: &str, now: &str) -> Result<EntryView> {
    txn.create(title, now)
}

/// Append one log entry.
pub fn add_log(txn: &Txn<'_>, input: &str, msg: &str, now: &str) -> Result<EntryView> {
    let msg = msg.trim();
    if msg.is_empty() {
        return Err(StoreError::InvalidInput(
            "a log entry needs a message".into(),
        ));
    }
    let mut change = Change::open(txn, input, None)?;
    change.entry.log.push(LogEntry {
        ts: now.to_owned(),
        msg: msg.to_owned(),
    });
    change.save(txn, now)
}

/// Move to a state. Closing stamps the moment; reopening forgets it, because a
/// reopened entry is not done and a stale timestamp would say it was.
pub fn set_state(txn: &Txn<'_>, input: &str, state: State, now: &str) -> Result<EntryView> {
    let mut change = Change::open(txn, input, None)?;
    change.entry.state = state;
    change.entry.done = if state.is_closed() {
        Some(now.to_owned())
    } else {
        None
    };
    change.save(txn, now)
}

/// Apply `+add`, `-remove`, or bare-replace label changes.
///
/// A bare label (`tk label a7b3 urgent`) replaces the whole set, which is the
/// only kind of change that can lose a concurrent `+x`: it writes a set that was
/// read before the other writer's change landed. Deltas are the safe form, and
/// they are what the help text recommends for more than one agent.
pub fn edit_labels(txn: &Txn<'_>, input: &str, changes: &[String], now: &str) -> Result<EntryView> {
    if changes.is_empty() {
        return Err(StoreError::InvalidInput(
            "no labels given: use +tag, -tag, or a bare tag to replace the set".into(),
        ));
    }
    let mut change = Change::open(txn, input, None)?;
    apply_label_changes(&mut change.entry.labels, changes);
    change.save(txn, now)
}

/// Fold label changes into a set: bare values replace, `+`/`-` adjust.
fn apply_label_changes(current: &mut Vec<String>, changes: &[String]) {
    let bare: Vec<String> = changes
        .iter()
        .filter(|c| !c.starts_with(['+', '-']))
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
        .collect();
    if !bare.is_empty() {
        current.clear();
        current.extend(bare);
    }
    for change in changes {
        if let Some(add) = change.strip_prefix('+') {
            current.push(add.trim().to_owned());
        } else if let Some(remove) = change.strip_prefix('-')
            && !remove.trim().is_empty()
        {
            let remove = remove.trim();
            current.retain(|l| !l.eq_ignore_ascii_case(remove));
        }
    }
    normalize_labels(current);
}

/// Lowercase, drop blanks and duplicates, sort: one spelling per label, so
/// filtering and counting are unambiguous.
fn normalize_labels(labels: &mut Vec<String>) {
    labels.retain(|l| !l.trim().is_empty());
    for label in labels.iter_mut() {
        *label = label.trim().to_lowercase();
    }
    labels.sort();
    labels.dedup();
}

/// Add one blocker, refusing a loop.
pub fn add_blocker(txn: &Txn<'_>, input: &str, blocker: &str, now: &str) -> Result<EntryView> {
    let mut change = Change::open(txn, input, None)?;
    let blocker_ref = resolve_blocker(txn, &change.r#ref, blocker)?;
    if !change.entry.blocked_by.contains(&blocker_ref) {
        change.entry.blocked_by.push(blocker_ref);
        change.entry.blocked_by.sort();
    }
    change.save(txn, now)
}

/// Remove one blocker. `false` means it was not blocking.
pub fn remove_blocker(txn: &Txn<'_>, input: &str, blocker: &str, now: &str) -> Result<EntryView> {
    let mut change = Change::open(txn, input, None)?;
    let blocker_ref = txn.resolve(blocker)?;
    change.entry.blocked_by.retain(|b| b != &blocker_ref);
    change.save(txn, now)
}

fn resolve_blockers(txn: &Txn<'_>, own: &str, refs: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for r#ref in refs {
        let resolved = resolve_blocker(txn, own, r#ref)?;
        if !out.contains(&resolved) {
            out.push(resolved);
        }
    }
    out.sort();
    Ok(out)
}

/// Resolve a blocker and refuse anything that would close a loop.
///
/// An unresolved name is an error rather than a dropped reference: a blocker
/// that silently vanishes is a task that silently becomes ready.
fn resolve_blocker(txn: &Txn<'_>, own: &str, blocker: &str) -> Result<String> {
    let blocker_ref = txn.resolve(blocker)?;
    if blocker_ref == own {
        return Err(StoreError::WouldCycle {
            blocked: own.to_owned(),
            path: format!("{own} -> {own}"),
        });
    }
    if let Some(path) = would_cycle(txn, own, &blocker_ref)? {
        return Err(StoreError::WouldCycle {
            blocked: own.to_owned(),
            path,
        });
    }
    Ok(blocker_ref)
}

/// Walk `blocker`'s own blockers looking for `own`. Returns the loop, for the
/// message, rather than a bare yes.
fn would_cycle(txn: &Txn<'_>, own: &str, blocker: &str) -> Result<Option<String>> {
    let mut seen: Vec<String> = vec![blocker.to_owned()];
    let mut stack = vec![blocker.to_owned()];
    while let Some(current) = stack.pop() {
        if current == own {
            seen.push(current);
            return Ok(Some(seen.join(" -> ")));
        }
        let Ok((_, entry)) = txn.load(&current) else {
            continue;
        };
        for next in &entry.blocked_by {
            if !seen.contains(next) {
                seen.push(next.clone());
                stack.push(next.clone());
            }
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// The multi-field edit
// ---------------------------------------------------------------------------

/// Everything `tk edit` and `apply`'s `edit` intent can change, in one write.
#[derive(Debug, Default, Clone)]
pub struct Edit {
    pub title: Option<String>,
    pub slug: Option<String>,
    pub labels: Option<Vec<String>>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub blockers: Option<Vec<String>>,
    pub add_blockers: Vec<String>,
    pub remove_blockers: Vec<String>,
    /// Append one log entry along with the rest.
    pub note: Option<String>,
}

impl Edit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.slug.is_none()
            && self.labels.is_none()
            && self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.blockers.is_none()
            && self.add_blockers.is_empty()
            && self.remove_blockers.is_empty()
            && self.note.is_none()
    }

    /// Merge label changes into one delta list, in a fixed order.
    fn label_changes(&self) -> Vec<String> {
        let mut changes = Vec::new();
        if let Some(labels) = &self.labels {
            changes.extend(labels.iter().cloned());
        }
        changes.extend(self.add_labels.iter().map(|l| format!("+{l}")));
        changes.extend(self.remove_labels.iter().map(|l| format!("-{l}")));
        changes
    }
}

/// Apply every requested change in a single write.
///
/// One `Change`, one `save`: the fields cannot disagree about what they were
/// read from, and the file is rewritten once rather than once per field.
pub fn apply_edit(txn: &Txn<'_>, input: &str, edit: &Edit, now: &str) -> Result<EntryView> {
    if edit.is_empty() {
        return Err(StoreError::InvalidInput("nothing to change".into()));
    }
    if let Some(title) = &edit.title
        && title.trim().is_empty()
    {
        return Err(StoreError::EmptyTitle);
    }

    let mut change = Change::open(txn, input, None)?;
    if let Some(title) = &edit.title {
        change.entry.title = title.trim().to_owned();
    }
    if let Some(slug) = &edit.slug {
        change.slug = Some(ids::slug(slug));
    }
    let label_changes = edit.label_changes();
    if !label_changes.is_empty() {
        apply_label_changes(&mut change.entry.labels, &label_changes);
    }

    // Blockers are resolved before anything is written, so a bad reference
    // fails the whole edit rather than leaving half of it applied.
    let blockers = match (
        &edit.blockers,
        edit.add_blockers.is_empty(),
        edit.remove_blockers.is_empty(),
    ) {
        (Some(refs), _, _) => Some(resolve_blockers(txn, &change.r#ref, refs)?),
        (None, false, _) => {
            let mut current = change.entry.blocked_by.clone();
            for blocker in &edit.add_blockers {
                let resolved = resolve_blocker(txn, &change.r#ref, blocker)?;
                if !current.contains(&resolved) {
                    current.push(resolved);
                }
            }
            current.sort();
            Some(current)
        }
        (None, true, false) => {
            let mut current = change.entry.blocked_by.clone();
            for blocker in &edit.remove_blockers {
                let resolved = txn.resolve(blocker)?;
                current.retain(|b| b != &resolved);
            }
            Some(current)
        }
        (None, true, true) => None,
    };

    if let Some(blockers) = blockers {
        change.entry.blocked_by = blockers;
    }
    if let Some(note) = &edit.note {
        let note = note.trim();
        if note.is_empty() {
            return Err(StoreError::InvalidInput(
                "a log entry needs a message".into(),
            ));
        }
        change.entry.log.push(LogEntry {
            ts: now.to_owned(),
            msg: note.to_owned(),
        });
    }
    change.save(txn, now)
}

// ---------------------------------------------------------------------------
// Purge
// ---------------------------------------------------------------------------

/// What a purge removed, and what it unblocked.
#[derive(Debug, serde::Serialize)]
pub struct Purge {
    pub deleted: EntryView,
    /// Entries that were blocked by the deleted one, now unblocked.
    pub unblocked: Vec<String>,
}

/// Delete an entry, and drop references to it.
///
/// A dangling blocker would leave an entry permanently unready with nothing on
/// disk to explain why, so purge cleans up after itself. That is why it holds
/// the store lock and why it is not `rm`.
pub fn purge(txn: &Txn<'_>, input: &str) -> Result<Purge> {
    let r#ref = txn.resolve(input)?;
    let known = txn.known()?;
    let (path, entry) = txn.load(&r#ref)?;
    let deleted = crate::store::view_of(&entry, &crate::store::file_name(&path), &known);

    let mut unblocked = Vec::new();
    for (other_path, mut other) in txn.store().scan()?.entries {
        if other.r#ref == r#ref {
            continue;
        }
        if other.blocked_by.iter().any(|b| b == &r#ref) {
            other.blocked_by.retain(|b| b != &r#ref);
            Txn::touch(&mut other, &entry.updated);
            txn.write(&other, Some(&other_path))?;
            unblocked.push(other.r#ref);
        }
    }
    txn.delete(&r#ref)?;
    Ok(Purge { deleted, unblocked })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Ctx, Filter, TASKS_DIR};

    const NOW: &str = "2026-01-10T12:00:00Z";
    const LATER: &str = "2026-02-01T09:30:00Z";

    fn store() -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Ctx::at_tasks_dir(dir.path().join(TASKS_DIR));
        ctx.txn_init().unwrap();
        (dir, ctx)
    }

    fn add(ctx: &Ctx, title: &str) -> EntryView {
        let txn = ctx.txn().unwrap();
        create(&txn, title, NOW).unwrap()
    }

    #[test]
    fn the_log_keeps_every_entry_in_order() {
        let (_dir, ctx) = store();
        let view = add(&ctx, "Alpha");
        let txn = ctx.txn().unwrap();
        add_log(&txn, &view.entry.r#ref, "first", NOW).unwrap();
        let view = add_log(&txn, &view.entry.r#ref, "second", LATER).unwrap();
        assert_eq!(view.entry.log.len(), 2);
        assert_eq!(view.entry.log[0].ts, NOW);
        assert_eq!(view.entry.log[1].msg, "second");
        assert_eq!(view.entry.updated, LATER, "the log is the record of change");

        // Nothing about the log is replaceable, so there is no second narrative
        // to go stale or to disagree with it.
        assert!(matches!(
            add_log(&txn, &view.entry.r#ref, "   ", NOW),
            Err(StoreError::InvalidInput(_))
        ));
    }

    #[test]
    fn labels_are_normalized_and_deltas_compose() {
        let (_dir, ctx) = store();
        let view = add(&ctx, "Alpha");
        let txn = ctx.txn().unwrap();
        let labels = |changes: &[&str]| {
            edit_labels(
                &txn,
                &view.entry.r#ref,
                &changes.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
                NOW,
            )
            .unwrap()
            .entry
            .labels
        };
        assert_eq!(
            labels(&["+Backend", "+backend", "+ api "]),
            ["api", "backend"]
        );
        assert_eq!(labels(&["+urgent", "-backend"]), ["api", "urgent"]);
        // A bare label replaces the whole set.
        assert_eq!(labels(&["ops"]), ["ops"]);
        // Add and remove compose in one call, case-insensitively.
        assert_eq!(labels(&["+One", "-one"]), ["ops"]);
        assert_eq!(labels(&["+one", "-ops", "-ONE"]), Vec::<String>::new());
    }

    #[test]
    fn closing_stamps_and_reopening_clears() {
        let (_dir, ctx) = store();
        let view = add(&ctx, "Alpha");
        let txn = ctx.txn().unwrap();
        let done = set_state(&txn, &view.entry.r#ref, State::Done, LATER).unwrap();
        assert_eq!(done.entry.done.as_deref(), Some(LATER));
        let reopened = set_state(&txn, &done.entry.r#ref, State::Open, LATER).unwrap();
        assert!(reopened.entry.done.is_none());
        assert_eq!(reopened.entry.created, NOW, "created never moves");
        assert_eq!(reopened.entry.log.len(), 0);
    }

    #[test]
    fn blocking_is_checked_before_it_is_written() {
        let (_dir, ctx) = store();
        let a = add(&ctx, "Alpha");
        let b = add(&ctx, "Beta");
        let c = add(&ctx, "Gamma");
        let txn = ctx.txn().unwrap();

        add_blocker(&txn, &b.entry.r#ref, &a.entry.r#ref, NOW).unwrap();
        add_blocker(&txn, &c.entry.r#ref, &b.entry.r#ref, NOW).unwrap();

        // Direct loop.
        let err = add_blocker(&txn, &a.entry.r#ref, &a.entry.r#ref, NOW).unwrap_err();
        assert!(matches!(err, StoreError::WouldCycle { .. }), "{err:?}");
        // Indirect loop, with the path in the message.
        let err = add_blocker(&txn, &a.entry.r#ref, &c.entry.r#ref, NOW).unwrap_err();
        match err {
            StoreError::WouldCycle { path, .. } => {
                assert!(
                    path.contains("a7") || path.contains(&a.entry.r#ref),
                    "{path}"
                );
                assert!(path.contains("->"), "{path}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        // Nothing was written by the refusals.
        let store = ctx.store().unwrap();
        let alpha = store.get(&a.entry.r#ref).unwrap();
        assert!(
            alpha.entry.blocked_by.is_empty(),
            "{:?}",
            alpha.entry.blocked_by
        );

        // A blocker that does not exist is an error, not a drop.
        assert!(add_blocker(&txn, &a.entry.r#ref, "nope", NOW).is_err());
    }

    #[test]
    fn readiness_follows_blockers_and_completion() {
        let (_dir, ctx) = store();
        let a = add(&ctx, "Alpha");
        let b = add(&ctx, "Beta");
        let txn = ctx.txn().unwrap();
        add_blocker(&txn, &b.entry.r#ref, &a.entry.r#ref, NOW).unwrap();
        let store = ctx.store().unwrap();
        let ready = |f: Filter| store.list(&f).unwrap().0;
        // `ready` is open with nothing in the way — the same filter `tk ready`
        // builds, not a concept of its own.
        let startable = || Filter {
            state: Some(State::Open),
            blocked: Some(false),
            ..Default::default()
        };
        assert_eq!(ready(startable()).len(), 1);

        // Completing the blocker lets the blocked entry start.
        set_state(&txn, &a.entry.r#ref, State::Done, LATER).unwrap();
        let ready = ready(startable());
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].entry.r#ref, b.entry.r#ref);
    }

    #[test]
    fn a_dangling_blocker_is_unresolved_and_never_ready() {
        let (_dir, ctx) = store();
        let a = add(&ctx, "Alpha");
        // Written by hand, the way a lost purge or a hand edit would leave it.
        let path = ctx.tasks_dir.join(&a.file);
        let mut entry: Entry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        entry.blocked_by = vec!["zzzz".into()];
        std::fs::write(&path, serde_json::to_string_pretty(&entry).unwrap()).unwrap();

        let view = ctx.store().unwrap().get(&a.entry.r#ref).unwrap();
        assert_eq!(view.unresolved_blockers, ["zzzz"]);
        assert!(view.is_blocked());
        assert!(
            !view.is_ready(),
            "unresolved counts as blocking, never as done"
        );
    }

    #[test]
    fn the_multi_field_edit_writes_once_and_orders_nothing_badly() {
        let (_dir, ctx) = store();
        let view = add(&ctx, "Alpha");
        let txn = ctx.txn().unwrap();
        let edit = Edit {
            title: Some("Alpha, revised".into()),
            add_labels: vec!["backend".into()],
            note: Some("started".into()),
            ..Default::default()
        };
        let out = apply_edit(&txn, &view.entry.r#ref, &edit, LATER).unwrap();
        assert_eq!(out.entry.title, "Alpha, revised");
        assert_eq!(out.entry.labels, ["backend"]);
        assert_eq!(out.entry.log.len(), 1);
        assert_eq!(out.entry.updated, LATER);
        assert_eq!(out.file, view.file, "a title change does not move the file");
        assert_eq!(out.entry.created, NOW);

        // A bad reference fails the whole edit: the blockers are resolved before
        // anything is written.
        let edit = Edit {
            title: Some("Should not stick".into()),
            add_blockers: vec!["nope".into()],
            ..Default::default()
        };
        assert!(apply_edit(&txn, &view.entry.r#ref, &edit, LATER).is_err());
        let after = ctx.store().unwrap().get(&view.entry.r#ref).unwrap();
        assert_eq!(after.entry.title, "Alpha, revised");
    }

    #[test]
    fn an_edit_with_nothing_to_do_is_refused() {
        let (_dir, ctx) = store();
        let view = add(&ctx, "Alpha");
        let txn = ctx.txn().unwrap();
        assert!(matches!(
            apply_edit(&txn, &view.entry.r#ref, &Edit::default(), NOW),
            Err(StoreError::InvalidInput(_))
        ));
    }

    #[test]
    fn purge_unblocks_what_it_was_blocking() {
        let (_dir, ctx) = store();
        let a = add(&ctx, "Alpha");
        let b = add(&ctx, "Beta");
        let txn = ctx.txn().unwrap();
        add_blocker(&txn, &b.entry.r#ref, &a.entry.r#ref, NOW).unwrap();
        assert!(!ctx.store().unwrap().get(&b.entry.r#ref).unwrap().is_ready());

        let outcome = purge(&txn, &a.entry.r#ref).unwrap();
        assert_eq!(outcome.deleted.entry.r#ref, a.entry.r#ref);
        assert_eq!(outcome.unblocked, std::slice::from_ref(&b.entry.r#ref));

        let store = ctx.store().unwrap();
        assert!(store.get(&a.entry.r#ref).is_err(), "the entry is gone");
        let beta = store.get(&b.entry.r#ref).unwrap();
        assert!(
            beta.entry.blocked_by.is_empty(),
            "the reference was dropped"
        );
        assert!(beta.is_ready());
    }

    #[test]
    fn every_operation_refuses_a_stale_revision_without_writing() {
        let (_dir, ctx) = store();
        let view = add(&ctx, "Alpha");
        let txn = ctx.txn().unwrap();
        // The caller read an older revision.
        let mut change = Change::open(&txn, &view.entry.r#ref, None).unwrap();
        change.entry.labels = vec!["x".into()];
        change.save(&txn, LATER).unwrap();

        assert!(matches!(
            txn.store().check_rev(&view.entry.r#ref, Some(&view.rev)),
            Err(StoreError::StaleRevision { .. })
        ));
    }
}
