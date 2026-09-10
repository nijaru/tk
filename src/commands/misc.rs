//! `tk init` / `tk mv` / `tk clean` / `tk check` / `tk purge` / `tk recover` /
//! `tk path` / `tk lock`

use std::io::{BufRead, IsTerminal as _};

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
use crate::output::code;
use crate::record::op;
use crate::store::{self, StoreLock};

use super::Writer;

/// Delete a task record
///
/// Refuses while other tasks still reference it: those references would become
/// dangling, and a broken graph is worse than a stale record.
#[derive(Args)]
pub struct Purge {
    /// Task alias, ID, or ID prefix
    pub id: String,
    /// Skip confirmation
    #[usage(short = 'f', long)]
    pub force: bool,
    /// Delete anyway, removing the references that point at it
    #[usage(long)]
    pub scrub: bool,
    /// Reject the delete unless the task still has this revision
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Purge {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn()?;
        let id = txn.resolve(&self.id)?;
        txn.check_rev(&id, self.if_rev.as_deref())?;
        let record = txn.load(&id)?;

        if !self.force {
            // Never delete unattended: a script or agent that forgets -f gets an
            // error, not a silent removal.
            if !std::io::stdin().is_terminal() {
                return Err(crate::output::invalid(format!(
                    "refusing to delete {} without -f (stdin is not a terminal)",
                    record.state.alias
                )));
            }
            print!(
                "Delete {} {:?}? [y/N] ",
                record.state.alias, record.state.title
            );
            use std::io::Write as _;
            std::io::stdout().flush().into_diagnostic()?;
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .into_diagnostic()?;
            if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
                ctx.emit("purge", &serde_json::Value::Null, None, Vec::new(), || {
                    "Aborted.".to_owned()
                });
                return Ok(());
            }
        }

        let out = txn.purge(&id, self.scrub)?;
        let data = serde_json::json!({
            "deleted": out.deleted,
            "references_scrubbed": out.references_scrubbed,
            "referrers": out.referrers,
        });
        let human = format!(
            "Deleted {} (scrubbed {} references)",
            out.deleted, out.references_scrubbed
        );
        ctx.emit("purge", &data, None, Vec::new(), || human);
        Ok(())
    }
}

/// Initialize .tasks/ in the current directory
#[derive(Args)]
pub struct Init {
    /// Project name (default: directory name)
    #[usage(short = 'P', long)]
    pub project: Option<String>,
}

impl RunWith<AppCtx> for Init {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if ctx.store.exists {
            // Either this store is already v1, or it is a legacy layout that
            // must be migrated rather than written over. `check_format` says
            // which, in words the reader can act on.
            return match ctx.store.check_format() {
                Ok(()) => Err(miette::miette!(
                    "task store already initialized at {}",
                    ctx.store.tasks_dir.display()
                )),
                Err(e) => Err(e.into()),
            };
        }
        let name = match self.project {
            Some(p) => p,
            None => ctx
                .store
                .root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| s != "." && s != "/")
                .unwrap_or_else(|| "tk".to_owned()),
        };
        ids::validate_project(&name)?;
        // Creating the store is deliberate: an explicit --tasks-dir is allowed
        // to come into existence here, and only here.
        let txn = ctx.store.txn_init()?;
        let config = crate::model::Config {
            project: name,
            ..Default::default()
        };
        txn.init_store(&config)?;
        let data = serde_json::json!({
            "tasks_dir": ctx.store.tasks_dir.display().to_string(),
            "format": config.format,
            "project": config.project,
        });
        let human = format!(
            "Initialized empty tk project in {}",
            ctx.store.tasks_dir.display()
        );
        ctx.emit("init", &data, None, Vec::new(), || human);
        Ok(())
    }
}

/// Move a task to a different project
///
/// Identity is unaffected: a move changes one display field, and no reference
/// anywhere needs rewriting.
#[derive(Args)]
pub struct Mv {
    /// Task alias, ID, or ID prefix
    pub source: String,
    /// Target project name
    pub project: String,
}

impl RunWith<AppCtx> for Mv {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ids::validate_project(&self.project)?;
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.source)?;
        let record = writer.load(&id)?;
        if record.state.project == self.project {
            return Err(miette::miette!(
                "{} is already in project {:?}",
                record.state.alias,
                self.project
            ));
        }
        writer.append(&id, op::PROJECT, serde_json::json!(self.project), None)?;
        let t = writer.view(&id)?;
        let human = format!(
            "Moved {} ({}) to project {}",
            t.task.alias, t.task.id, t.task.project
        );
        ctx.emit("mv", &t, Some(t.rev.clone()), Vec::new(), || human);
        Ok(())
    }
}

/// Remove old completed tasks
#[derive(Args)]
pub struct Clean {
    /// Remove tasks completed more than N days ago
    #[usage(long = "older-than")]
    pub older_than: Option<i64>,
    /// Force clean even if disabled in config
    #[usage(long)]
    pub force: bool,
    /// Delete the records instead of archiving them (scrubs references)
    #[usage(long)]
    pub purge: bool,
}

impl RunWith<AppCtx> for Clean {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let config = ctx.store.load_config()?;
        let days = if let Some(n) = self.older_than {
            if n < 0 {
                return Err(crate::output::invalid("--older-than must be non-negative"));
            }
            n
        } else if config.clean_after.enabled || self.force {
            let d = config.clean_after.days;
            if d <= 0 {
                crate::model::Config::default().clean_after.days
            } else {
                d
            }
        } else {
            ctx.emit("clean", &serde_json::Value::Null, None, Vec::new(), || {
                "Auto-clean is disabled. Use --older-than N or enable with 'tk config clean-after enable'.".to_owned()
            });
            return Ok(());
        };
        let txn = ctx.store.txn()?;
        let out = txn.clean(days, self.purge)?;
        let data = serde_json::json!({
            "archived": out.archived,
            "purged": out.purged,
            "references_scrubbed": out.references_scrubbed,
            "days": days,
        });
        let human = if self.purge {
            format!(
                "Purged {} tasks completed more than {days} days ago (scrubbed {} references).",
                out.purged, out.references_scrubbed
            )
        } else {
            format!(
                "Archived {} tasks completed more than {days} days ago. Use --purge to delete them.",
                out.archived
            )
        };
        ctx.emit("clean", &data, None, Vec::new(), || human);
        Ok(())
    }
}

/// Check store integrity
///
/// Exits non-zero when anything is reported, so scripts and agents cannot
/// mistake findings for success.
#[derive(Args)]
pub struct Check;

impl RunWith<AppCtx> for Check {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let issues = store::check_integrity(&ctx.store)?;
        if issues.is_empty() {
            ctx.emit(
                "check",
                &serde_json::json!({"ok": true, "issues": []}),
                None,
                Vec::new(),
                || "No integrity issues found.".to_owned(),
            );
            return Ok(());
        }
        let summary = format!("integrity check failed: {} issue(s)", issues.len());
        if ctx.json {
            // Report the findings structurally, then exit non-zero without a
            // second envelope.
            return Err(ctx.fail(
                "check",
                code::CHECK_FAILED,
                &summary,
                &serde_json::json!({"ok": false, "issues": issues}),
                issues,
            ));
        }
        for issue in &issues {
            println!("{}", format::warning(issue, ctx.color));
        }
        Err(crate::output::Reported(summary).into())
    }
}

/// Drop a record's torn last line, left behind by an interrupted write
///
/// Only the incomplete final line is removed: it was never a complete event,
/// so nothing that was appended is lost.
#[derive(Args)]
pub struct Recover {
    /// Task alias, ID, or ID prefix (default: every record)
    pub id: Option<String>,
    /// Report what would be dropped without changing anything
    #[usage(long)]
    pub dry_run: bool,
}

impl RunWith<AppCtx> for Recover {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let txn = ctx.store.txn()?;
        let id = self.id.map(|input| txn.resolve(&input)).transpose()?;
        let out = txn.recover(id.as_deref(), self.dry_run)?;
        let data = serde_json::json!({
            "repaired": out.repaired,
            "bytes_dropped": out.bytes_dropped,
            "dry_run": out.dry_run,
        });
        let human = if out.repaired.is_empty() {
            "No torn records found.".to_owned()
        } else {
            format!(
                "{} {} record(s), dropping {} byte(s): {}",
                if self.dry_run {
                    "Would repair"
                } else {
                    "Repaired"
                },
                out.repaired.len(),
                out.bytes_dropped,
                out.repaired.join(", ")
            )
        };
        ctx.emit("recover", &data, None, Vec::new(), || human);
        Ok(())
    }
}

/// Print the resolved task store location
#[derive(Args)]
pub struct StorePath;

impl RunWith<AppCtx> for StorePath {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let s = &ctx.store;
        let data = serde_json::json!({
            "tasks_dir": s.tasks_dir.display().to_string(),
            "root": s.root.display().to_string(),
            "exists": s.exists,
            "source": s.source.name(),
            "worktree": s.worktree,
        });
        let human = s.tasks_dir.display().to_string();
        ctx.emit("path", &data, None, Vec::new(), || human);
        Ok(())
    }
}

/// Run a command while holding the store mutation lock
///
/// The lock serializes cooperating tk writers (and, when used for sync, the
/// checkout itself). The command must not run tk against the same store: the
/// lock is advisory and not reentrant.
#[derive(Args)]
pub struct Lock {
    /// Directory tree to scan for task stores to lock (repeatable)
    #[usage(long, value_name = "DIR")]
    pub scan: Vec<String>,
    /// Command to run while holding the lock, after --
    #[usage(value_name = "COMMAND", double_dash = "required", allow_hyphen_values)]
    pub command: Vec<String>,
}

impl RunWith<AppCtx> for Lock {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if self.command.is_empty() {
            return Err(crate::output::invalid(
                "provide a command after --, for example: tk lock -- git pull --ff-only",
            ));
        }

        let stores: Vec<store::Ctx> = if self.scan.is_empty() {
            ctx.store.require()?;
            vec![ctx.store.clone()]
        } else {
            let mut found = Vec::new();
            for dir in &self.scan {
                found.extend(store::find_stores(std::path::Path::new(dir))?);
            }
            let mut stores: Vec<store::Ctx> = found
                .into_iter()
                .map(store::Ctx::at_tasks_dir)
                .filter(|c| c.exists)
                .collect();
            // Deterministic order so concurrent lockers cannot deadlock.
            stores.sort_by(|a, b| a.tasks_dir.cmp(&b.tasks_dir));
            stores.dedup_by(|a, b| a.tasks_dir == b.tasks_dir);
            if stores.is_empty() {
                return Err(miette::miette!(
                    "no task stores found under {}",
                    self.scan.join(", ")
                ));
            }
            stores
        };

        // All locks live until every store is held, then the command runs.
        let guards: Vec<StoreLock> = stores
            .iter()
            .map(|c| c.lock_store())
            .collect::<Result<_, _>>()?;

        let status = std::process::Command::new(&self.command[0])
            .args(&self.command[1..])
            .status()
            .into_diagnostic()?;
        drop(guards);

        // Propagate the child's status exactly; scripts branch on it.
        std::process::exit(status.code().unwrap_or(1));
    }
}
