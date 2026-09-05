//! `tk start/open/defer/done/close` — single-field status transitions.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::Status;
use crate::store;

use super::resolve;

macro_rules! status_cmd {
    ($name:ident, $status:expr, $verb:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Args)]
        pub struct $name {
            /// Task ID or ref
            pub id: String,
        }

        impl RunWith<AppCtx> for $name {
            type Output = miette::Result<()>;

            fn run_with(self, ctx: AppCtx) -> Self::Output {
                let id = resolve(&ctx, &self.id)?;
                let t = store::update_status(&ctx.store, &id, $status).into_diagnostic()?;
                if ctx.json {
                    println!("{}", format::format_json(&t));
                } else {
                    println!("{} {}: {}", $verb, t.id, t.task.title);
                }
                Ok(())
            }
        }
    };
}

status_cmd!(
    Start,
    Status::Active,
    "Started",
    "Start working on a task (open → active)"
);
status_cmd!(
    Open,
    Status::Open,
    "Set to open",
    "Reset a task status to open"
);
status_cmd!(Defer, Status::Deferred, "Deferred", "Defer a task");
status_cmd!(Done, Status::Done, "Completed", "Complete a task");
status_cmd!(Close, Status::Closed, "Closed", "Close/cancel a task");
