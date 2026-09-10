//! `tk start/open/defer/done/close` — single-field status transitions.
//!
//! A status change is last-writer-wins, so it needs no lock: two agents setting
//! a status at the same moment produce two events in some order, and the last
//! one is the status. `completed_at` is derived during the fold from the
//! transition itself, so it can never disagree with the status.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Status;
use crate::record::op;

use super::Writer;

macro_rules! status_cmd {
    ($name:ident, $command:literal, $status:expr, $verb:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Args)]
        pub struct $name {
            /// Task alias, ID, or ID prefix
            pub id: String,
        }

        impl RunWith<AppCtx> for $name {
            type Output = miette::Result<()>;

            fn run_with(self, ctx: AppCtx) -> Self::Output {
                let writer = Writer::new(&ctx, false)?;
                let id = writer.store().resolve(&self.id)?;
                writer.append(&id, op::STATUS, serde_json::json!($status), None)?;
                let t = writer.view(&id)?;
                let human = format!("{} {}: {}", $verb, t.task.alias, t.task.title);
                ctx.emit($command, &t, Some(t.rev.clone()), Vec::new(), || human);
                Ok(())
            }
        }
    };
}

status_cmd!(
    Start,
    "start",
    Status::Active,
    "Started",
    "Start working on a task (open → active)"
);
status_cmd!(
    Open,
    "open",
    Status::Open,
    "Set to open",
    "Reset a task status to open"
);
status_cmd!(Defer, "defer", Status::Deferred, "Deferred", "Defer a task");
status_cmd!(Done, "done", Status::Done, "Completed", "Complete a task");
status_cmd!(
    Close,
    "close",
    Status::Closed,
    "Closed",
    "Close/cancel a task"
);
