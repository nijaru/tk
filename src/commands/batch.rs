//! `tk apply`: many intents in one call.

use std::io::Read as _;

use usage::{Args, RunWith};

use crate::apply::{self, Batch};
use crate::cli::AppCtx;

/// Apply a batch of intents from stdin under one lock
#[derive(Args, Debug)]
pub struct Apply {
    /// Read the batch from a file instead of stdin
    #[usage(short = 'f', long, value_name = "PATH")]
    pub file: Option<String>,
    /// Report what would change, and change nothing
    #[usage(long)]
    pub dry_run: bool,
}

impl Apply {
    fn read(&self) -> miette::Result<String> {
        match &self.file {
            Some(path) => std::fs::read_to_string(path)
                .map_err(|e| miette::miette!("could not read {path}: {e}")),
            None => {
                let mut input = String::new();
                std::io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|e| miette::miette!("could not read stdin: {e}"))?;
                Ok(input)
            }
        }
    }
}

impl RunWith<AppCtx> for Apply {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let batch = Batch::parse(&self.read()?)?;
        let now = ctx.now();
        // One transaction, one lock, for the whole batch: the intents see each
        // other's writes, which is what makes cumulative blocker checks correct.
        let report = {
            let txn = ctx.store.txn()?;
            apply::run(&txn, &batch, self.dry_run, &now)?
        };

        let mut issues = Vec::new();
        if let Some(failure) = &report.failed {
            issues.push(format!(
                "intent {} ({}) failed: {}",
                failure.index, failure.op, failure.message
            ));
            if report.not_attempted > 0 {
                issues.push(format!(
                    "{} later intent(s) were not attempted",
                    report.not_attempted
                ));
            }
        }
        let human = |issues: &[String]| {
            let mut lines = Vec::new();
            for applied in &report.applied {
                let verb = if report.dry_run { "would" } else { "did" };
                lines.push(format!(
                    "{verb} {} {}  {}",
                    applied.index + 1,
                    applied.op,
                    crate::format::truncate(&applied.entry.entry.title, 60)
                ));
            }
            for issue in issues {
                lines.push(format!("error: {issue}"));
            }
            lines.push(String::new());
            lines.push(report.note.to_owned());
            lines.join("\n")
        };

        match &report.failed {
            // The report is the result: emitted once, as the failure it is.
            Some(failure) => {
                let message = format!(
                    "intent {} ({}) failed: {}",
                    failure.index, failure.op, failure.message
                );
                Err(ctx.fail("apply", &failure.error_code, &message, &report, issues))
            }
            None => {
                ctx.emit("apply", &report, None, Vec::new(), || human(&issues));
                Ok(())
            }
        }
    }
}
