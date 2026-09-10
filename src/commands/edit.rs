//! Changing an entry: state, status, labels, criteria, blockers, the log, and
//! the multi-field edit.
//!
//! Every one of these is a thin wrapper: resolve, call the operation, print the
//! resulting document. What a change *means* is in [`crate::ops`], which is why
//! `apply` cannot disagree with any of them.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::State as EntryState;
use crate::ops::{self, AcceptanceChange, Edit as EditFields};
use crate::store::{Result, Txn};

/// Resolve a ref, then refuse if the caller read an older revision.
///
/// Inside the transaction, so nothing can change between the check and the
/// write it protects.
fn target(txn: &Txn<'_>, input: &str, rev: Option<&str>) -> Result<String> {
    ops::resolve_rev(txn, input, rev)
}

/// Print the entry a change produced.
fn emit_view(ctx: &AppCtx, command: &'static str, view: crate::model::EntryView) {
    let rev = Some(view.rev.clone());
    ctx.emit(command, &view, rev, Vec::new(), || {
        format::render_summary(&view)
    });
}

/// Add a log entry to a task
#[derive(Args, Debug)]
pub struct Note {
    /// Ref, or part of a title
    pub r#ref: String,
    /// What happened, in one line
    pub message: Vec<String>,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Note {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let now = ctx.now();
        let message = self.message.join(" ");
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let view = ops::add_log(&txn, &r#ref, &message, &now)?;
        drop(txn);
        emit_view(&ctx, "note", view);
        Ok(())
    }
}

/// Replace the current status (the summary, not the log)
#[derive(Args, Debug)]
pub struct Status {
    /// Ref, or part of a title
    pub r#ref: String,
    /// Where things stand, in one line
    pub text: Vec<String>,
    /// Remove the status
    #[usage(long)]
    pub clear: bool,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Status {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let now = ctx.now();
        let text = self.text.join(" ");
        if text.is_empty() && !self.clear {
            return Err(ctx.fail(
                "status",
                crate::output::code::INVALID_INPUT,
                "give the status text, or --clear to remove it",
                &serde_json::Value::Null,
                Vec::new(),
            ));
        }
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let text = if self.clear {
            None
        } else {
            Some(text.as_str())
        };
        let view = ops::set_status(&txn, &r#ref, text, &now)?;
        drop(txn);
        emit_view(&ctx, "status", view);
        Ok(())
    }
}

/// Move a task to a state: open, done, or dropped
#[derive(Args, Debug)]
pub struct State {
    /// Ref, or part of a title
    pub r#ref: String,
    /// open, done, or dropped
    pub state: String,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl State {
    pub fn set(
        ctx: &AppCtx,
        command: &'static str,
        r#ref: String,
        raw: &str,
        rev: Option<String>,
    ) -> miette::Result<()> {
        ctx.require_store()?;
        let state = EntryState::parse(raw)?;
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &r#ref, rev.as_deref())?;
        let view = ops::set_state(&txn, &r#ref, state, &now)?;
        drop(txn);
        emit_view(ctx, command, view);
        Ok(())
    }
}

impl RunWith<AppCtx> for State {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        State::set(&ctx, "state", self.r#ref, &self.state, self.if_rev)
    }
}

/// Mark a task done
#[derive(Args, Debug)]
pub struct Done {
    /// Ref, or part of a title
    pub r#ref: String,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Done {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        State::set(&ctx, "done", self.r#ref, "done", self.if_rev)
    }
}

/// Drop a task without doing it
#[derive(Args, Debug)]
pub struct Drop {
    /// Ref, or part of a title
    pub r#ref: String,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Drop {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        State::set(&ctx, "drop", self.r#ref, "dropped", self.if_rev)
    }
}

/// Change a task's labels
#[derive(Args, Debug)]
pub struct Label {
    /// Ref, or part of a title
    pub r#ref: String,
    /// +add, -remove, or a bare label to replace the whole set
    #[usage(required)]
    pub changes: Vec<String>,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Label {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let view = ops::edit_labels(&txn, &r#ref, &self.changes, &now)?;
        drop(txn);
        emit_view(&ctx, "label", view);
        Ok(())
    }
}

/// Add or change what must be true for this task to be done
#[derive(Args, Debug)]
pub struct Accept {
    /// Ref, or part of a title
    pub r#ref: String,
    /// Criteria to add
    pub criteria: Vec<String>,
    /// Replace the whole list
    #[usage(long, delimiter = ',')]
    pub set: Vec<String>,
    /// Remove a criterion
    #[usage(long, value_name = "TEXT")]
    pub remove: Vec<String>,
    /// Remove every criterion
    #[usage(long)]
    pub clear: bool,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Accept {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let change = AcceptanceChange {
            set: (!self.set.is_empty()).then(|| self.set.clone()),
            add: self.criteria.clone(),
            remove: self.remove.clone(),
            clear: self.clear,
        };
        if change.is_empty() {
            return Err(ctx.fail(
                "accept",
                crate::output::code::INVALID_INPUT,
                "give a criterion, --set, --remove, or --clear",
                &serde_json::Value::Null,
                Vec::new(),
            ));
        }
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let view = ops::edit_acceptance(&txn, &r#ref, &change, &now)?;
        drop(txn);
        emit_view(&ctx, "accept", view);
        Ok(())
    }
}

/// Add a blocker dependency
#[derive(Args, Debug)]
pub struct Block {
    /// Ref, or part of a title
    pub r#ref: String,
    /// What it waits on: one or more refs or title fragments
    #[usage(required)]
    pub blocker: Vec<String>,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Block {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let mut view = None;
        for blocker in &self.blocker {
            view = Some(ops::add_blocker(&txn, &r#ref, blocker, &now)?);
        }
        drop(txn);
        if let Some(view) = view {
            emit_view(&ctx, "block", view);
        }
        Ok(())
    }
}

/// Remove a blocker dependency
#[derive(Args, Debug)]
pub struct Unblock {
    /// Ref, or part of a title
    pub r#ref: String,
    /// What it no longer waits on
    #[usage(required)]
    pub blocker: Vec<String>,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Unblock {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let mut view = None;
        for blocker in &self.blocker {
            view = Some(ops::remove_blocker(&txn, &r#ref, blocker, &now)?);
        }
        drop(txn);
        if let Some(view) = view {
            emit_view(&ctx, "unblock", view);
        }
        Ok(())
    }
}

/// Edit a task's fields in one write
#[derive(Args, Debug)]
pub struct Edit {
    /// Ref, or part of a title
    pub r#ref: String,
    /// New title
    #[usage(short = 't', long)]
    pub title: Option<String>,
    /// Move the file to a new slug (the ref never changes)
    #[usage(long)]
    pub slug: Option<String>,
    /// Replace the status
    #[usage(long)]
    pub status: Option<String>,
    /// Remove the status
    #[usage(long = "clear-status")]
    pub clear_status: bool,
    /// Replace the whole label set
    #[usage(short = 'l', long, delimiter = ',')]
    pub label: Vec<String>,
    /// Add labels
    #[usage(long = "add-label", delimiter = ',')]
    pub add_label: Vec<String>,
    /// Remove labels
    #[usage(long = "remove-label", delimiter = ',')]
    pub remove_label: Vec<String>,
    /// Replace the acceptance criteria
    #[usage(long, delimiter = ',')]
    pub accept: Vec<String>,
    /// Add acceptance criteria
    #[usage(long = "add-accept", delimiter = ',')]
    pub add_accept: Vec<String>,
    /// Remove acceptance criteria
    #[usage(long = "remove-accept", value_name = "TEXT")]
    pub remove_accept: Vec<String>,
    /// Remove every acceptance criterion
    #[usage(long = "clear-accept")]
    pub clear_accept: bool,
    /// Replace the blockers
    #[usage(short = 'b', long = "blocked-by", value_name = "REF")]
    pub blocked_by: Vec<String>,
    /// Add one blocker
    #[usage(long = "add-block", value_name = "REF")]
    pub add_block: Vec<String>,
    /// Remove one blocker
    #[usage(long = "remove-block", value_name = "REF")]
    pub remove_block: Vec<String>,
    /// Add a log entry along with the rest
    #[usage(short = 'n', long)]
    pub note: Option<String>,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl Edit {
    pub fn fields(&self) -> EditFields {
        EditFields {
            title: self.title.clone(),
            slug: self.slug.clone(),
            status: match (self.clear_status, &self.status) {
                (true, _) => Some(None),
                (false, Some(text)) => Some(Some(text.clone())),
                (false, None) => None,
            },
            labels: (!self.label.is_empty()).then(|| self.label.clone()),
            add_labels: self.add_label.clone(),
            remove_labels: self.remove_label.clone(),
            acceptance: AcceptanceChange {
                set: (!self.accept.is_empty()).then(|| self.accept.clone()),
                add: self.add_accept.clone(),
                remove: self.remove_accept.clone(),
                clear: self.clear_accept,
            },
            blockers: (!self.blocked_by.is_empty()).then(|| self.blocked_by.clone()),
            add_blockers: self.add_block.clone(),
            remove_blockers: self.remove_block.clone(),
            note: self.note.clone(),
        }
    }
}

impl RunWith<AppCtx> for Edit {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let fields = self.fields();
        if fields.is_empty() {
            return Err(ctx.fail(
                "edit",
                crate::output::code::INVALID_INPUT,
                "nothing to change: pass a field, or see 'tk edit --help'",
                &serde_json::Value::Null,
                Vec::new(),
            ));
        }
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let view = ops::apply_edit(&txn, &r#ref, &fields, &now)?;
        drop(txn);
        emit_view(&ctx, "edit", view);
        Ok(())
    }
}
