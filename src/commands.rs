//! Subcommand implementations.
//!
//! Every command is the same three steps: resolve what it names, call one
//! [`crate::ops`] function, print what came back. Nothing here decides what a
//! change means — that lives in `ops`, so `apply` and the commands cannot drift.

mod add;
mod batch;
mod config;
mod edit;
mod list;
mod misc;

pub use add::{Add, Init};
pub use batch::Apply;
pub use config::Config;
pub use edit::{Accept, Block, Done, Drop, Edit, Label, Note, State, Status, Unblock};
pub use list::{List, Ready, Show};
pub use misc::{Check, Lock, Purge, StorePath};

use miette::Result;

use crate::cli::AppCtx;
use crate::timeutil;

impl AppCtx {
    /// Require a store this binary can read, applying the format gate.
    pub(crate) fn require_store(&self) -> Result<()> {
        Ok(self.store.require()?)
    }

    /// Now, as every write stamps it.
    pub(crate) fn now(&self) -> String {
        timeutil::now_rfc3339_nano()
    }

    /// Emit a result in the shape the caller asked for.
    ///
    /// `--json` always produces the same envelope (see [`crate::output`]);
    /// otherwise the human rendering is built lazily, so a JSON run never pays
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
