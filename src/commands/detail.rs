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
use crate::record::op;

use super::Writer;

/// Replace the current checkpoint (with no text, print it)
#[derive(Args)]
pub struct Checkpoint {
    /// Task alias, ID, or ID prefix
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
        let text = self.text.join(" ");
        if self.clear && !text.trim().is_empty() {
            return Err(miette::miette!(
                "provide checkpoint text or --clear, not both"
            ));
        }
        let writing = self.clear || !text.trim().is_empty();
        let writer = Writer::new(&ctx, self.if_rev.is_some())?;
        let id = writer.store().resolve(&self.id).into_diagnostic()?;
        let t = if writing {
            writer
                .store()
                .check_rev(&id, self.if_rev.as_deref())
                .into_diagnostic()?;
            let value = (!self.clear).then_some(text);
            writer.append(&id, op::CHECKPOINT, serde_json::json!(value), None)?;
            writer.view(&id)?
        } else {
            writer.view(&id)?
        };
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            match &t.task.checkpoint {
                Some(c) => println!("Checkpoint for {}:\n{c}", t.task.alias),
                None => println!("No checkpoint set for {}.", t.task.alias),
            }
        }
        Ok(())
    }
}

/// Add links to relevant research, decisions, or source locations
#[derive(Args)]
pub struct Link {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// References to add (paths, URLs, task IDs); with none, list them
    pub refs: Vec<String>,
}

impl RunWith<AppCtx> for Link {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id).into_diagnostic()?;
        let t = if self.refs.is_empty() {
            writer.view(&id)?
        } else {
            writer.append(&id, op::LINKS_ADD, serde_json::json!(self.refs), None)?;
            writer.view(&id)?
        };
        print_field(&t, Field::Links, "Links", ctx.json);
        Ok(())
    }
}

/// Remove links from a task
#[derive(Args)]
pub struct Unlink {
    /// Task alias, ID, or ID prefix
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
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id).into_diagnostic()?;
        writer.append(&id, op::LINKS_REMOVE, serde_json::json!(self.refs), None)?;
        print_field(&writer.view(&id)?, Field::Links, "Links", ctx.json);
        Ok(())
    }
}

/// Operations on one list-valued detail field.
macro_rules! list_cmd {
    ($name:ident, $field:expr, $label:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Args)]
        pub struct $name {
            /// Task alias, ID, or ID prefix
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
                let field = $field;
                let label = $label;
                if !self.values.is_empty() && (!self.remove.is_empty() || self.clear) {
                    return Err(miette::miette!("add values or remove/clear them, not both"));
                }
                let writer = Writer::new(&ctx, false)?;
                let id = writer.store().resolve(&self.id).into_diagnostic()?;
                let t = if self.clear {
                    writer.append(&id, field.set_op(), serde_json::json!([]), None)?;
                    writer.view(&id)?
                } else if !self.remove.is_empty() {
                    writer.append(&id, field.remove_op(), serde_json::json!(self.remove), None)?;
                    writer.view(&id)?
                } else if !self.values.is_empty() {
                    writer.append(&id, field.add_op(), serde_json::json!(self.values), None)?;
                    writer.view(&id)?
                } else {
                    writer.view(&id)?
                };
                print_field(&t, field, label, ctx.json);
                Ok(())
            }
        }
    };
}

list_cmd!(
    Accept,
    Field::Acceptance,
    "Acceptance",
    "Add or show what must be true for this task to be done"
);
list_cmd!(
    Evidence,
    Field::Evidence,
    "Evidence",
    "Add or show how completion was verified"
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
        // Locked: the "only terminal tasks archive" rule is checked against the
        // record, and a concurrent reopen must not slip between check and write.
        ctx.require_store()?;
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        txn.check_rev(&id, self.if_rev.as_deref())
            .into_diagnostic()?;
        let record = txn.load(&id).into_diagnostic()?;
        if !record.state.status.is_terminal() {
            return Err(miette::miette!(
                "only done or closed tasks can be archived ({} is {})",
                record.state.alias,
                record.state.status
            ));
        }
        let t = if record.state.is_archived() {
            txn.view_of(&id).into_diagnostic()?
        } else {
            txn.append(&id, op::ARCHIVED, serde_json::Value::Null)
                .into_diagnostic()?;
            txn.view_of(&id).into_diagnostic()?
        };
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Archived {}.", t.task.alias);
        }
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
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id).into_diagnostic()?;
        let t = writer.view(&id)?;
        if t.task.is_archived() {
            writer.append(&id, op::UNARCHIVED, serde_json::Value::Null, None)?;
        }
        let t = writer.view(&id)?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Unarchived {}.", t.task.alias);
        }
        Ok(())
    }
}

/// One list-valued detail field.
#[derive(Clone, Copy)]
pub enum Field {
    Links,
    Acceptance,
    Evidence,
}

impl Field {
    fn values(self, task: &crate::model::TaskState) -> &[String] {
        match self {
            Self::Links => &task.links,
            Self::Acceptance => &task.acceptance,
            Self::Evidence => &task.evidence,
        }
    }
    fn add_op(self) -> &'static str {
        match self {
            Self::Links => op::LINKS_ADD,
            Self::Acceptance => op::ACCEPTANCE_ADD,
            Self::Evidence => op::EVIDENCE_ADD,
        }
    }
    fn remove_op(self) -> &'static str {
        match self {
            Self::Links => op::LINKS_REMOVE,
            Self::Acceptance => op::ACCEPTANCE_REMOVE,
            Self::Evidence => op::EVIDENCE_REMOVE,
        }
    }
    fn set_op(self) -> &'static str {
        match self {
            Self::Links => op::LINKS_SET,
            Self::Acceptance => op::ACCEPTANCE_SET,
            Self::Evidence => op::EVIDENCE_SET,
        }
    }
}

fn print_field(t: &TaskView, field: Field, label: &str, json: bool) {
    if json {
        println!("{}", format::format_json(t));
        return;
    }
    let values = field.values(&t.task);
    if values.is_empty() {
        println!("No {label} recorded for {}.", t.task.alias);
        return;
    }
    println!("{label} ({}):", values.len());
    for value in values {
        println!("  - {value}");
    }
}
