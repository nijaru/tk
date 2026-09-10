//! `tk block` / `tk unblock` / `tk relate` / `tk unrelate`
//!
//! Existence and cycle checks run inside the same transaction as the write, so
//! a concurrent graph change cannot invalidate the validation.

use usage::{Args, RunWith};

use crate::cli::AppCtx;

/// Shorten an unresolvable reference for human output.
fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

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
        let txn = ctx.store.txn()?;
        let id = txn.resolve(&self.id)?;
        let blocker = txn.resolve(&self.blocker)?;
        let label = txn.alias_of(&blocker).unwrap_or_else(|| short(&blocker));
        if txn
            .load(&id)?
            .state
            .blocked_by
            .iter()
            .any(|b| b == &blocker)
        {
            let human = format!("Task {id} is already blocked by {label}");
            ctx.emit("block", &serde_json::Value::Null, None, Vec::new(), || {
                human
            });
            return Ok(());
        }
        let t = txn.add_blocker(&id, &blocker)?;
        let human = format!("Blocked {} by {label}", t.task.alias);
        ctx.emit("block", &t, Some(t.rev.clone()), Vec::new(), || human);
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
        let txn = ctx.store.txn()?;
        let id = txn.resolve(&self.id)?;
        let blocker = txn.resolve(&self.blocker)?;
        let label = txn.alias_of(&blocker).unwrap_or_else(|| short(&blocker));
        let (t, found) = txn.remove_blocker(&id, &blocker)?;
        let human = if found {
            format!("Unblocked {} from {label}", t.task.alias)
        } else {
            format!("Task {} is not blocked by {label}", t.task.alias)
        };
        ctx.emit("unblock", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}

/// Record a non-blocking relationship with another task
///
/// `relate` is a "see also", not a constraint: it never blocks work and is
/// never cycle-checked. Documents belong in `tk link`; this is for tasks.
#[derive(Args)]
pub struct Relate {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Task to relate to
    pub other: String,
}

impl RunWith<AppCtx> for Relate {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn()?;
        let id = txn.resolve(&self.id)?;
        let other = txn.resolve(&self.other)?;
        let label = txn.alias_of(&other).unwrap_or_else(|| short(&other));
        let t = txn.add_related(&id, &other)?;
        let human = format!("Related {} to {label}", t.task.alias);
        ctx.emit("relate", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}

/// Remove a non-blocking relationship
#[derive(Args)]
pub struct Unrelate {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Task to unrelate
    pub other: String,
}

impl RunWith<AppCtx> for Unrelate {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn()?;
        let id = txn.resolve(&self.id)?;
        let other = txn.resolve(&self.other)?;
        let label = txn.alias_of(&other).unwrap_or_else(|| short(&other));
        let (t, found) = txn.remove_related(&id, &other)?;
        let human = if found {
            format!("Unrelated {} from {label}", t.task.alias)
        } else {
            format!("{} is not related to {label}", t.task.alias)
        };
        ctx.emit("unrelate", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}
