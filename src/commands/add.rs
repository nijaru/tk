//! `tk add`

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Priority;
use crate::store::{self, CreateOptions};
use crate::{format, timeutil};

/// Create a task
#[derive(Args)]
pub struct Add {
    /// Task title
    #[usage(required)]
    pub title: Vec<String>,
    /// Priority (0-4, p0-p4, or none/urgent/high/medium/low)
    #[usage(short = 'p', long)]
    pub priority: Option<String>,
    /// Project prefix
    #[usage(short = 'P', long)]
    pub project: Option<String>,
    /// Description
    #[usage(short = 'd', long)]
    pub desc: Option<String>,
    /// Labels (comma-separated, repeatable)
    #[usage(short = 'l', long, delimiter = ',')]
    pub labels: Vec<String>,
    /// Assignees (comma-separated, repeatable)
    #[usage(short = 'A', long, delimiter = ',')]
    pub assignees: Vec<String>,
    /// Parent task ID
    #[usage(long)]
    pub parent: Option<String>,
    /// Estimate (user-defined units)
    #[usage(long)]
    pub estimate: Option<i64>,
    /// Due date (YYYY-MM-DD or relative +Nh/+Nd/+Nw/+Nm)
    #[usage(long)]
    pub due: Option<String>,
}

impl RunWith<AppCtx> for Add {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let priority = self
            .priority
            .map(|p| Priority::parse(&p))
            .transpose()
            .into_diagnostic()?;
        let due_date = self
            .due
            .map(|d| timeutil::parse_due_date(&d))
            .transpose()
            .into_diagnostic()?
            .flatten();

        // Resolution, parent validation, and the write share one transaction.
        let txn = ctx.store.txn_bootstrap().into_diagnostic()?;
        let parent = self
            .parent
            .map(|p| txn.resolve(&p))
            .transpose()
            .into_diagnostic()?;
        if let Some(p) = &parent {
            store::validate_parent(txn.ctx(), p, "").into_diagnostic()?;
        }

        let labels = (!self.labels.is_empty()).then_some(self.labels);
        let assignees = (!self.assignees.is_empty()).then_some(self.assignees);

        let t = txn
            .create(CreateOptions {
                title: self.title.join(" "),
                description: self.desc,
                priority,
                project: self.project,
                labels,
                assignees,
                parent,
                estimate: self.estimate,
                due_date,
            })
            .into_diagnostic()?;

        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!("Created task {}: {}", t.id, t.task.title);
        }
        Ok(())
    }
}
