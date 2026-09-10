//! CLI root: global flags, subcommand dispatch, context construction.

use usage::{Cli, RunWith, Subcommands};

use crate::store::Ctx as StoreCtx;

/// Shared state handed to every command.
pub struct AppCtx {
    pub store: StoreCtx,
    pub json: bool,
    pub color: bool,
}

/// Minimal task tracker. One JSON document per task in .tasks/ — no daemon, no
/// index, no database. Run with no arguments to see what is ready.
#[derive(Cli)]
#[usage(bin = "tk", version)]
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
    /// What to do; with nothing, show what is ready
    #[usage(subcommand)]
    pub command: Option<Commands>,
}

/// `tk` with no subcommand is not an error: it answers the question the tool
/// exists for. usage-rs cannot generate that decision (an optional subcommand
/// has a state nothing generated can resolve), so the root implements the
/// dispatch itself.
impl RunWith<AppCtx> for Cli {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(command) => command.run_with(ctx),
            None => crate::commands::Ready::default().run_with(ctx),
        }
    }
}

#[derive(Subcommands)]
#[usage(run_with)]
// Commands hold their parsed arguments, and some commands have many flags; the
// enum is built once per process.
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Create .tasks/ here
    Init(crate::commands::Init),
    /// Create a task
    #[usage(alias = "new")]
    Add(crate::commands::Add),
    /// List tasks
    #[usage(alias = "ls")]
    List(crate::commands::List),
    /// List what can be started now
    #[usage(alias = "rdy")]
    Ready(crate::commands::Ready),
    /// Show one task
    Show(crate::commands::Show),
    /// Add a log entry
    #[usage(alias = "log")]
    Note(crate::commands::Note),
    /// Mark a task done
    Done(crate::commands::Done),
    /// Drop a task without doing it
    Drop(crate::commands::Drop),
    /// Reopen a done or dropped task
    Open(crate::commands::Open),
    /// Add or remove labels
    #[usage(alias = "tag")]
    Label(crate::commands::Label),
    /// Edit a task's fields in one write
    Edit(crate::commands::Edit),
    /// Add a blocker dependency
    Block(crate::commands::Block),
    /// Remove a blocker dependency
    Unblock(crate::commands::Unblock),
    /// Delete a task
    #[usage(alias = "rm")]
    Purge(crate::commands::Purge),
    /// Check store integrity (non-zero exit on findings)
    #[usage(alias = "ck")]
    Check(crate::commands::Check),
    /// Print the resolved task store location
    Path(crate::commands::StorePath),
    /// Apply a batch of intents from stdin under one lock
    Apply(crate::commands::Apply),
}

pub fn run() -> miette::Result<()> {
    let cli = Cli::parse();
    let json = cli.json;
    // Captured before the command is consumed by dispatch.
    let command = cli.command.as_ref().map_or("ready", |c| c.name());
    // Store errors convert through `?` rather than `into_diagnostic()`, which
    // would hide the concrete error type and lose the failure's kind.
    let result: miette::Result<()> = (|| {
        let store = StoreCtx::resolve(cli.dir.as_deref(), cli.tasks_dir.as_deref())?;
        let ctx = AppCtx {
            store,
            json,
            color: crate::format::use_color(),
        };
        // A typo is still an error: only a real subcommand parses.
        cli.run_with(ctx)
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
/// Recovered from the error's concrete type: miette's own `code()` is left empty
/// so human-facing output does not lead with a machine string, which means the
/// caller walks the types it knows about. Unknown failures report `error`.
fn error_code(err: &miette::Report) -> String {
    use crate::ids::IdError;
    use crate::model::ModelError;
    use crate::output::{InputError, code};
    use crate::store::StoreError;

    if let Some(store_error) = err.downcast_ref::<StoreError>() {
        return store_error.code().to_owned();
    }
    if err.downcast_ref::<InputError>().is_some() {
        return code::INVALID_INPUT.to_owned();
    }
    if let Some(id_error) = err.downcast_ref::<IdError>() {
        return match id_error {
            IdError::NotFound(_) => code::NOT_FOUND,
            IdError::Ambiguous { .. } => code::AMBIGUOUS,
            IdError::Empty => code::INVALID_INPUT,
        }
        .to_owned();
    }
    if err.downcast_ref::<ModelError>().is_some() {
        return code::INVALID_INPUT.to_owned();
    }
    code::ERROR.to_owned()
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
            Self::Note(_) => "note",
            Self::Done(_) => "done",
            Self::Drop(_) => "drop",
            Self::Open(_) => "open",
            Self::Label(_) => "label",
            Self::Edit(_) => "edit",
            Self::Block(_) => "block",
            Self::Unblock(_) => "unblock",
            Self::Purge(_) => "purge",
            Self::Check(_) => "check",
            Self::Path(_) => "path",
            Self::Apply(_) => "apply",
        }
    }
}
