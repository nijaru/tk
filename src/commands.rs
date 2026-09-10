//! Subcommand implementations.

mod add;
mod batch;
mod config;
mod deps;
mod detail;
mod edit;
mod list;
mod log;
mod misc;
mod show;
mod status;

pub use add::Add;
pub use batch::Apply;
pub use config::Config;
pub use deps::{Block, Unblock};
pub use detail::{Accept, Archive, Checkpoint, Evidence, Link, Unarchive, Unlink};
pub use edit::Edit;
pub use list::{List, Ready};
pub use log::Log;
pub use misc::{Check, Clean, Init, Lock, Mv, Purge, Recover, StorePath};
pub use show::Show;
pub use status::{Close, Defer, Done, Open, Start};

use miette::Result;

use crate::cli::AppCtx;

/// Resolve a user-supplied alias, ID, or ID prefix against the store.
///
/// Read-only callers use this; mutations resolve through [`crate::ops`].
pub fn resolve(ctx: &AppCtx, input: &str) -> Result<String> {
    Ok(ctx.store.store()?.resolve(input)?)
}

impl AppCtx {
    /// Require an existing v1 store, with the format gate applied.
    pub(crate) fn require_store(&self) -> Result<()> {
        Ok(self.store.require()?)
    }

    /// Emit a result in the shape the caller asked for.
    ///
    /// `--json` always produces the same envelope (see [`crate::output`]);
    /// otherwise the human rendering is built lazily, so JSON runs never pay
    /// for it.
    pub(crate) fn emit<T: serde::Serialize>(
        &self,
        command: &str,
        data: &T,
        rev: Option<String>,
        issues: Vec<String>,
        human: impl FnOnce() -> String,
    ) {
        if self.json {
            let data = serde_json::to_value(data).unwrap_or(serde_json::Value::Null);
            let envelope = crate::output::ok(command, data, rev, issues);
            println!("{}", crate::format::format_json(&envelope));
        } else {
            println!("{}", human());
        }
    }

    /// Emit a failure the command decided how to name, then exit non-zero.
    pub(crate) fn fail<T: serde::Serialize>(
        &self,
        command: &str,
        error_code: &str,
        message: &str,
        data: &T,
        issues: Vec<String>,
    ) -> miette::Report {
        if self.json {
            let mut envelope = crate::output::err(command, error_code, message);
            envelope.data = serde_json::to_value(data).unwrap_or(serde_json::Value::Null);
            envelope.issues = issues;
            println!("{}", crate::format::format_json(&envelope));
        }
        crate::output::Reported(message.to_owned()).into()
    }
}
