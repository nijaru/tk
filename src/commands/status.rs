//! `tk start/open/defer/done/close` — status transitions.
//!
//! A status change is a status change: it records what state the task is in, and
//! nothing more. There is no claim and no ownership, so two people can both mark
//! a task active; use `checkpoint` to say what you are doing about it.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Status;
use crate::ops::{self, Mutation};

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
                let m = Mutation::free(&ctx.store)?;
                let id = m.store().resolve(&self.id)?;
                let t = ops::set_status(&m, &id, $status)?;
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
    "Mark a task as being worked on (open → active)"
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
