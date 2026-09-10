//! Task detail commands: checkpoint, links, acceptance, evidence, archival.
//!
//! A checkpoint is one replaceable summary of where the work stands; the log
//! stays the history. Links point at research, decisions, and source locations
//! instead of copying them. Acceptance says what must be true to be done;
//! evidence records how that was verified.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::TaskView;
use crate::store::{ListField, ListOp};

/// Replace the current checkpoint (with no text, print it)
#[derive(Args)]
pub struct Checkpoint {
    /// Task ID or ref
    pub id: String,
    /// Checkpoint text: current result, blocker, next action, verification
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
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let text = self.text.join(" ");
        if self.clear && !text.trim().is_empty() {
            return Err(miette::miette!(
                "provide checkpoint text or --clear, not both"
            ));
        }
        let t = if self.clear || !text.trim().is_empty() {
            txn.set_checkpoint(&id, (!self.clear).then_some(text), self.if_rev.as_deref())
                .into_diagnostic()?
        } else {
            txn.view(&txn.load(&id).into_diagnostic()?)
        };
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            match &t.task.checkpoint {
                Some(c) => println!("Checkpoint for {}:\n{c}", t.id),
                None => println!("No checkpoint set for {}.", t.id),
            }
        }
        Ok(())
    }
}

/// Add links to relevant research, decisions, or source locations
#[derive(Args)]
pub struct Link {
    /// Task ID or ref
    pub id: String,
    /// References to add (paths, URLs, task IDs); with none, list them
    pub refs: Vec<String>,
}

impl RunWith<AppCtx> for Link {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let t = if self.refs.is_empty() {
            txn.view(&txn.load(&id).into_diagnostic()?)
        } else {
            txn.edit_list(&id, ListField::Links, ListOp::Add(self.refs), None)
                .into_diagnostic()?
        };
        print_field(&t, ListField::Links, "Links", ctx.json);
        Ok(())
    }
}

/// Remove links from a task
#[derive(Args)]
pub struct Unlink {
    /// Task ID or ref
    pub id: String,
    /// References to remove
    pub refs: Vec<String>,
}

impl RunWith<AppCtx> for Unlink {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if self.refs.is_empty() {
            return Err(miette::miette!("provide at least one reference to remove"));
        }
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let t = txn
            .edit_list(&id, ListField::Links, ListOp::Remove(self.refs), None)
            .into_diagnostic()?;
        print_field(&t, ListField::Links, "Links", ctx.json);
        Ok(())
    }
}

/// Operations on one list-valued detail field.
macro_rules! list_cmd {
    ($name:ident, $field:expr, $label:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Args)]
        pub struct $name {
            /// Task ID or ref
            pub id: String,
            /// Values to add; with none and no flags, list them
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
                let txn = ctx.store.txn().into_diagnostic()?;
                let id = txn.resolve(&self.id).into_diagnostic()?;
                let field = $field;
                let label = $label;
                if !self.values.is_empty() && (!self.remove.is_empty() || self.clear) {
                    return Err(miette::miette!("add values or remove/clear them, not both"));
                }
                let t = if self.clear {
                    txn.edit_list(&id, field, ListOp::Clear, None)
                        .into_diagnostic()?
                } else if !self.remove.is_empty() {
                    txn.edit_list(&id, field, ListOp::Remove(self.remove), None)
                        .into_diagnostic()?
                } else if !self.values.is_empty() {
                    txn.edit_list(&id, field, ListOp::Add(self.values), None)
                        .into_diagnostic()?
                } else {
                    txn.view(&txn.load(&id).into_diagnostic()?)
                };
                print_field(&t, field, label, ctx.json);
                Ok(())
            }
        }
    };
}

list_cmd!(
    Accept,
    ListField::Acceptance,
    "Acceptance",
    "Add or show what must be true for this task to be done"
);
list_cmd!(
    Evidence,
    ListField::Evidence,
    "Evidence",
    "Add or show how completion was verified"
);

/// Retire a done/closed task from active views without deleting it
#[derive(Args)]
pub struct Archive {
    /// Task ID or ref
    pub id: String,
    /// Reject the change unless the task still has this revision
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Archive {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let t = txn.archive(&id, self.if_rev.as_deref()).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Archived {}.", t.id);
        }
        Ok(())
    }
}

/// Return an archived task to active views
#[derive(Args)]
pub struct Unarchive {
    /// Task ID or ref
    pub id: String,
}

impl RunWith<AppCtx> for Unarchive {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let t = txn.unarchive(&id).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Unarchived {}.", t.id);
        }
        Ok(())
    }
}

fn print_field(t: &TaskView, field: ListField, label: &str, json: bool) {
    if json {
        println!("{}", format::format_json(t));
        return;
    }
    let values = field.values(&t.task);
    if values.is_empty() {
        println!("No {label} recorded for {}.", t.id);
        return;
    }
    println!("{label} ({}):", values.len());
    for value in values {
        println!("  - {value}");
    }
}
