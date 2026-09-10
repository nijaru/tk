//! `tk log`

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;

/// Add a log entry to a task
#[derive(Args)]
pub struct Log {
    /// Task ID or ref
    pub id: String,
    /// Log message
    pub msg: Vec<String>,
}

impl RunWith<AppCtx> for Log {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let msg = self.msg.join(" ");
        if msg.trim().is_empty() {
            return Err(miette::miette!("log message cannot be empty"));
        }
        // The append is applied to current state under the store lock.
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let t = txn.add_log(&id, &msg).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Logged to {}: {msg}", t.id);
        }
        Ok(())
    }
}
