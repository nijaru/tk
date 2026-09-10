//! `tk edit`
//!
//! The read that label/assignee updates depend on happens inside the same
//! transaction as the write, and `--if-rev` rejects a mutation prepared against
//! a stale read.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Priority;
use crate::store::{self, UpdateOptions};
use crate::{format, timeutil};

/// Edit a task
#[derive(Args)]
pub struct Edit {
    /// Task ID or ref
    pub id: String,
    /// New title
    #[usage(short = 't', long)]
    pub title: Option<String>,
    /// New priority
    #[usage(short = 'p', long)]
    pub priority: Option<String>,
    /// Labels (+add, or replace; use --remove-label to remove)
    #[usage(short = 'l', long, delimiter = ',')]
    pub labels: Vec<String>,
    /// Labels to remove (comma-separated)
    #[usage(long = "remove-label", delimiter = ',')]
    pub remove_labels: Vec<String>,
    /// Assignees (+add, or replace; use --remove-assignee to remove)
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
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let current = txn.load(&id).into_diagnostic()?;
        let mut u = UpdateOptions {
            title: self.title,
            expect_rev: self.if_rev,
            ..Default::default()
        };

        if let Some(p) = self.priority {
            u.priority = Some(Priority::parse(&p).into_diagnostic()?);
        }
        if !self.labels.is_empty() || !self.remove_labels.is_empty() {
            let mut ops = self.labels;
            ops.extend(self.remove_labels.iter().map(|l| format!("-{l}")));
            u.labels = Some(apply_slice_updates(&current.labels, &ops));
        }
        if !self.assignees.is_empty() || !self.remove_assignees.is_empty() {
            let mut ops = self.assignees;
            ops.extend(self.remove_assignees.iter().map(|a| format!("-{a}")));
            u.assignees = Some(apply_slice_updates(&current.assignees, &ops));
        }
        if let Some(d) = self.due {
            if d == "-" {
                u.due_date = Some(None);
            } else {
                let parsed = timeutil::parse_due_date(&d).into_diagnostic()?;
                u.due_date = Some(parsed);
            }
        }
        if let Some(p) = self.parent {
            if p == "-" {
                u.parent = Some(None);
            } else {
                let pid = txn.resolve(&p).into_diagnostic()?;
                store::validate_parent(txn.ctx(), &pid, &id).into_diagnostic()?;
                u.parent = Some(Some(pid));
            }
        }
        if let Some(d) = self.desc {
            u.description = Some(if d == "-" { None } else { Some(d) });
        }
        if let Some(e) = self.estimate {
            u.estimate = Some(if e == 0 { None } else { Some(e) });
        }

        let updated = txn.update(&id, u).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&updated));
        } else {
            println!("Updated {}: {}", updated.id, updated.task.title);
        }
        Ok(())
    }
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
    use super::apply_slice_updates;

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
}
