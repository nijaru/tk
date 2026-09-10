//! `tk config` — nested configuration commands.
//!
//! Bare `tk config` (and bare intermediate nodes) show the relevant section,
//! mirroring the old Kong `default:"1"` behavior via `Option` subcommands.

use usage::{Args, RunWith, Subcommands};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
use crate::model::Priority;

/// Show or set configuration
#[derive(Args)]
pub struct Config {
    #[usage(subcommand)]
    pub command: Option<ConfigCmd>,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum ConfigCmd {
    /// Show configuration
    Show(ConfigShow),
    /// Get or set the default project
    Project(ProjectArgs),
    /// Manage directory aliases for -C
    Alias(ConfigAlias),
    /// Show or set default values
    Defaults(DefaultsArgs),
    /// Configure auto-cleanup
    #[usage(name = "clean-after")]
    CleanAfter(CleanAfterArgs),
}

impl RunWith<AppCtx> for Config {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(cmd) => cmd.run_with(ctx),
            None => ConfigShow.run_with(ctx),
        }
    }
}

/// Show configuration
#[derive(Args)]
pub struct ConfigShow;

impl RunWith<AppCtx> for ConfigShow {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let config = ctx.store.load_config()?;
        let human = format::format_config(&config);
        ctx.emit("config", &config, None, Vec::new(), || human);
        Ok(())
    }
}

// --- project ---

/// Get or set the default project
#[derive(Args)]
pub struct ProjectArgs {
    #[usage(subcommand)]
    pub command: Option<ProjectCmd>,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum ProjectCmd {
    /// Show default project
    Show(ProjectShow),
    /// Set default project
    Set(ProjectSet),
    /// Rename project and all its tasks
    Rename(ProjectRename),
}

impl RunWith<AppCtx> for ProjectArgs {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(cmd) => cmd.run_with(ctx),
            None => ProjectShow.run_with(ctx),
        }
    }
}

/// Show default project
#[derive(Args)]
pub struct ProjectShow;

impl RunWith<AppCtx> for ProjectShow {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let config = ctx.store.load_config()?;
        let human = format!("Default project: {}", config.project);
        ctx.emit(
            "config",
            &serde_json::json!({"project": config.project}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// Set default project
#[derive(Args)]
pub struct ProjectSet {
    /// Project name to set
    pub name: String,
}

impl RunWith<AppCtx> for ProjectSet {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ids::validate_project(&self.name)?;
        let name = self.name.clone();
        let txn = ctx.store.txn()?;
        let config = txn.update_config(|c| c.project = name.clone())?;
        let human = format!("Default project set to {:?}", config.project);
        ctx.emit(
            "config",
            &serde_json::json!({"project": config.project}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// Rename project and all its tasks
#[derive(Args)]
pub struct ProjectRename {
    /// Old project name
    pub old: String,
    /// New project name
    pub new: String,
}

impl RunWith<AppCtx> for ProjectRename {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn()?;
        let res = txn.rename_project(&self.old, &self.new)?;
        let data = serde_json::json!({
            "old": self.old,
            "new": self.new,
            "renamed": res.renamed,
        });
        let human = format!(
            "Renamed project {:?} -> {:?}\n  Updated {} tasks",
            self.old,
            self.new,
            res.renamed.len()
        );
        ctx.emit("config", &data, None, Vec::new(), || human);
        Ok(())
    }
}

// --- alias ---

/// Manage directory aliases for -C
#[derive(Args)]
pub struct ConfigAlias {
    /// Alias name
    pub name: Option<String>,
    /// Directory path
    pub path: Option<String>,
    /// Remove the alias
    #[usage(short = 'r', long = "rm")]
    pub rm: bool,
}

impl RunWith<AppCtx> for ConfigAlias {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if let Some(name) = self.name.clone() {
            if self.rm {
                let txn = ctx.store.txn()?;
                txn.update_config(|c| {
                    if let Some(a) = c.aliases.as_mut() {
                        a.remove(&name);
                    }
                })?;
                let human = format!("Removed alias {name:?}");
                ctx.emit(
                    "config",
                    &serde_json::json!({"removed": name}),
                    None,
                    Vec::new(),
                    || human,
                );
                return Ok(());
            }
            if let Some(path) = self.path.clone() {
                let txn = ctx.store.txn()?;
                let config = txn.update_config(|c| {
                    c.aliases
                        .get_or_insert_default()
                        .insert(name.clone(), path.clone());
                })?;
                let back = config.aliases.as_ref().and_then(|a| a.get(&name)).cloned();
                let human = format!("Alias {name:?} -> {back:?} set");
                ctx.emit(
                    "config",
                    &serde_json::json!({"alias": name, "path": back}),
                    None,
                    Vec::new(),
                    || human,
                );
                return Ok(());
            }
        }
        let config = ctx.store.load_config()?;
        let aliases = config.aliases.clone().unwrap_or_default();
        let human = if aliases.is_empty() {
            "No aliases configured.".to_owned()
        } else {
            let mut out = "Aliases:".to_owned();
            for (k, v) in &aliases {
                out.push_str(&format!("\n  {k:<10} -> {v}"));
            }
            out
        };
        ctx.emit(
            "config",
            &serde_json::json!({"aliases": aliases}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

// --- defaults ---

/// Show or set default values
#[derive(Args)]
pub struct DefaultsArgs {
    #[usage(subcommand)]
    pub command: Option<DefaultsCmd>,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum DefaultsCmd {
    /// Show default values
    Show(DefaultsShow),
    /// Set default priority
    Priority(DefaultsPriority),
    /// Set default labels
    Labels(DefaultsLabels),
    /// Set default assignees
    Assignees(DefaultsAssignees),
}

impl RunWith<AppCtx> for DefaultsArgs {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(cmd) => cmd.run_with(ctx),
            None => DefaultsShow.run_with(ctx),
        }
    }
}

/// Show default values
#[derive(Args)]
pub struct DefaultsShow;

impl RunWith<AppCtx> for DefaultsShow {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let config = ctx.store.load_config()?;
        let human = format!(
            "Defaults:\n  Priority:  {}\n  Labels:    {:?}\n  Assignees: {:?}",
            config.defaults.priority as u8, config.defaults.labels, config.defaults.assignees
        );
        ctx.emit(
            "config",
            &serde_json::json!({"defaults": config.defaults}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// Set default priority
#[derive(Args)]
pub struct DefaultsPriority {
    /// Default priority level (0-4)
    pub level: u8,
}

impl RunWith<AppCtx> for DefaultsPriority {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let p =
            Priority::from_u8(self.level).ok_or_else(|| miette::miette!("priority must be 0-4"))?;
        let txn = ctx.store.txn()?;
        txn.update_config(|c| c.defaults.priority = p)?;
        let human = format!("Default priority set to {}", p.short());
        ctx.emit(
            "config",
            &serde_json::json!({"priority": p as u8}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// Set default labels
#[derive(Args)]
pub struct DefaultsLabels {
    /// Default labels (comma-separated)
    #[usage(delimiter = ',')]
    pub labels: Vec<String>,
}

impl RunWith<AppCtx> for DefaultsLabels {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let labels = self.labels.clone();
        let txn = ctx.store.txn()?;
        txn.update_config(|c| c.defaults.labels = labels.clone())?;
        let human = format!("Default labels set to {labels:?}");
        ctx.emit(
            "config",
            &serde_json::json!({"labels": labels}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// Set default assignees
#[derive(Args)]
pub struct DefaultsAssignees {
    /// Default assignees (comma-separated)
    #[usage(delimiter = ',')]
    pub assignees: Vec<String>,
}

impl RunWith<AppCtx> for DefaultsAssignees {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let assignees = self.assignees.clone();
        let txn = ctx.store.txn()?;
        txn.update_config(|c| c.defaults.assignees = assignees.clone())?;
        let human = format!("Default assignees set to {assignees:?}");
        ctx.emit(
            "config",
            &serde_json::json!({"assignees": assignees}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

// --- clean-after ---

/// Configure auto-cleanup
#[derive(Args)]
pub struct CleanAfterArgs {
    #[usage(subcommand)]
    pub command: Option<CleanAfterCmd>,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum CleanAfterCmd {
    /// Show clean-after config
    Show(CleanAfterShow),
    /// Enable auto-clean
    Enable(CleanAfterEnable),
    /// Disable auto-clean
    Disable(CleanAfterDisable),
    /// Set clean-after days
    Days(CleanAfterDays),
}

impl RunWith<AppCtx> for CleanAfterArgs {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(cmd) => cmd.run_with(ctx),
            None => CleanAfterShow.run_with(ctx),
        }
    }
}

/// Show clean-after config
#[derive(Args)]
pub struct CleanAfterShow;

impl RunWith<AppCtx> for CleanAfterShow {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let config = ctx.store.load_config()?;
        let status = if config.clean_after.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let human = format!("Clean After: {status} ({} days)", config.clean_after.days);
        ctx.emit(
            "config",
            &serde_json::json!({"clean_after": config.clean_after}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}

/// Enable auto-clean
#[derive(Args)]
pub struct CleanAfterEnable;

impl RunWith<AppCtx> for CleanAfterEnable {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn()?;
        txn.update_config(|c| c.clean_after.enabled = true)?;
        ctx.emit(
            "config",
            &serde_json::json!({"clean_after": {"enabled": true}}),
            None,
            Vec::new(),
            || "Auto-clean enabled.".to_owned(),
        );
        Ok(())
    }
}

/// Disable auto-clean
#[derive(Args)]
pub struct CleanAfterDisable;

impl RunWith<AppCtx> for CleanAfterDisable {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let txn = ctx.store.txn()?;
        txn.update_config(|c| c.clean_after.enabled = false)?;
        ctx.emit(
            "config",
            &serde_json::json!({"clean_after": {"enabled": false}}),
            None,
            Vec::new(),
            || "Auto-clean disabled.".to_owned(),
        );
        Ok(())
    }
}

/// Set clean-after days
#[derive(Args)]
pub struct CleanAfterDays {
    /// Days after which to clean completed tasks
    pub days: i64,
}

impl RunWith<AppCtx> for CleanAfterDays {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if self.days < 0 {
            return Err(miette::miette!("days must be >= 0"));
        }
        let days = self.days;
        let txn = ctx.store.txn()?;
        txn.update_config(|c| c.clean_after.days = days)?;
        let human = format!("Clean-after days set to {days}.");
        ctx.emit(
            "config",
            &serde_json::json!({"clean_after": {"days": days}}),
            None,
            Vec::new(),
            || human,
        );
        Ok(())
    }
}
