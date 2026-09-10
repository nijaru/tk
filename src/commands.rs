//! Subcommand implementations.

mod add;
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
pub use config::Config;
pub use deps::{Block, Unblock};
pub use detail::{Accept, Archive, Checkpoint, Evidence, Link, Unarchive, Unlink};
pub use edit::Edit;
pub use list::{List, Ready};
pub use log::Log;
pub use misc::{Check, Clean, Init, Lock, Mv, Purge, Recover, StorePath};
pub use show::Show;
pub use status::{Close, Defer, Done, Open, Start};

use miette::IntoDiagnostic;

use crate::cli::AppCtx;
use crate::model::TaskView;
use crate::store;

/// Resolve a user-supplied alias, ID, or ID prefix against the store.
///
/// Read-only callers use this; mutations resolve inside their transaction.
pub fn resolve(ctx: &AppCtx, input: &str) -> miette::Result<String> {
    let store = ctx.store.store().into_diagnostic()?;
    store.resolve(input).into_diagnostic()
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
            let txn = ctx.store.txn().into_diagnostic()?;
            let store = store::Store::new(txn.ctx());
            Ok(Self {
                store,
                txn: Some(txn),
            })
        } else {
            Ok(Self {
                store: ctx.store.store().into_diagnostic()?,
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
                txn.append_if_rev(id, op, data, expect_rev)
                    .into_diagnostic()?;
            }
            None => {
                self.store.append(id, op, data).into_diagnostic()?;
            }
        }
        Ok(())
    }

    pub(crate) fn view(&self, id: &str) -> miette::Result<TaskView> {
        self.store.view_of(id).into_diagnostic()
    }

    pub(crate) fn load(&self, id: &str) -> miette::Result<crate::record::Record> {
        self.store.load(id).into_diagnostic()
    }
}

impl AppCtx {
    /// Require an existing v1 store, with the format gate applied.
    pub(crate) fn require_store(&self) -> miette::Result<()> {
        self.store.require().into_diagnostic()
    }
}
