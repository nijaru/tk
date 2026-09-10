//! `tk apply`

use usage::{Args, RunWith};

use crate::cli::AppCtx;

/// Apply a batch of intents from stdin under one lock
///
/// Every intent is resolved and validated before anything is written, so a
/// batch that is wrong is rejected whole. Intents run in order; see
/// `tk apply --help` in the README for the accepted `op` values.
#[derive(Args)]
pub struct Apply {
    /// Validate the batch and report what would change, without writing
    #[usage(long)]
    pub dry_run: bool,
}

impl RunWith<AppCtx> for Apply {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        crate::apply::run(&ctx, self.dry_run)
    }
}
