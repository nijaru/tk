//! Task operations: one implementation per change, shared by the CLI and `tk apply`.
//!
//! A command is resolve → call → emit. `apply` is a dispatcher over the same
//! functions, so the two paths cannot drift apart.
//!
//! [`Mutation`] carries the lock decision. `Mutation::free` is reads plus
//! appends that are last-writer-wins or commutative; `Mutation::locked` holds
//! the store lock and is required by anything that must see a consistent store.
//! Operations that need the lock say so by asking for it, so forgetting it is a
//! loud internal error rather than a silent race.

use serde_json::Value;

use crate::ids;
use crate::model::{LogEntry, Priority, Status, TaskState, TaskView};
use crate::record::{Record, op};
use crate::store::{CreateOptions, Ctx, Store, StoreError, Txn};

/// A mutation handle over one store.
pub struct Mutation<'a> {
    store: Store<'a>,
    txn: Option<Txn<'a>>,
}

impl<'a> Mutation<'a> {
    /// No lock. Only for reads and for appends that do not depend on a read of
    /// the same record.
    pub fn free(ctx: &'a Ctx) -> Result<Self, StoreError> {
        Ok(Self {
            store: ctx.store()?,
            txn: None,
        })
    }

    /// Holds `<store>/.lock` for the life of the mutation.
    pub fn locked(ctx: &'a Ctx) -> Result<Self, StoreError> {
        let txn = ctx.txn()?;
        let store = Store::new(txn.ctx());
        Ok(Self {
            store,
            txn: Some(txn),
        })
    }

    /// Pick the cheapest handle that is still correct.
    ///
    /// Callers pass the *reason* they might need the lock (a conditional write,
    /// a graph check, a replacement computed from a read), so the decision lives
    /// with the operation that has the reason rather than in a convention.
    pub fn choose(ctx: &'a Ctx, needs_lock: bool) -> Result<Self, StoreError> {
        if needs_lock {
            Self::locked(ctx)
        } else {
            Self::free(ctx)
        }
    }

    pub fn store(&self) -> &Store<'a> {
        &self.store
    }

    fn locked_txn(&self, what: &str) -> Result<&Txn<'a>, StoreError> {
        self.txn.as_ref().ok_or_else(|| {
            StoreError::Msg(format!(
                "internal error: {what} requires the store lock; this is a bug in tk"
            ))
        })
    }

    /// Reject the operation unless the record still carries `expected`.
    pub fn check_rev(&self, id: &str, expected: Option<&str>) -> Result<(), StoreError> {
        match expected {
            None => Ok(()),
            Some(_) => self
                .locked_txn("a conditional write")?
                .check_rev(id, expected),
        }
    }

    pub fn load(&self, id: &str) -> Result<Record, StoreError> {
        self.store.load(id)
    }

    pub fn view(&self, id: &str) -> Result<TaskView, StoreError> {
        self.store.view_of(id)
    }

    fn append(&self, id: &str, op_name: &str, data: Value) -> Result<(), StoreError> {
        self.store.append(id, op_name, data)
    }

    fn append_and_view(
        &self,
        id: &str,
        op_name: &str,
        data: Value,
    ) -> Result<TaskView, StoreError> {
        self.store.append_and_view(id, op_name, data)
    }
}

// ---------------------------------------------------------------------------
// Create and single-value fields
// ---------------------------------------------------------------------------

/// Create a task. Locked: alias uniqueness is store-wide.
pub fn create(m: &Mutation<'_>, opts: CreateOptions) -> Result<TaskView, StoreError> {
    m.locked_txn("creating a task")?.create(opts)
}

pub fn set_title(
    m: &Mutation<'_>,
    id: &str,
    title: String,
    if_rev: Option<&str>,
) -> Result<TaskView, StoreError> {
    m.check_rev(id, if_rev)?;
    m.append_and_view(id, op::TITLE, Value::from(title))
}

pub fn set_description(
    m: &Mutation<'_>,
    id: &str,
    description: Option<String>,
    if_rev: Option<&str>,
) -> Result<TaskView, StoreError> {
    m.check_rev(id, if_rev)?;
    m.append_and_view(id, op::DESCRIPTION, serde_json::json!(description))
}

pub fn set_priority(
    m: &Mutation<'_>,
    id: &str,
    priority: Priority,
) -> Result<TaskView, StoreError> {
    m.append_and_view(id, op::PRIORITY, serde_json::json!(priority as u8))
}

/// Set workflow status. Last writer wins; completion time is derived by the
/// fold from the transition, so it cannot disagree with the status.
pub fn set_status(m: &Mutation<'_>, id: &str, status: Status) -> Result<TaskView, StoreError> {
    m.append_and_view(id, op::STATUS, serde_json::json!(status))
}

/// Change the display project. Identity is untouched, so no reference anywhere
/// needs rewriting.
pub fn set_project(m: &Mutation<'_>, id: &str, project: &str) -> Result<TaskView, StoreError> {
    ids::validate_project(project)?;
    m.append_and_view(id, op::PROJECT, serde_json::json!(project))
}

/// Replace or clear the current checkpoint.
pub fn set_checkpoint(
    m: &Mutation<'_>,
    id: &str,
    text: Option<String>,
    if_rev: Option<&str>,
) -> Result<TaskView, StoreError> {
    m.check_rev(id, if_rev)?;
    let text = text.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    m.append_and_view(id, op::CHECKPOINT, serde_json::json!(text))
}

pub fn add_log(m: &Mutation<'_>, id: &str, msg: &str) -> Result<TaskView, StoreError> {
    let entry = LogEntry {
        ts: String::new(),
        msg: msg.to_owned(),
    };
    let data = serde_json::to_value(&entry)
        .map_err(|e| StoreError::InvalidInput(format!("log entry: {e}")))?;
    m.append_and_view(id, op::LOG, data)
}

// ---------------------------------------------------------------------------
// Multi-field edit
// ---------------------------------------------------------------------------

/// A multi-field edit: what `tk edit` and `{"op":"edit"}` both express.
///
/// One struct so the two paths cannot apply the fields in different orders or
/// forget one when a field is added.
#[derive(Debug, Default, Clone)]
pub struct Edit {
    pub title: Option<String>,
    /// `None` leaves the description; `Some(None)` clears it.
    pub description: Option<Option<String>>,
    pub priority: Option<Priority>,
    /// `None` leaves the parent; `Some(None)` clears it; `Some(Some(r))` sets it
    /// from a reference, which is resolved here.
    pub parent: Option<Option<String>>,
    /// As the CLI accepts them: `+x` adds, `-x` removes, a bare value replaces.
    pub labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub if_rev: Option<String>,
}

impl Edit {
    /// Does this edit need the store lock?
    ///
    /// A parent change validates other records, a bare label value is a
    /// read-modify-write, and `--if-rev` must compare and write together.
    pub fn needs_lock(&self) -> bool {
        self.if_rev.is_some()
            || self.parent.is_some()
            || self.labels.iter().any(|v| !v.starts_with(['+', '-']))
    }
}

/// Apply a multi-field edit in a fixed order.
///
pub fn apply_edit(m: &Mutation<'_>, id: &str, edit: &Edit) -> Result<TaskView, StoreError> {
    // One conditional check for the whole edit. Checking per field would reject
    // the second field of a legitimate edit, because the first already moved the
    // revision; the caller holds the lock throughout, so one check still means
    // "nothing changed since I read it".
    m.check_rev(id, edit.if_rev.as_deref())?;

    if let Some(title) = &edit.title {
        set_title(m, id, title.clone(), None)?;
    }
    if let Some(description) = &edit.description {
        set_description(m, id, description.clone(), None)?;
    }
    if let Some(priority) = edit.priority {
        set_priority(m, id, priority)?;
    }
    if let Some(parent) = &edit.parent {
        match parent {
            None => {
                set_parent(m, id, None)?;
            }
            Some(reference) => {
                let pid = m.store().resolve(reference)?;
                set_parent(m, id, Some(&pid))?;
            }
        }
    }
    let mut labels = edit.labels.clone();
    labels.extend(edit.remove_labels.iter().map(|l| format!("-{l}")));
    edit_list(m, id, ListField::Labels, ListEdit::Deltas(&labels), None)
}

// ---------------------------------------------------------------------------
// List-valued fields
// ---------------------------------------------------------------------------

/// One list-valued field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListField {
    Labels,
    Links,
    Acceptance,
    Evidence,
}

impl ListField {
    /// The field's name, as the user sees it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Labels => "labels",
            Self::Links => "links",
            Self::Acceptance => "acceptance",
            Self::Evidence => "evidence",
        }
    }

    pub fn values(self, state: &TaskState) -> &[String] {
        match self {
            Self::Labels => &state.labels,
            Self::Links => &state.links,
            Self::Acceptance => &state.acceptance,
            Self::Evidence => &state.evidence,
        }
    }

    fn add_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_ADD,
            Self::Links => op::LINKS_ADD,
            Self::Acceptance => op::ACCEPTANCE_ADD,
            Self::Evidence => op::EVIDENCE_ADD,
        }
    }

    fn remove_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_REMOVE,
            Self::Links => op::LINKS_REMOVE,
            Self::Acceptance => op::ACCEPTANCE_REMOVE,
            Self::Evidence => op::EVIDENCE_REMOVE,
        }
    }

    fn set_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_SET,
            Self::Links => op::LINKS_SET,
            Self::Acceptance => op::ACCEPTANCE_SET,
            Self::Evidence => op::EVIDENCE_SET,
        }
    }

    /// Every list field, for tests and iteration.
    pub fn all() -> [Self; 4] {
        [Self::Labels, Self::Links, Self::Acceptance, Self::Evidence]
    }
}

/// What to do with a list field.
#[derive(Debug, Clone)]
pub enum ListEdit<'a> {
    /// Append values that are not already present.
    Add(&'a [String]),
    Remove(&'a [String]),
    Clear,
    /// Values as the CLI accepts them: `+x` adds, `-x` removes, a bare value
    /// replaces the whole set. Replacements are computed from the current
    /// state, which is why they need the lock.
    Deltas(&'a [String]),
}

/// Apply one list edit.
///
/// `Deltas` with any bare value is a read-modify-write and therefore requires
/// the lock; `Add`, `Remove`, `Clear`, and pure `+`/`-` deltas are commutative
/// appends and do not.
pub fn edit_list(
    m: &Mutation<'_>,
    id: &str,
    field: ListField,
    edit: ListEdit<'_>,
    if_rev: Option<&str>,
) -> Result<TaskView, StoreError> {
    m.check_rev(id, if_rev)?;
    match edit {
        ListEdit::Add(values) => m.append_and_view(id, field.add_op(), serde_json::json!(values)),
        ListEdit::Remove(values) => {
            m.append_and_view(id, field.remove_op(), serde_json::json!(values))
        }
        ListEdit::Clear => m.append_and_view(id, field.set_op(), serde_json::json!([])),
        ListEdit::Deltas(values) => apply_deltas(m, id, field, values),
    }
}

fn apply_deltas(
    m: &Mutation<'_>,
    id: &str,
    field: ListField,
    values: &[String],
) -> Result<TaskView, StoreError> {
    if values.is_empty() {
        return m.view(id);
    }
    let adds: Vec<String> = values
        .iter()
        .filter_map(|v| v.strip_prefix('+').map(str::to_owned))
        .collect();
    let removes: Vec<String> = values
        .iter()
        .filter_map(|v| v.strip_prefix('-').map(str::to_owned))
        .collect();
    let replaces = values.iter().any(|v| !v.starts_with(['+', '-']));

    if !replaces {
        if !adds.is_empty() {
            m.append(id, field.add_op(), serde_json::json!(adds))?;
        }
        if !removes.is_empty() {
            m.append(id, field.remove_op(), serde_json::json!(removes))?;
        }
        return m.view(id);
    }

    // A bare value replaces the set, so the result depends on what is there
    // now: this is the read-modify-write the lock exists for.
    m.locked_txn("replacing a list")?;
    let current = field.values(&m.load(id)?.state).to_vec();
    let merged = merge(&current, values);
    if merged != current {
        m.append(id, field.set_op(), serde_json::json!(merged))?;
    }
    m.view(id)
}

/// `+x` adds, `-x` removes, a bare value replaces everything before it.
fn merge(current: &[String], values: &[String]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = current.iter().cloned().collect();
    let mut replaced = false;
    for value in values {
        if let Some(add) = value.strip_prefix('+') {
            set.insert(add.to_owned());
        } else if let Some(remove) = value.strip_prefix('-') {
            set.remove(remove);
        } else {
            if !replaced {
                set.clear();
                replaced = true;
            }
            set.insert(value.clone());
        }
    }
    set.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

/// Add a blocking edge. Locked: the cycle check reads other records.
pub fn add_blocker(m: &Mutation<'_>, id: &str, blocker: &str) -> Result<TaskView, StoreError> {
    if id == blocker {
        return Err(StoreError::InvalidInput("task cannot block itself".into()));
    }
    m.locked_txn("adding a blocker")?.add_blocker(id, blocker)
}

pub fn remove_blocker(
    m: &Mutation<'_>,
    id: &str,
    blocker: &str,
) -> Result<(TaskView, bool), StoreError> {
    m.locked_txn("removing a blocker")?
        .remove_blocker(id, blocker)
}

/// Set or clear the parent. Locked: validation reads other records.
pub fn set_parent(
    m: &Mutation<'_>,
    id: &str,
    parent: Option<&str>,
) -> Result<TaskView, StoreError> {
    m.locked_txn("setting a parent")?.set_parent(id, parent)
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn archive(m: &Mutation<'_>, id: &str, if_rev: Option<&str>) -> Result<TaskView, StoreError> {
    let txn = m.locked_txn("archiving a task")?;
    txn.check_rev(id, if_rev)?;
    let record = txn.load(id)?;
    if !record.state.status.is_terminal() {
        return Err(StoreError::InvalidInput(format!(
            "only done or closed tasks can be archived ({} is {})",
            record.state.alias, record.state.status
        )));
    }
    if record.state.is_archived() {
        return txn.view_of(id);
    }
    m.append_and_view(id, op::ARCHIVED, Value::Null)
}

pub fn unarchive(m: &Mutation<'_>, id: &str) -> Result<TaskView, StoreError> {
    if !m.load(id)?.state.is_archived() {
        return m.view(id);
    }
    m.append_and_view(id, op::UNARCHIVED, Value::Null)
}

/// Delete a record. Locked: it checks whether other records still reference it.
pub fn purge(
    m: &Mutation<'_>,
    id: &str,
    scrub: bool,
    if_rev: Option<&str>,
) -> Result<crate::store::PurgeOutcome, StoreError> {
    let txn = m.locked_txn("deleting a task")?;
    txn.check_rev(id, if_rev)?;
    txn.purge(id, scrub)
}

// ---------------------------------------------------------------------------
// Store-wide operations
// ---------------------------------------------------------------------------

/// Change every task's display project. Locked: it writes several records.
pub fn rename_project(
    m: &Mutation<'_>,
    old: &str,
    new: &str,
) -> Result<crate::store::RenameResult, StoreError> {
    m.locked_txn("renaming a project")?.rename_project(old, new)
}

/// Archive (or delete) terminal tasks older than `days`.
pub fn clean(
    m: &Mutation<'_>,
    days: i64,
    purge_records: bool,
) -> Result<crate::store::CleanOutcome, StoreError> {
    m.locked_txn("cleaning the store")?
        .clean(days, purge_records)
}

/// Truncate torn tails left by interrupted writes.
pub fn recover(
    m: &Mutation<'_>,
    id: Option<&str>,
    dry_run: bool,
) -> Result<crate::store::RecoverOutcome, StoreError> {
    m.locked_txn("recovering records")?.recover(id, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn deltas_add_remove_and_replace() {
        assert_eq!(merge(&v(&["a"]), &v(&["+b"])), v(&["a", "b"]));
        assert_eq!(merge(&v(&["a", "b"]), &v(&["-a"])), v(&["b"]));
        // A bare value replaces everything before it, including earlier adds.
        assert_eq!(merge(&v(&["a"]), &v(&["+c", "x", "y"])), v(&["x", "y"]));
        assert_eq!(
            merge(&v(&["a", "b"]), &v(&["-a", "-b"])),
            Vec::<String>::new()
        );
        // Order is stable and duplicates collapse.
        assert_eq!(merge(&v(&["b", "a"]), &v(&["+a"])), v(&["a", "b"]));
    }

    #[test]
    fn every_field_has_a_distinct_op_set() {
        let mut ops = std::collections::HashSet::new();
        for field in ListField::all() {
            for op_name in [field.add_op(), field.remove_op(), field.set_op()] {
                assert!(ops.insert(op_name), "duplicate op {op_name}");
            }
        }
        assert_eq!(ops.len(), 12);
    }
}
