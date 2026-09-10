//! `tk init` / `tk mv` / `tk clean` / `tk check` / `tk purge` / `tk recover` /
//! `tk path` / `tk lock`

use std::io::{BufRead, IsTerminal as _};

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
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
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        txn.check_rev(&id, self.if_rev.as_deref())
            .into_diagnostic()?;
        let record = txn.load(&id).into_diagnostic()?;

        if !self.force {
            // Never delete unattended: a script or agent that forgets -f gets an
            // error, not a silent removal.
            if !std::io::stdin().is_terminal() {
                return Err(miette::miette!(
                    "refusing to delete {} without -f (stdin is not a terminal)",
                    record.state.alias
                ));
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
                println!("Aborted.");
                return Ok(());
            }
        }

        let out = txn.purge(&id, self.scrub).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "deleted": out.deleted,
                    "references_scrubbed": out.references_scrubbed,
                    "referrers": out.referrers,
                }))
            );
        } else {
            println!(
                "Deleted {} (scrubbed {} references)",
                out.deleted, out.references_scrubbed
            );
        }
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
                Err(e) => Err(e).into_diagnostic(),
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
        ids::validate_project(&name).into_diagnostic()?;
        // Creating the store is deliberate: an explicit --tasks-dir is allowed
        // to come into existence here, and only here.
        let txn = ctx.store.txn_init().into_diagnostic()?;
        let config = crate::model::Config {
            project: name,
            ..Default::default()
        };
        txn.init_store(&config).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "tasks_dir": ctx.store.tasks_dir.display().to_string(),
                    "format": config.format,
                    "project": config.project,
                }))
            );
        } else {
            println!(
                "Initialized empty tk project in {}",
                ctx.store.tasks_dir.display()
            );
        }
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
        ids::validate_project(&self.project).into_diagnostic()?;
        let writer = Writer::new(&ctx, false)?;
        let id = writer.store().resolve(&self.source).into_diagnostic()?;
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
        if ctx.json {
            println!("{}", format::format_json(&t));
        } else {
            println!(
                "Moved {} ({}) to project {}",
                t.task.alias, t.task.id, t.task.project
            );
        }
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
        let config = ctx.store.load_config().into_diagnostic()?;
        let days = if let Some(n) = self.older_than {
            if n < 0 {
                return Err(miette::miette!("--older-than must be non-negative"));
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
            println!(
                "Auto-clean is disabled. Use --older-than N or enable with 'tk config clean-after enable'."
            );
            return Ok(());
        };
        let txn = ctx.store.txn().into_diagnostic()?;
        let out = txn.clean(days, self.purge).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "archived": out.archived,
                    "purged": out.purged,
                    "references_scrubbed": out.references_scrubbed,
                    "days": days,
                }))
            );
        } else if self.purge {
            println!(
                "Purged {} tasks completed more than {days} days ago (scrubbed {} references).",
                out.purged, out.references_scrubbed
            );
        } else {
            println!(
                "Archived {} tasks completed more than {days} days ago. Use --purge to delete them.",
                out.archived
            );
        }
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
        let issues = store::check_integrity(&ctx.store).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "ok": issues.is_empty(),
                    "issues": issues,
                }))
            );
        } else if issues.is_empty() {
            println!("No integrity issues found.");
        } else {
            for issue in &issues {
                println!("{}", format::warning(issue, ctx.color));
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(miette::miette!(
                "integrity check failed: {} issue(s)",
                issues.len()
            ))
        }
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
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = self
            .id
            .map(|input| txn.resolve(&input))
            .transpose()
            .into_diagnostic()?;
        let out = txn.recover(id.as_deref(), self.dry_run).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "repaired": out.repaired,
                    "bytes_dropped": out.bytes_dropped,
                    "dry_run": out.dry_run,
                }))
            );
        } else if out.repaired.is_empty() {
            println!("No torn records found.");
        } else if self.dry_run {
            println!(
                "Would repair {} record(s), dropping {} byte(s): {}",
                out.repaired.len(),
                out.bytes_dropped,
                out.repaired.join(", ")
            );
        } else {
            println!(
                "Repaired {} record(s), dropping {} byte(s): {}",
                out.repaired.len(),
                out.bytes_dropped,
                out.repaired.join(", ")
            );
        }
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
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "tasks_dir": s.tasks_dir.display().to_string(),
                    "root": s.root.display().to_string(),
                    "exists": s.exists,
                    "source": s.source.name(),
                    "worktree": s.worktree,
                }))
            );
        } else {
            println!("{}", s.tasks_dir.display());
        }
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
            return Err(miette::miette!(
                "provide a command after --, for example: tk lock -- git pull --ff-only"
            ));
        }

        let stores: Vec<store::Ctx> = if self.scan.is_empty() {
            ctx.store.require().into_diagnostic()?;
            vec![ctx.store.clone()]
        } else {
            let mut found = Vec::new();
            for dir in &self.scan {
                found.extend(store::find_stores(std::path::Path::new(dir)).into_diagnostic()?);
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
            .collect::<Result<_, _>>()
            .into_diagnostic()?;

        let status = std::process::Command::new(&self.command[0])
            .args(&self.command[1..])
            .status()
            .into_diagnostic()?;
        drop(guards);

        // Propagate the child's status exactly; scripts branch on it.
        std::process::exit(status.code().unwrap_or(1));
    }
}
