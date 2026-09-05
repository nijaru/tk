//! `tk block` / `tk unblock`

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::store;

use super::resolve;

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
        let id = resolve(&ctx, &self.id)?;
        let blocker = resolve(&ctx, &self.blocker)?;
        if id == blocker {
            return Err(miette::miette!("task cannot block itself"));
        }
        let existing = store::read_task(&ctx.store, &id).into_diagnostic()?;
        if existing.blocked_by.iter().any(|b| b == &blocker) {
            println!("Task {id} is already blocked by {blocker}");
            return Ok(());
        }
        if store::would_block_cycle(&ctx.store, &id, &blocker) {
            return Err(miette::miette!("would create circular dependency"));
        }
        let t = store::add_blocker(&ctx.store, &id, &blocker).into_diagnostic()?;
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
        let id = resolve(&ctx, &self.id)?;
        let blocker = resolve(&ctx, &self.blocker)?;
        let (t, found) = store::remove_blocker(&ctx.store, &id, &blocker).into_diagnostic()?;
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
