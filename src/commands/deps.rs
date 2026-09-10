//! `tk block` / `tk unblock`
//!
//! Existence and cycle checks run inside the same transaction as the write, so
//! a concurrent graph change cannot invalidate the validation.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;

/// Add a blocker dependency
#[derive(Args)]
pub struct Block {
    /// Task to block (alias, ID, or ID prefix)
    pub id: String,
    /// Blocking task (alias, ID, or ID prefix)
    pub blocker: String,
}

impl RunWith<AppCtx> for Block {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let blocker = txn.resolve(&self.blocker).into_diagnostic()?;
        let blocker_label = txn.alias_of(&blocker).unwrap_or_else(|| short(&blocker));
        if txn
            .load(&id)
            .into_diagnostic()?
            .state
            .blocked_by
            .iter()
            .any(|b| b == &blocker)
        {
            println!("Task {id} is already blocked by {blocker_label}");
            return Ok(());
        }
        let t = txn.add_blocker(&id, &blocker).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Blocked {} by {blocker_label}", t.task.alias);
        }
        Ok(())
    }
}

/// Remove a blocker dependency
#[derive(Args)]
pub struct Unblock {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Blocking task to remove
    pub blocker: String,
}

impl RunWith<AppCtx> for Unblock {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let blocker = txn.resolve(&self.blocker).into_diagnostic()?;
        let blocker_label = txn.alias_of(&blocker).unwrap_or_else(|| short(&blocker));
        let (t, found) = txn.remove_blocker(&id, &blocker).into_diagnostic()?;
        if !found {
            println!("Task {} is not blocked by {blocker_label}", t.task.alias);
            return Ok(());
        }
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Unblocked {} from {blocker_label}", t.task.alias);
        }
        Ok(())
    }
}

/// Display an ID compactly in human output.
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}
