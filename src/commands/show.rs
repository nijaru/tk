//! `tk show`
//!
//! Read-only: it reports inconsistencies and never repairs them. `tk recover`
//! drops a torn tail, and `tk purge --scrub` removes dangling references.

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::TaskView;
use crate::store;

use super::resolve;

/// Show task details
#[derive(Args)]
pub struct Show {
    /// Task alias, ID, or ID prefix
    pub id: String,
}

#[derive(serde::Serialize)]
struct ShowOutput<'a> {
    #[serde(flatten)]
    view: &'a TaskView,
    issues: &'a [String],
}

impl RunWith<AppCtx> for Show {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let id = resolve(&ctx, &self.id)?;
        let (t, issues) = store::get_task(&ctx.store, &id).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&ShowOutput {
                    view: &t,
                    issues: &issues,
                })
            );
        } else {
            println!("{}", format::format_task_detail(&t, ctx.color));
            for issue in &issues {
                println!("{}", format::warning(issue, ctx.color));
            }
            if !issues.is_empty() {
                println!(
                    "{}",
                    format::warning(
                        "Run 'tk check' for the whole store, or 'tk recover <id>' to drop a torn line.",
                        ctx.color
                    )
                );
            }
        }
        Ok(())
    }
}
