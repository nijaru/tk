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
    /// Record a non-blocking relationship with another task
    Relate(crate::commands::Relate),
    /// Remove a non-blocking relationship
    Unrelate(crate::commands::Unrelate),
    /// Delete a task record
    #[usage(alias = "rm")]
    Purge(crate::commands::Purge),
    /// Drop a record's torn last line (from an interrupted write)
    Recover(crate::commands::Recover),
    /// Move a task to a different project
    Mv(crate::commands::Mv),
    /// Apply a batch of intents from stdin under one lock
    Apply(crate::commands::Apply),
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
    let cli = Cli::parse();
    let json = cli.json;
    // Captured before the command is consumed by dispatch.
    let command = cli.command.name();
    // Store errors convert through `?` rather than `into_diagnostic()`, which
    // would hide the concrete error type and lose the failure's kind.
    let result: miette::Result<()> = (|| {
        let store = StoreCtx::resolve(cli.dir.as_deref(), cli.tasks_dir.as_deref())?;
        let ctx = AppCtx {
            store,
            json,
            color: crate::format::use_color(),
        };
        cli.run_command_with(ctx)
    })();
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            // A failing `--json` run still answers in the same envelope, so a
            // caller never has to parse stderr to find out what happened.
            if json && err.downcast_ref::<crate::output::Reported>().is_none() {
                let code = error_code(&err);
                let envelope = crate::output::err(command, &code, &format!("{err}"));
                println!("{}", crate::format::format_json(&envelope));
            }
            Err(err)
        }
    }
}

/// The failure's kind, for the JSON envelope.
///
/// Tries the concrete error first, then whatever miette carries, so a store
/// failure keeps its specific kind instead of collapsing to `error`.
fn error_code(err: &miette::Report) -> String {
    if let Some(store_error) = err.downcast_ref::<crate::store::StoreError>() {
        return store_error.code().to_owned();
    }
    match err.code() {
        Some(code) => code.to_string(),
        None => crate::output::code::ERROR.to_owned(),
    }
}

impl Commands {
    /// The invoked subcommand's name, for the JSON envelope.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Init(_) => "init",
            Self::Add(_) => "add",
            Self::List(_) => "list",
            Self::Ready(_) => "ready",
            Self::Show(_) => "show",
            Self::Start(_) => "start",
            Self::Open(_) => "open",
            Self::Defer(_) => "defer",
            Self::Done(_) => "done",
            Self::Close(_) => "close",
            Self::Edit(_) => "edit",
            Self::Checkpoint(_) => "checkpoint",
            Self::Link(_) => "link",
            Self::Unlink(_) => "unlink",
            Self::Accept(_) => "accept",
            Self::Evidence(_) => "evidence",
            Self::Archive(_) => "archive",
            Self::Unarchive(_) => "unarchive",
            Self::Log(_) => "log",
            Self::Block(_) => "block",
            Self::Unblock(_) => "unblock",
            Self::Relate(_) => "relate",
            Self::Unrelate(_) => "unrelate",
            Self::Purge(_) => "purge",
            Self::Recover(_) => "recover",
            Self::Mv(_) => "mv",
            Self::Apply(_) => "apply",
            Self::Clean(_) => "clean",
            Self::Check(_) => "check",
            Self::Path(_) => "path",
            Self::Lock(_) => "lock",
            Self::Config(_) => "config",
        }
    }
}
