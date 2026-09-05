//! Subcommand implementations.

mod add;
mod config;
mod deps;
mod edit;
mod list;
mod log;
mod misc;
mod show;
mod status;

pub use add::Add;
pub use config::Config;
pub use deps::{Block, Unblock};
pub use edit::Edit;
pub use list::{List, Ready};
pub use log::Log;
pub use misc::{Check, Clean, Init, Mv, Remove};
pub use show::Show;
pub use status::{Close, Defer, Done, Open, Start};

use miette::IntoDiagnostic;

use crate::cli::AppCtx;
use crate::ids;

/// Resolve a user-supplied ID/prefix/ref against the store.
pub fn resolve(ctx: &AppCtx, input: &str) -> miette::Result<String> {
    ctx.require_store()?;
    ids::resolve_id(&ctx.store.tasks_dir, input).into_diagnostic()
}

impl AppCtx {
    fn require_store(&self) -> miette::Result<()> {
        self.store.require().into_diagnostic()
    }
}
