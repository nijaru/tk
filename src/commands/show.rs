//! `tk show`

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::store;

use super::resolve;

/// Show task details
#[derive(Args)]
pub struct Show {
    /// Task ID or ref
    pub id: String,
}

impl RunWith<AppCtx> for Show {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let id = resolve(&ctx, &self.id)?;
        let (t, cleanup) = store::get_task(&ctx.store, &id).into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("{}", format::format_task_detail(&t, ctx.color));
            if let Some(c) = cleanup
                && !c.is_empty()
            {
                println!("{}", format::warning(&c.message(&id), ctx.color));
            }
        }
        Ok(())
    }
}
