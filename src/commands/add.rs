//! `tk add`

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::model::Priority;
use crate::ops::{self, Mutation};
use crate::store::CreateOptions;

/// Create a task
#[derive(Args)]
pub struct Add {
    /// Task title
    #[usage(required)]
    pub title: Vec<String>,
    /// Priority (0-4, p0-p4, or none/urgent/high/medium/low)
    #[usage(short = 'p', long)]
    pub priority: Option<String>,
    /// Project (display grouping; identity is unaffected)
    #[usage(short = 'P', long)]
    pub project: Option<String>,
    /// Description
    #[usage(short = 'd', long)]
    pub desc: Option<String>,
    /// Labels (comma-separated, repeatable)
    #[usage(short = 'l', long, delimiter = ',')]
    pub labels: Vec<String>,
    /// Parent task (alias, ID, or ID prefix)
    #[usage(long)]
    pub parent: Option<String>,
}

impl RunWith<AppCtx> for Add {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let priority = self.priority.map(|p| Priority::parse(&p)).transpose()?;
        // Creation takes the store lock: alias uniqueness is store-wide, and
        // parent validation must not race a concurrent delete.
        ctx.require_store()?;
        let m = Mutation::locked(&ctx.store)?;
        let parent = self.parent.map(|p| m.store().resolve(&p)).transpose()?;
        let t = ops::create(
            &m,
            CreateOptions {
                title: self.title.join(" "),
                description: self.desc,
                priority,
                project: self.project,
                labels: (!self.labels.is_empty()).then_some(self.labels),
                parent,
            },
        )?;
        let human = format!("Created task {} ({})", t.task.alias, t.task.id);
        ctx.emit("add", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}
