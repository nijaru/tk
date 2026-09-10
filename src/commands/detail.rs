//! Task detail commands: checkpoint, links, acceptance, evidence, archival.
//!
//! A checkpoint is one replaceable summary of where the work stands; the log
//! stays the history. Links point at research, decisions, and source locations
//! instead of copying them. Acceptance says what must be true to be done;
//! evidence records how that was verified.
//!
//! These commands write. `tk show` reads, and `show --json` carries every one of
//! these fields, so there is one place to look.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::TaskView;
use crate::ops::{self, ListEdit, ListField, Mutation};

fn rev_of(t: &TaskView) -> Option<String> {
    Some(t.rev.clone())
}

/// Replace the current checkpoint
#[derive(Args)]
pub struct Checkpoint {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Current result, blocker, next action, verification
    pub text: Vec<String>,
    /// Clear the checkpoint
    #[usage(long)]
    pub clear: bool,
    /// Reject the update unless the task still has this revision
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Checkpoint {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let text = self.text.join(" ");
        if self.clear && !text.trim().is_empty() {
            return Err(crate::output::invalid(
                "provide checkpoint text or --clear, not both",
            ));
        }
        if !self.clear && text.trim().is_empty() {
            return Err(crate::output::invalid(format!(
                "nothing to set: give checkpoint text or --clear (read it with 'tk show {}')",
                self.id
            )));
        }
        let m = Mutation::choose(&ctx.store, self.if_rev.is_some())?;
        let id = m.store().resolve(&self.id)?;
        let value = (!self.clear).then_some(text);
        let t = ops::set_checkpoint(&m, &id, value, self.if_rev.as_deref())?;
        let human = match &t.task.checkpoint {
            Some(c) => format!("Checkpoint for {}:\n{c}", t.task.alias),
            None => format!("Cleared the checkpoint for {}.", t.task.alias),
        };
        ctx.emit("checkpoint", &t, rev_of(&t), Vec::new(), || human);
        Ok(())
    }
}

/// Add document links to a task
///
/// A link is a document reference (a path or URL). Use `parent` or `block` for
/// relationships between tasks.
#[derive(Args)]
pub struct Link {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// References to add
    #[usage(required)]
    pub refs: Vec<String>,
}

impl RunWith<AppCtx> for Link {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let m = Mutation::free(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let t = ops::edit_list(&m, &id, ListField::Links, ListEdit::Add(&self.refs), None)?;
        let human = field_human(&t, ListField::Links, "Links");
        ctx.emit("link", &t, rev_of(&t), Vec::new(), || human);
        Ok(())
    }
}

/// Remove document links from a task
#[derive(Args)]
pub struct Unlink {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// References to remove
    #[usage(required)]
    pub refs: Vec<String>,
}

impl RunWith<AppCtx> for Unlink {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let m = Mutation::free(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let t = ops::edit_list(
            &m,
            &id,
            ListField::Links,
            ListEdit::Remove(&self.refs),
            None,
        )?;
        let human = field_human(&t, ListField::Links, "Links");
        ctx.emit("unlink", &t, rev_of(&t), Vec::new(), || human);
        Ok(())
    }
}

/// Operations on one list-valued detail field.
macro_rules! list_cmd {
    ($name:ident, $command:literal, $field:expr, $label:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Args)]
        pub struct $name {
            /// Task alias, ID, or ID prefix
            pub id: String,
            /// Values to add
            pub values: Vec<String>,
            /// Remove an exact value (repeatable)
            #[usage(long, value_name = "TEXT")]
            pub remove: Vec<String>,
            /// Remove all values
            #[usage(long)]
            pub clear: bool,
        }

        impl RunWith<AppCtx> for $name {
            type Output = miette::Result<()>;

            fn run_with(self, ctx: AppCtx) -> Self::Output {
                let field = $field;
                let label = $label;
                if !self.values.is_empty() && (!self.remove.is_empty() || self.clear) {
                    return Err(crate::output::invalid(
                        "add values or remove/clear them, not both",
                    ));
                }
                if self.values.is_empty() && self.remove.is_empty() && !self.clear {
                    return Err(crate::output::invalid(format!(
                        "nothing to do: give values, --remove, or --clear \
                         (read them with 'tk show {id}')",
                        id = self.id
                    )));
                }
                let m = Mutation::free(&ctx.store)?;
                let id = m.store().resolve(&self.id)?;
                let edit = if self.clear {
                    ListEdit::Clear
                } else if !self.remove.is_empty() {
                    ListEdit::Remove(&self.remove)
                } else {
                    ListEdit::Add(&self.values)
                };
                let t = ops::edit_list(&m, &id, field, edit, None)?;
                let human = field_human(&t, field, label);
                ctx.emit($command, &t, rev_of(&t), Vec::new(), || human);
                Ok(())
            }
        }
    };
}

list_cmd!(
    Accept,
    "accept",
    ListField::Acceptance,
    "Acceptance",
    "Add or change what must be true for this task to be done"
);
list_cmd!(
    Evidence,
    "evidence",
    ListField::Evidence,
    "Evidence",
    "Add or change how completion was verified"
);

/// Retire a done/closed task from active views without deleting it
#[derive(Args)]
pub struct Archive {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Reject the change unless the task still has this revision
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Archive {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let m = Mutation::locked(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let t = ops::archive(&m, &id, self.if_rev.as_deref())?;
        let human = format!("Archived {}.", t.task.alias);
        ctx.emit("archive", &t, rev_of(&t), Vec::new(), || human);
        Ok(())
    }
}

/// Return an archived task to active views
#[derive(Args)]
pub struct Unarchive {
    /// Task alias, ID, or ID prefix
    pub id: String,
}

impl RunWith<AppCtx> for Unarchive {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let m = Mutation::free(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let t = ops::unarchive(&m, &id)?;
        let human = format!("Unarchived {}.", t.task.alias);
        ctx.emit("unarchive", &t, rev_of(&t), Vec::new(), || human);
        Ok(())
    }
}

fn field_human(t: &TaskView, field: ListField, label: &str) -> String {
    let values = field.values(&t.task);
    if values.is_empty() {
        return format!("No {label} recorded for {}.", t.task.alias);
    }
    let mut out = format!("{label} ({}):", values.len());
    for value in values {
        out.push_str(&format!("\n  - {value}"));
    }
    out
}
