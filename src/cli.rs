//! CLI root: global flags, subcommand dispatch, context construction.

use usage::{Cli, Subcommands};

use crate::store::Ctx as StoreCtx;

/// Shared state handed to every command.
pub struct AppCtx {
    pub store: StoreCtx,
    pub json: bool,
    pub color: bool,
}

/// Minimal task tracker. Append-only JSON records in .tasks/ — no daemons, no runtime.
#[derive(Cli)]
#[usage(bin = "tk", version, run_with)]
pub struct Cli {
    /// Output as JSON
    #[usage(short = 'j', long, global)]
    pub json: bool,
    /// Run in a different directory
    #[usage(short = 'C', long, global, value_name = "DIR")]
    pub dir: Option<String>,
    /// Task store directory: exact path, must already exist (no discovery)
    #[usage(long = "tasks-dir", global, value_name = "DIR")]
    pub tasks_dir: Option<String>,
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
    /// Replace the current checkpoint (one summary, not the log)
    #[usage(alias = "ck")]
    Checkpoint(crate::commands::Checkpoint),
    /// Add links to research, decisions, or source locations
    Link(crate::commands::Link),
    /// Remove links from a task
    Unlink(crate::commands::Unlink),
    /// Add or show acceptance criteria
    Accept(crate::commands::Accept),
    /// Add or show completion evidence
    Evidence(crate::commands::Evidence),
    /// Archive a done/closed task without deleting it
    Archive(crate::commands::Archive),
    /// Return an archived task to active views
    Unarchive(crate::commands::Unarchive),
    /// Add a log entry to a task
    Log(crate::commands::Log),
    /// Add a blocker dependency
    Block(crate::commands::Block),
    /// Remove a blocker dependency
    Unblock(crate::commands::Unblock),
    /// Delete a task record
    #[usage(alias = "rm")]
    Purge(crate::commands::Purge),
    /// Drop a record's torn last line (from an interrupted write)
    Recover(crate::commands::Recover),
    /// Move a task to a different project
    Mv(crate::commands::Mv),
    /// Remove old completed tasks (archives by default)
    Clean(crate::commands::Clean),
    /// Check store integrity (non-zero exit on findings)
    Check(crate::commands::Check),
    /// Print the resolved task store location
    Path(crate::commands::StorePath),
    /// Run a command while holding the store mutation lock
    Lock(crate::commands::Lock),
    /// Show or set configuration
    Config(crate::commands::Config),
}

pub fn run() -> miette::Result<()> {
    use miette::IntoDiagnostic;
    let cli = Cli::parse();
    let store =
        StoreCtx::resolve(cli.dir.as_deref(), cli.tasks_dir.as_deref()).into_diagnostic()?;
    let ctx = AppCtx {
        store,
        json: cli.json,
        color: crate::format::use_color(),
    };
    cli.run_command_with(ctx)
}
