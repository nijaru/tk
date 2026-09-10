//! `tk edit`
//!
//! Single-value changes (title, priority, description, due, estimate) are
//! last-writer-wins appends and take no lock. Two cases need the lock, because
//! the value depends on a read rather than on the caller's intent:
//!
//! - bare label/assignee values *replace* the set, so the result depends on
//!   what is there now;
//! - `--if-rev` must compare and write with nothing in between.
//!
//! `+label` / `-label` are deltas and stay lock-free, so concurrent agents can
//! each add their own label without losing anyone's edit.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Priority;
use crate::record::op;
use crate::{format, timeutil};

use super::Writer;

/// Edit a task
#[derive(Args)]
pub struct Edit {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// New title
    #[usage(short = 't', long)]
    pub title: Option<String>,
    /// New priority
    #[usage(short = 'p', long)]
    pub priority: Option<String>,
    /// Labels: `+add` a value, or bare values to replace the set
    ///
    /// A leading `-` is read as a flag by the shell and this parser alike, so
    /// removal is `--remove-label`; `-l +x` and `--labels=-x` both work.
    #[usage(short = 'l', long, delimiter = ',')]
    pub labels: Vec<String>,
    /// Labels to remove (comma-separated)
    #[usage(long = "remove-label", delimiter = ',')]
    pub remove_labels: Vec<String>,
    /// Assignees: `+add` a value, or bare values to replace the set
    #[usage(short = 'A', long, delimiter = ',')]
    pub assignees: Vec<String>,
    /// Assignees to remove (comma-separated)
    #[usage(long = "remove-assignee", delimiter = ',')]
    pub remove_assignees: Vec<String>,
    /// Due date (YYYY-MM-DD, relative, or - to clear)
    #[usage(long)]
    pub due: Option<String>,
    /// Parent task (or - to clear)
    #[usage(long)]
    pub parent: Option<String>,
    /// Description (or - to clear)
    #[usage(short = 'd', long = "desc")]
    pub desc: Option<String>,
    /// Estimate (or 0 to clear)
    #[usage(long)]
    pub estimate: Option<i64>,
    /// Reject the edit unless the task still has this revision (`show --json`)
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Edit {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let replaces_labels = self.labels.iter().any(|v| !is_delta(v));
        let replaces_assignees = self.assignees.iter().any(|v| !is_delta(v));
        let needs_lock =
            self.if_rev.is_some() || self.parent.is_some() || replaces_labels || replaces_assignees;

        let writer = Writer::new(&ctx, needs_lock)?;
        let id = writer.store().resolve(&self.id).into_diagnostic()?;

        // One conditional check for the whole edit. Checking per append would
        // reject the second field of a legitimate multi-field edit, because the
        // first append already moved the revision. The lock is held throughout,
        // so a single check still means "nothing changed since I read it".
        writer
            .store()
            .check_rev(&id, self.if_rev.as_deref())
            .into_diagnostic()?;

        if let Some(title) = self.title {
            writer.append(&id, op::TITLE, serde_json::json!(title), None)?;
        }
        if let Some(priority) = self.priority {
            let p = Priority::parse(&priority).into_diagnostic()?;
            writer.append(&id, op::PRIORITY, serde_json::json!(p as u8), None)?;
        }
        if let Some(desc) = self.desc {
            let value = if desc == "-" { None } else { Some(desc) };
            writer.append(&id, op::DESCRIPTION, serde_json::json!(value), None)?;
        }
        if let Some(estimate) = self.estimate {
            let value = if estimate == 0 { None } else { Some(estimate) };
            writer.append(&id, op::ESTIMATE, serde_json::json!(value), None)?;
        }
        if let Some(due) = self.due {
            let parsed = timeutil::parse_due_date(&due).into_diagnostic()?;
            writer.append(&id, op::DUE_DATE, serde_json::json!(parsed), None)?;
        }

        let mut label_ops = self.labels;
        label_ops.extend(self.remove_labels.iter().map(|l| format!("-{l}")));
        apply_list_ops(&writer, &id, ListKind::Labels, label_ops, replaces_labels)?;

        let mut assignee_ops = self.assignees;
        assignee_ops.extend(self.remove_assignees.iter().map(|a| format!("-{a}")));
        apply_list_ops(
            &writer,
            &id,
            ListKind::Assignees,
            assignee_ops,
            replaces_assignees,
        )?;

        if let Some(parent) = self.parent {
            if parent == "-" {
                writer.append(&id, op::PARENT_CLEAR, serde_json::Value::Null, None)?;
            } else {
                let pid = writer.store().resolve(&parent).into_diagnostic()?;
                // Validate existence and acyclicity in the same transaction as
                // the write. `Writer::new` took the lock for exactly this case,
                // so opening a second transaction here would deadlock.
                let txn = writer.txn().ok_or_else(|| {
                    miette::miette!("internal error: a parent change needs the store lock")
                })?;
                txn.set_parent(&id, Some(&pid)).into_diagnostic()?;
            }
        }

        let updated = writer.view(&id)?;
        if ctx.json {
            println!("{}", format::format_json(&updated));
        } else {
            println!("Updated {}: {}", updated.task.alias, updated.task.title);
        }
        Ok(())
    }
}

/// `+x` adds, `-x` removes, bare values replace the whole set.
fn is_delta(value: &str) -> bool {
    value.starts_with('+') || value.starts_with('-')
}

#[derive(Clone, Copy)]
enum ListKind {
    Labels,
    Assignees,
}

impl ListKind {
    fn add_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_ADD,
            Self::Assignees => op::ASSIGNEES_ADD,
        }
    }
    fn remove_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_REMOVE,
            Self::Assignees => op::ASSIGNEES_REMOVE,
        }
    }
    fn set_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_SET,
            Self::Assignees => op::ASSIGNEES_SET,
        }
    }
    fn current(self, record: &crate::record::Record) -> &[String] {
        match self {
            Self::Labels => &record.state.labels,
            Self::Assignees => &record.state.assignees,
        }
    }
}

/// Apply `+`/`-` as commutative deltas; apply bare values as a replacement
/// computed from the current set (which is why the caller locked).
fn apply_list_ops(
    writer: &Writer<'_>,
    id: &str,
    kind: ListKind,
    ops: Vec<String>,
    replaces: bool,
) -> miette::Result<()> {
    if ops.is_empty() {
        return Ok(());
    }
    if !replaces {
        let add: Vec<String> = ops
            .iter()
            .filter_map(|v| v.strip_prefix('+').map(str::to_owned))
            .collect();
        let remove: Vec<String> = ops
            .iter()
            .filter_map(|v| v.strip_prefix('-').map(str::to_owned))
            .collect();
        if !add.is_empty() {
            writer.append(id, kind.add_op(), serde_json::json!(add), None)?;
        }
        if !remove.is_empty() {
            writer.append(id, kind.remove_op(), serde_json::json!(remove), None)?;
        }
        return Ok(());
    }

    let record = writer.load(id)?;
    let merged = apply_slice_updates(kind.current(&record), &ops);
    if merged != kind.current(&record) {
        writer.append(id, kind.set_op(), serde_json::json!(merged), None)?;
    }
    Ok(())
}

/// `+x` adds, `-x` removes, bare values replace the whole set (sorted).
fn apply_slice_updates(current: &[String], updates: &[String]) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<String> = current.iter().cloned().collect();
    let mut replaced = false;
    for s in updates {
        if let Some(add) = s.strip_prefix('+') {
            set.insert(add.to_owned());
        } else if let Some(rem) = s.strip_prefix('-') {
            set.remove(rem);
        } else {
            if !replaced {
                set.clear();
                replaced = true;
            }
            set.insert(s.clone());
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{apply_slice_updates, is_delta};

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn slice_ops() {
        assert_eq!(apply_slice_updates(&v(&["a"]), &v(&["+b"])), v(&["a", "b"]));
        assert_eq!(apply_slice_updates(&v(&["a", "b"]), &v(&["-a"])), v(&["b"]));
        assert_eq!(
            apply_slice_updates(&v(&["a"]), &v(&["x", "y"])),
            v(&["x", "y"])
        );
    }

    #[test]
    fn deltas_are_recognised() {
        assert!(is_delta("+x"));
        assert!(is_delta("-x"));
        assert!(!is_delta("x"));
    }
}
