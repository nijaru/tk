//! `tk rm` / `tk init` / `tk mv` / `tk clean` / `tk check` / `tk repair` /
//! `tk path` / `tk lock`

use std::io::BufRead;

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
use crate::store::{self, StoreLock};

/// Delete a task
#[derive(Args)]
pub struct Remove {
    /// Task ID or ref
    pub id: String,
    /// Skip confirmation
    #[usage(short = 'f', long)]
    pub force: bool,
    /// Reject the delete unless the task still has this revision (`show --json`)
    #[usage(long = "if-rev", value_name = "REV")]
    pub if_rev: Option<String>,
}

impl RunWith<AppCtx> for Remove {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        if !self.force {
            let t = txn.load(&id).into_diagnostic()?;
            print!("Delete {} {:?}? [y/N] ", t.id(), t.title);
            use std::io::Write as _;
            std::io::stdout().flush().into_diagnostic()?;
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .into_diagnostic()?;
            match line.trim().to_lowercase().as_str() {
                "y" | "yes" => {}
                _ => {
                    println!("Aborted.");
                    return Ok(());
                }
            }
        }
        let out = txn.remove(&id, self.if_rev.as_deref()).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "deleted": out.id,
                    "references_scrubbed": out.references_scrubbed,
                }))
            );
        } else {
            println!(
                "Deleted task {} (scrubbed {} references)",
                out.id, out.references_scrubbed
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
            return Err(miette::miette!(
                ".tasks directory already exists at {}",
                ctx.store.tasks_dir.display()
            ));
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
        if ctx.store.config_path().exists() {
            return Err(miette::miette!(
                "task store already initialized at {}",
                ctx.store.tasks_dir.display()
            ));
        }
        let config = crate::model::Config {
            project: name,
            ..Default::default()
        };
        txn.save_config(&config).into_diagnostic()?;
        println!(
            "Initialized empty tk project in {}",
            ctx.store.tasks_dir.display()
        );
        Ok(())
    }
}

/// Move a task to a different project
///
/// (Deliberate break from the Go version: `mv` moves tasks only. Renaming a
/// whole project lives under `config project rename`.)
#[derive(Args)]
pub struct Mv {
    /// Task ID or ref
    pub source: String,
    /// Target project name
    pub project: String,
}

impl RunWith<AppCtx> for Mv {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.source).into_diagnostic()?;
        let res = txn.move_task(&id, &self.project).into_diagnostic()?;
        if ctx.json {
            println!(
                "{}",
                format::format_json(&serde_json::json!({
                    "old_id": res.old_id,
                    "new_id": res.new_id,
                    "references_updated": res.references_updated,
                }))
            );
        } else {
            println!(
                "Moved {} -> {} (updated {} references)",
                res.old_id, res.new_id, res.references_updated
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

/// Check task integrity
///
/// Exits non-zero when anything is reported, so scripts and agents cannot
/// mistake findings for success.
#[derive(Args)]
pub struct Check;

impl RunWith<AppCtx> for Check {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.store.require().into_diagnostic()?;
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

/// Repair recorded inconsistencies in a task file
#[derive(Args)]
pub struct Repair {
    /// Task ID or ref
    pub id: String,
    /// Also drop references to missing blocker/parent tasks
    #[usage(long = "drop-missing")]
    pub drop_missing: bool,
}

impl RunWith<AppCtx> for Repair {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn().into_diagnostic()?;
        let id = txn.resolve(&self.id).into_diagnostic()?;
        let out = txn.repair(&id, self.drop_missing).into_diagnostic()?;

        if ctx.json {
            println!("{}", format::format_json(&out));
        } else if out.changed() {
            println!("Repaired {id}:");
            if let Some(was) = &out.renamed_id {
                println!("  rewrote content ID (was {was}) to match the file name");
            }
            if !out.dropped_blockers.is_empty() {
                println!("  dropped blockers: {}", out.dropped_blockers.join(", "));
            }
            if let Some(p) = &out.dropped_parent {
                println!("  dropped parent: {p}");
            }
        } else {
            println!("No repairs needed for {id}.");
        }

        // Remaining issues are reported, never fixed implicitly.
        let remaining = store::inconsistencies(txn.ctx(), &txn.load(&id).into_diagnostic()?, &id);
        if ctx.json {
            if !remaining.is_empty() {
                for issue in &remaining {
                    eprintln!("{}", format::warning(issue, ctx.color));
                }
            }
        } else {
            for issue in &remaining {
                println!("{}", format::warning(issue, ctx.color));
            }
            if !remaining.is_empty() && !self.drop_missing {
                println!(
                    "Run 'tk repair {id} --drop-missing' to drop these references, or restore the missing tasks."
                );
            }
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
