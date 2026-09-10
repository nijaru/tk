//! `tk edit`
//!
//! Single-value changes (title, priority, description) are last-writer-wins
//! appends and take no lock. Two cases need the lock, because the value depends
//! on a read rather than on the caller's intent: a bare label value *replaces*
//! the set, and `--if-rev` must compare and write with nothing in between.
//!
//! `+label` and `-label` are deltas and stay lock-free, so concurrent agents can
//! each add their own label without losing anyone's edit.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Priority;
use crate::ops::{self, ListEdit, ListField, Mutation};

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
    /// A leading `-` reads as a flag to the shell and this parser alike, so use
    /// `--remove-label`; `-l +x` and `--labels=-x` both work.
    #[usage(short = 'l', long, delimiter = ',')]
    pub labels: Vec<String>,
    /// Labels to remove (comma-separated)
    #[usage(long = "remove-label", delimiter = ',')]
    pub remove_labels: Vec<String>,
    /// Parent task (or - to clear)
    #[usage(long)]
    pub parent: Option<String>,
    /// Description (or - to clear)
    #[usage(short = 'd', long = "desc")]
    pub desc: Option<String>,
    /// Reject the edit unless the task still has this revision (`show --json`)
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Edit {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let replaces_labels = self.labels.iter().any(|v| !is_delta(v));
        // A parent change validates other records; a replacement reads this one.
        let needs_lock = self.if_rev.is_some() || self.parent.is_some() || replaces_labels;

        let m = Mutation::choose(&ctx.store, needs_lock)?;
        let id = m.store().resolve(&self.id)?;

        // One conditional check for the whole edit. Checking per append would
        // reject the second field of a legitimate multi-field edit, because the
        // first append already moved the revision.
        m.check_rev(&id, self.if_rev.as_deref())?;

        if let Some(title) = self.title {
            ops::set_title(&m, &id, title, None)?;
        }
        if let Some(priority) = &self.priority {
            ops::set_priority(&m, &id, Priority::parse(priority)?)?;
        }
        if let Some(desc) = self.desc {
            let value = (desc != "-").then_some(desc);
            ops::set_description(&m, &id, value, None)?;
        }
        if let Some(parent) = self.parent {
            if parent == "-" {
                ops::set_parent(&m, &id, None)?;
            } else {
                let pid = m.store().resolve(&parent)?;
                ops::set_parent(&m, &id, Some(&pid))?;
            }
        }

        let mut label_ops = self.labels;
        label_ops.extend(self.remove_labels.iter().map(|l| format!("-{l}")));
        let updated = ops::edit_list(
            &m,
            &id,
            ListField::Labels,
            ListEdit::Deltas(&label_ops),
            None,
        )?;

        let human = format!("Updated {}: {}", updated.task.alias, updated.task.title);
        ctx.emit(
            "edit",
            &updated,
            Some(updated.rev.clone()),
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// `+x` adds, `-x` removes, a bare value replaces the set.
fn is_delta(value: &str) -> bool {
    value.starts_with('+') || value.starts_with('-')
}
