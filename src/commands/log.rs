//! `tk log`

use miette::IntoDiagnostic;

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::LogEntry;
use crate::record::op;

use super::Writer;

/// Add a log entry to a task
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
        // Appending is commutative and cannot lose a concurrent writer's entry,
        // so eight agents can log to one task at once without a lock.
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.id)?;
        let entry = LogEntry {
            ts: String::new(),
            msg: msg.clone(),
        };
        writer.append(
            &id,
            op::LOG,
            serde_json::to_value(&entry).into_diagnostic()?,
            None,
        )?;
        let t = writer.view(&id)?;
        let human = format!("Logged to {}: {msg}", t.task.alias);
        ctx.emit("log", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}
