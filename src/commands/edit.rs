//! `tk edit`
//!
//! Single-value changes (title, priority, description) are last-writer-wins
//! appends and take no lock. Two cases need the lock, because the value depends
//! on a read rather than on the caller's intent: a bare label value *replaces*
//! the set, and `--if-rev` must compare and write with nothing in between.
//!
//! `+label` and `-label` are deltas and stay lock-free, so concurrent agents can
//! each add their own label without losing anyone's edit.
//!
//! The edit itself is applied by [`ops::apply_edit`], which is also what
//! `tk apply` calls, so the two cannot disagree about fields or order.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Priority;
use crate::ops::{self, Mutation};

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
        let edit = ops::Edit {
            title: self.title,
            description: self.desc.map(clearable),
            priority: self.priority.as_deref().map(Priority::parse).transpose()?,
            parent: self.parent.map(clearable),
            labels: self.labels,
            remove_labels: self.remove_labels,
            if_rev: self.if_rev,
        };
        let m = Mutation::choose(&ctx.store, edit.needs_lock())?;
        let id = m.store().resolve(&self.id)?;
        let updated = ops::apply_edit(&m, &id, &edit)?;
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

/// `-` is how the CLI says "clear this".
fn clearable(value: String) -> Option<String> {
    (value != "-").then_some(value)
}
