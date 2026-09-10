//! `tk block` / `tk unblock`
//!
//! Existence and cycle checks run inside the same transaction as the write, so
//! a concurrent graph change cannot invalidate the validation.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::store;

/// Add a blocker dependency
#[derive(Args)]
pub struct Block {
    /// Task ID or ref to block
    pub id: String,
    /// Blocking task ID or ref
    pub blocker: String,
}

impl RunWith<AppCtx> for Block {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let blocker = txn.resolve(&self.blocker).into_diagnostic()?;
        if id == blocker {
            return Err(miette::miette!("task cannot block itself"));
        }
        let existing = txn.load(&id).into_diagnostic()?;
        if existing.blocked_by.iter().any(|b| b == &blocker) {
            println!("Task {id} is already blocked by {blocker}");
            return Ok(());
        }
        if store::would_block_cycle(txn.ctx(), &id, &blocker) {
            return Err(miette::miette!("would create circular dependency"));
        }
        let t = txn.add_blocker(&id, &blocker).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Blocked {id} by {blocker}");
        }
        Ok(())
    }
}

/// Remove a blocker dependency
#[derive(Args)]
pub struct Unblock {
    /// Task ID or ref
    pub id: String,
    /// Blocking task ID or ref to remove
    pub blocker: String,
}

impl RunWith<AppCtx> for Unblock {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let blocker = txn.resolve(&self.blocker).into_diagnostic()?;
        let (t, found) = txn.remove_blocker(&id, &blocker).into_diagnostic()?;
        if !found {
            println!("Task {id} is not blocked by {blocker}");
            return Ok(());
        }
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Unblocked {id} from {blocker}");
        }
        Ok(())
    }
}
