//! `tk log`

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::ops::{self, Mutation};

/// Append a log entry to a task
///
/// The log is history: entries are never edited or removed. Use `checkpoint`
/// for the replaceable "where things stand" summary.
#[derive(Args)]
pub struct Log {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Log message
    pub msg: Vec<String>,
}

impl RunWith<AppCtx> for Log {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let msg = self.msg.join(" ");
        if msg.trim().is_empty() {
            return Err(crate::output::invalid("log message cannot be empty"));
        }
        // An append is commutative and cannot lose a concurrent writer's entry,
        // so no lock: eight agents can log to one task at once.
        let m = Mutation::free(&ctx.store)?;
        let id = m.store().resolve(&self.id)?;
        let t = ops::add_log(&m, &id, &msg)?;
        let human = format!("Logged to {}: {msg}", t.task.alias);
        ctx.emit("log", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}
