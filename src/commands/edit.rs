//! Changing an entry: state, labels, blockers, the log, and the multi-field edit.
//!
//! Every one of these is a thin wrapper: resolve, call the operation, print the
//! resulting document. What a change *means* is in [`crate::ops`], which is why
//! `apply` cannot disagree with any of them.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::State;
use crate::ops::{self, Edit as EditFields};
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

/// Move a task to a state: open, done, or dropped
#[derive(Args, Debug)]
pub struct StateCmd {
    /// Ref, or part of a title
    pub r#ref: String,
    /// open, done, or dropped
    pub state: String,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl StateCmd {
    /// The one path every state change takes, so `done`, `drop`, and `open`
    /// cannot drift from each other.
    pub fn set(
        ctx: &AppCtx,
        command: &'static str,
        r#ref: String,
        raw: &str,
        rev: Option<String>,
    ) -> miette::Result<()> {
        ctx.require_store()?;
        let state = State::parse(raw)?;
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &r#ref, rev.as_deref())?;
        let view = ops::set_state(&txn, &r#ref, state, &now)?;
        drop(txn);
        emit_view(ctx, command, view);
        Ok(())
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
        StateCmd::set(&ctx, "done", self.r#ref, "done", self.if_rev)
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
        StateCmd::set(&ctx, "drop", self.r#ref, "dropped", self.if_rev)
    }
}

/// Reopen a done or dropped task
#[derive(Args, Debug)]
pub struct Open {
    /// Ref, or part of a title
    pub r#ref: String,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Open {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        StateCmd::set(&ctx, "open", self.r#ref, "open", self.if_rev)
    }
}

/// Add or remove labels: +add, -remove
///
/// Deltas only. Replacing a whole label set is `tk edit --label`, so a stray
/// `tk label a7b3 urgent` cannot quietly drop the labels someone else added.
#[derive(Args, Debug)]
pub struct Label {
    /// Ref, or part of a title
    pub r#ref: String,
    /// +add or -remove
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
        for change in &self.changes {
            if !change.starts_with(['+', '-']) {
                return Err(ctx
                    .fail(
                        "label",
                        crate::output::code::INVALID_INPUT,
                        &format!(
                            "labels change by delta: use +{change} or -{change} (tk edit --label replaces the set)"
                        ),
                        &serde_json::Value::Null,
                        Vec::new(),
                    ));
            }
        }
        let now = ctx.now();
        let txn = ctx.store.txn()?;
        let r#ref = target(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let view = ops::edit_labels(&txn, &r#ref, &self.changes, &now)?;
        drop(txn);
        emit_view(&ctx, "label", view);
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
    /// Replace the whole label set
    #[usage(short = 'l', long, delimiter = ',')]
    pub label: Vec<String>,
    /// Add labels
    #[usage(long = "add-label", delimiter = ',')]
    pub add_label: Vec<String>,
    /// Remove labels
    #[usage(long = "remove-label", delimiter = ',')]
    pub remove_label: Vec<String>,
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
            labels: (!self.label.is_empty()).then(|| self.label.clone()),
            add_labels: self.add_label.clone(),
            remove_labels: self.remove_label.clone(),
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
