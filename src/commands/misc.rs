//! Deleting, checking, locating, and locking the store.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::ops;
use crate::store;

/// Delete a task
#[derive(Args, Debug)]
pub struct Purge {
    /// Ref, or part of a title
    pub r#ref: String,
    /// Say what would be deleted, and delete nothing
    #[usage(long)]
    pub dry_run: bool,
    /// Refuse if the entry changed since this revision was read
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Purge {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn()?;
        let r#ref = ops::resolve_rev(&txn, &self.r#ref, self.if_rev.as_deref())?;
        let outcome = if self.dry_run {
            ops::Purge {
                deleted: txn.store().get(&r#ref)?,
                unblocked: txn
                    .store()
                    .scan()?
                    .entries
                    .into_iter()
                    .filter(|(_, e)| e.blocked_by.iter().any(|b| b == &r#ref))
                    .map(|(_, e)| e.r#ref)
                    .collect(),
            }
        } else {
            ops::purge(&txn, &r#ref)?
        };
        drop(txn);

        let deleted = outcome.deleted.entry.r#ref.clone();
        let title = outcome.deleted.entry.title.clone();
        let unblocked = outcome.unblocked.clone();
        let dry_run = self.dry_run;
        ctx.emit("purge", &outcome, None, Vec::new(), move || {
            let verb = if dry_run { "would delete" } else { "deleted" };
            let mut line = format!("{verb} {deleted}  {}", format::truncate(&title, 60));
            if !unblocked.is_empty() {
                line.push_str(&format!("\nunblocked: {}", unblocked.join(", ")));
            }
            line
        });
        Ok(())
    }
}

/// Check store integrity (non-zero exit on findings)
#[derive(Args, Debug)]
pub struct Check {
    /// Print nothing when the store is clean
    #[usage(short = 'q', long)]
    pub quiet: bool,
}

impl RunWith<AppCtx> for Check {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let issues = store::check_integrity(&ctx.store)?;
        if issues.is_empty() {
            ctx.emit(
                "check",
                &serde_json::json!({ "clean": true, "findings": [] }),
                None,
                Vec::new(),
                || {
                    if self.quiet {
                        String::new()
                    } else {
                        "ok: the store is consistent".to_owned()
                    }
                },
            );
            return Ok(());
        }
        // A failure, reported once: the findings are both the message a human
        // reads and the `findings` array a machine reads.
        let findings = issues
            .iter()
            .map(|issue| format!("- {issue}"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(ctx.fail(
            "check",
            crate::output::code::CHECK_FAILED,
            &findings,
            &serde_json::json!({ "clean": false, "findings": issues }),
            issues,
        ))
    }
}

/// Print the resolved task store location
#[derive(Args, Debug)]
pub struct StorePath;

impl RunWith<AppCtx> for StorePath {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let path = ctx.store.tasks_dir.display().to_string();
        let exists = ctx.store.exists;
        let source = ctx.store.source.name();
        ctx.emit(
            "path",
            &serde_json::json!({ "store": path, "exists": exists, "found": source }),
            None,
            Vec::new(),
            || path.clone(),
        );
        Ok(())
    }
}

/// Run a command while holding the store mutation lock
#[derive(Args, Debug)]
pub struct Lock {
    /// The command to run, after --
    #[usage(value_name = "COMMAND", double_dash = "required", allow_hyphen_values)]
    pub command: Vec<String>,
}

impl RunWith<AppCtx> for Lock {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if self.command.is_empty() {
            return Err(ctx.fail(
                "lock",
                crate::output::code::INVALID_INPUT,
                "nothing to run: 'tk lock -- <command> [args...]'",
                &serde_json::Value::Null,
                Vec::new(),
            ));
        }
        // The lock is released when this guard drops, or by the OS if the
        // process exits below.
        let _guard = ctx.store.lock_store()?;
        let status = std::process::Command::new(&self.command[0])
            .args(&self.command[1..])
            .status()
            .map_err(|e| miette::miette!("could not run {}: {e}", self.command[0]))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
