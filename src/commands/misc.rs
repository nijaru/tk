//! `tk rm` / `tk init` / `tk mv` / `tk clean` / `tk check`

use std::io::BufRead;

use miette::IntoDiagnostic;
use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
use crate::store;

use super::resolve;

/// Delete a task
#[derive(Args)]
pub struct Remove {
    /// Task ID or ref
    pub id: String,
    /// Skip confirmation
    #[usage(short = 'f', long)]
    pub force: bool,
}

impl RunWith<AppCtx> for Remove {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let id = resolve(&ctx, &self.id)?;
        if !self.force {
            let (t, _) = store::get_task(&ctx.store, &id).into_diagnostic()?;
            print!("Delete {} {:?}? [y/N] ", t.id, t.task.title);
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
        store::delete_task(&ctx.store, &id).into_diagnostic()?;
        println!("Deleted task {id}");
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
        std::fs::create_dir_all(&ctx.store.tasks_dir).into_diagnostic()?;
        let config = crate::model::Config {
            project: name,
            ..Default::default()
        };
        ctx.store.save_config(&config).into_diagnostic()?;
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
        let id = resolve(&ctx, &self.source)?;
        let res = store::move_task(&ctx.store, &id, &self.project).into_diagnostic()?;
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
        let n = store::clean_tasks(&ctx.store, days).into_diagnostic()?;
        println!("Cleaned {n} tasks completed more than {days} days ago.");
        Ok(())
    }
}

/// Check task integrity
#[derive(Args)]
pub struct Check;

impl RunWith<AppCtx> for Check {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let issues = store::check_integrity(&ctx.store).into_diagnostic()?;
        if issues.is_empty() {
            println!("No integrity issues found.");
        } else {
            for issue in &issues {
                println!("{}", format::warning(issue, ctx.color));
            }
        }
        Ok(())
    }
}
