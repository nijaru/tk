//! `tk list` / `tk ready`

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::{Priority, Status};
use crate::store::{self, ListOptions};

use super::resolve;

/// List tasks
#[derive(Args)]
pub struct List {
    /// Search title, description, and ID
    pub search: Vec<String>,
    /// Show all, including done and closed
    #[usage(short = 'a', long = "all")]
    pub all: bool,
    /// Filter by status (open/active/deferred/done/closed)
    #[usage(short = 's', long)]
    pub status: Option<String>,
    /// Filter by priority
    #[usage(short = 'p', long)]
    pub priority: Option<String>,
    /// Filter by project
    #[usage(short = 'P', long)]
    pub project: Option<String>,
    /// Filter by label
    #[usage(short = 'l', long)]
    pub label: Option<String>,
    /// Filter by assignee
    #[usage(long)]
    pub assignee: Option<String>,
    /// Filter by parent task
    #[usage(long)]
    pub parent: Option<String>,
    /// Top-level tasks only
    #[usage(long)]
    pub roots: bool,
    /// Overdue tasks only
    #[usage(long)]
    pub overdue: bool,
    /// Limit results
    #[usage(short = 'n', long, default = "20")]
    pub limit: i64,
}

impl RunWith<AppCtx> for List {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let tasks = run_list(&ctx, self)?;
        if ctx.json {
            println!("{}", format::format_json(&tasks));
        } else {
            println!("{}", format::format_task_list(&tasks, "", ctx.color));
        }
        Ok(())
    }
}

pub fn run_list(ctx: &AppCtx, cmd: List) -> miette::Result<Vec<crate::model::TaskView>> {
    let status = cmd
        .status
        .map(|s| Status::parse(&s))
        .transpose()
        .into_diagnostic()?;
    let priority = cmd
        .priority
        .map(|p| Priority::parse(&p))
        .transpose()
        .into_diagnostic()?;
    let parent = cmd.parent.map(|p| resolve(ctx, &p)).transpose()?.map(Some);
    let hide_terminal = !cmd.all && !status.is_some_and(|s| s.is_terminal());

    store::list_tasks(
        &ctx.store,
        &ListOptions {
            search: cmd.search.join(" "),
            hide_terminal,
            status,
            priority,
            project: cmd.project.unwrap_or_default(),
            label: cmd.label.unwrap_or_default(),
            assignee: cmd.assignee.unwrap_or_default(),
            parent,
            roots: cmd.roots,
            overdue: cmd.overdue,
            limit: if cmd.all {
                0
            } else {
                cmd.limit.max(0) as usize
            },
        },
    )
    .into_diagnostic()
}

/// List active/open unblocked tasks
#[derive(Args)]
pub struct Ready;

impl RunWith<AppCtx> for Ready {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let tasks = store::list_tasks(&ctx.store, &ListOptions::default()).into_diagnostic()?;
        let ready: Vec<_> = tasks
            .into_iter()
            .filter(|t| {
                matches!(t.task.status, Status::Open | Status::Active) && !t.blocked_by_incomplete
            })
            .collect();
        if ctx.json {
            println!("{}", format::format_json(&ready));
        } else {
            println!(
                "{}",
                format::format_task_list(&ready, "No ready tasks found. Good job!", ctx.color)
            );
        }
        Ok(())
    }
}
