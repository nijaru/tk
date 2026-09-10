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
pub use deps::{Block, Relate, Unblock, Unrelate};
pub use detail::{Accept, Archive, Checkpoint, Evidence, Link, Unarchive, Unlink};
pub use edit::Edit;
pub use list::{List, Ready};
pub use log::Log;
pub use misc::{Check, Clean, Init, Lock, Mv, Purge, Recover, StorePath};
pub use show::Show;
pub use status::{Close, Defer, Done, Open, Start};

use crate::cli::AppCtx;
use crate::model::TaskView;
use crate::store;

/// Resolve a user-supplied alias, ID, or ID prefix against the store.
///
/// Read-only callers use this; mutations resolve inside their transaction.
pub fn resolve(ctx: &AppCtx, input: &str) -> miette::Result<String> {
    let store = ctx.store.store()?;
    Ok(store.resolve(input)?)
}

/// One mutation path, chosen once per command.
///
/// `--if-rev` and any value derived from a read need the store lock; a plain
/// last-writer-wins or commutative append does not. Picking the path in one
/// place keeps that decision visible instead of scattered across commands.
pub(crate) struct Writer<'a> {
    store: store::Store<'a>,
    txn: Option<store::Txn<'a>>,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(ctx: &'a AppCtx, needs_lock: bool) -> miette::Result<Self> {
        ctx.require_store()?;
        if needs_lock {
            let txn = ctx.store.txn()?;
            let store = store::Store::new(txn.ctx());
            Ok(Self {
                store,
                txn: Some(txn),
            })
        } else {
            Ok(Self {
                store: ctx.store.store()?,
                txn: None,
            })
        }
    }

    /// Read the current state through the store handle.
    pub(crate) fn store(&self) -> &store::Store<'a> {
        &self.store
    }

    /// The lock-holding handle, when this writer was created with one.
    pub(crate) fn txn(&self) -> Option<&store::Txn<'a>> {
        self.txn.as_ref()
    }

    pub(crate) fn append(
        &self,
        id: &str,
        op: &str,
        data: serde_json::Value,
        expect_rev: Option<&str>,
    ) -> miette::Result<()> {
        match &self.txn {
            Some(txn) => {
                txn.append_if_rev(id, op, data, expect_rev)?;
            }
            None => {
                self.store.append(id, op, data)?;
            }
        }
        Ok(())
    }

    pub(crate) fn view(&self, id: &str) -> miette::Result<TaskView> {
        Ok(self.store.view_of(id)?)
    }

    pub(crate) fn load(&self, id: &str) -> miette::Result<crate::record::Record> {
        Ok(self.store.load(id)?)
    }
}

impl AppCtx {
    /// Require an existing v1 store, with the format gate applied.
    pub(crate) fn require_store(&self) -> miette::Result<()> {
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
