//! CLI root: global flags, subcommand dispatch, context construction.

use usage::{Cli, Subcommands};

use crate::store::Ctx as StoreCtx;

/// Shared state handed to every command.
pub struct AppCtx {
    pub store: StoreCtx,
    pub json: bool,
    pub color: bool,
}

/// Minimal task tracker. Plain JSON in .tasks/ — no daemons, no conflicts.
#[derive(Cli)]
#[usage(bin = "tk", version, run_with)]
pub struct Cli {
    /// Output as JSON
    #[usage(short = 'j', long, global)]
    pub json: bool,
    /// Run in a different directory
    #[usage(short = 'C', long, global, value_name = "DIR")]
    pub dir: Option<String>,
    #[usage(subcommand)]
    pub command: Commands,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum Commands {
    /// Initialize .tasks/ in the current directory
    Init(crate::commands::Init),
    /// Create a task
    Add(crate::commands::Add),
    /// List tasks
    #[usage(alias = "ls")]
    List(crate::commands::List),
    /// List active/open unblocked tasks
    #[usage(alias = "rdy")]
    Ready(crate::commands::Ready),
    /// Show task details
    Show(crate::commands::Show),
    /// Start working on a task (open → active)
    #[usage(alias = "active")]
    Start(crate::commands::Start),
    /// Reset a task status to open
    Open(crate::commands::Open),
    /// Defer a task
    Defer(crate::commands::Defer),
    /// Complete a task
    Done(crate::commands::Done),
    /// Close/cancel a task
    Close(crate::commands::Close),
    /// Edit a task
    Edit(crate::commands::Edit),
    /// Add a log entry to a task
    Log(crate::commands::Log),
    /// Add a blocker dependency
    Block(crate::commands::Block),
    /// Remove a blocker dependency
    Unblock(crate::commands::Unblock),
    /// Delete a task
    #[usage(alias = "rm")]
    Remove(crate::commands::Remove),
    /// Move a task to a different project
    Mv(crate::commands::Mv),
    /// Remove old completed tasks
    Clean(crate::commands::Clean),
    /// Check task integrity
    Check(crate::commands::Check),
    /// Show or set configuration
    Config(crate::commands::Config),
}

pub fn run() -> miette::Result<()> {
    use miette::IntoDiagnostic;
    let cli = Cli::parse();
    let store = StoreCtx::discover(cli.dir.as_deref()).into_diagnostic()?;
    let ctx = AppCtx {
        store,
        json: cli.json,
        color: crate::format::use_color(),
    };
    cli.run_command_with(ctx)
}
