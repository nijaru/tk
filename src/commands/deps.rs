//! `tk block` / `tk unblock` — the blocking graph.
//!
//! Existence and cycle checks run inside the same transaction as the write, so
//! a concurrent graph change cannot invalidate the validation. `tk ready` is the
//! reason this graph exists.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::ops::{self, Mutation};

/// Shorten an unresolvable reference for human output.
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Record that a task is blocked by another
#[derive(Args)]
pub struct Block {
    /// Task to block (alias, ID, or ID prefix)
    pub id: String,
    /// Blocking task
    pub blocker: String,
}

impl RunWith<AppCtx> for Block {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let m = Mutation::locked(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let blocker = m.store().resolve(&self.blocker)?;
        let label = m
            .store()
            .alias_of(&blocker)
            .unwrap_or_else(|| short(&blocker));
        if m.load(&id)?.state.blocked_by.contains(&blocker) {
            let human = format!("Task {id} is already blocked by {label}");
            ctx.emit("block", &serde_json::Value::Null, None, Vec::new(), || {
                human
            });
            return Ok(());
        }
        let t = ops::add_blocker(&m, &id, &blocker)?;
        let human = format!("Blocked {} by {label}", t.task.alias);
        ctx.emit("block", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}

/// Remove a blocking dependency
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
        let m = Mutation::locked(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let blocker = m.store().resolve(&self.blocker)?;
        let label = m
            .store()
            .alias_of(&blocker)
            .unwrap_or_else(|| short(&blocker));
        let (t, found) = ops::remove_blocker(&m, &id, &blocker)?;
        let human = if found {
            format!("Unblocked {} from {label}", t.task.alias)
        } else {
            format!("Task {} is not blocked by {label}", t.task.alias)
        };
        ctx.emit("unblock", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}
