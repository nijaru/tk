//! Task detail commands: checkpoint, links, acceptance, evidence, archival.
//!
//! A checkpoint is one replaceable summary of where the work stands; the log
//! stays the history. Links point at research, decisions, and source locations
//! instead of copying them. Acceptance says what must be true to be done;
//! evidence records how that was verified.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::TaskView;
use crate::record::op;

use super::Writer;

fn rev_of(t: &TaskView) -> Option<String> {
    Some(t.rev.clone())
}

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
            return Err(crate::output::invalid(
                "provide checkpoint text or --clear, not both",
            ));
        }
        let writing = self.clear || !text.trim().is_empty();
        let writer = Writer::new(&ctx, self.if_rev.is_some())?;
        let id = writer.store().resolve(&self.id)?;
        if writing {
            writer.store().check_rev(&id, self.if_rev.as_deref())?;
            let value = (!self.clear).then_some(text);
            writer.append(&id, op::CHECKPOINT, serde_json::json!(value), None)?;
        }
        let t = writer.view(&id)?;
        let human = match &t.task.checkpoint {
            Some(c) => format!("Checkpoint for {}:\n{c}", t.task.alias),
            None => format!("No checkpoint set for {}.", t.task.alias),
        };
        ctx.emit("checkpoint", &t, rev_of(&t), Vec::new(), || human);
        Ok(())
    }
}

/// Add links to relevant research, decisions, or source locations
///
/// A link is a document reference. Use `tk relate` for another task.
#[derive(Args)]
pub struct Link {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// References to add (paths, URLs); with none, list them
    pub refs: Vec<String>,
}

impl RunWith<AppCtx> for Link {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id)?;
        if !self.refs.is_empty() {
            writer.append(&id, op::LINKS_ADD, serde_json::json!(self.refs), None)?;
        }
        let t = writer.view(&id)?;
        let human = field_human(&t, Field::Links, "Links");
        ctx.emit("link", &t, rev_of(&t), Vec::new(), || human);
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
            return Err(crate::output::invalid(
                "provide at least one reference to remove",
            ));
        }
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id)?;
        writer.append(&id, op::LINKS_REMOVE, serde_json::json!(self.refs), None)?;
        let t = writer.view(&id)?;
        let human = field_human(&t, Field::Links, "Links");
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
                    return Err(crate::output::invalid(
                        "add values or remove/clear them, not both",
                    ));
                }
                let writer = Writer::new(&ctx, false)?;
                let id = writer.store().resolve(&self.id)?;
                if self.clear {
                    writer.append(&id, field.set_op(), serde_json::json!([]), None)?;
                } else if !self.remove.is_empty() {
                    writer.append(&id, field.remove_op(), serde_json::json!(self.remove), None)?;
                } else if !self.values.is_empty() {
                    writer.append(&id, field.add_op(), serde_json::json!(self.values), None)?;
                }
                let t = writer.view(&id)?;
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
    Field::Acceptance,
    "Acceptance",
    "Add or show what must be true for this task to be done"
);
list_cmd!(
    Evidence,
    "evidence",
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
        let txn = ctx.store.txn()?;
        let id = txn.resolve(&self.id)?;
        txn.check_rev(&id, self.if_rev.as_deref())?;
        let record = txn.load(&id)?;
        if !record.state.status.is_terminal() {
            return Err(crate::output::invalid(format!(
                "only done or closed tasks can be archived ({} is {})",
                record.state.alias, record.state.status
            )));
        }
        if !record.state.is_archived() {
            txn.append(&id, op::ARCHIVED, serde_json::Value::Null)?;
        }
        let t = txn.view_of(&id)?;
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
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id)?;
        let t = writer.view(&id)?;
        if t.task.is_archived() {
            writer.append(&id, op::UNARCHIVED, serde_json::Value::Null, None)?;
        }
        let t = writer.view(&id)?;
        let human = format!("Unarchived {}.", t.task.alias);
        ctx.emit("unarchive", &t, rev_of(&t), Vec::new(), || human);
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

fn field_human(t: &TaskView, field: Field, label: &str) -> String {
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
