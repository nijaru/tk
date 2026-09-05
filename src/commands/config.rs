//! `tk config` — nested configuration commands.
//!
//! Bare `tk config` (and bare intermediate nodes) show the relevant section,
//! mirroring the old Kong `default:"1"` behavior via `Option` subcommands.

use miette::IntoDiagnostic;
use usage::{Args, RunWith, Subcommands};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
use crate::model::Priority;
use crate::store;

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
        let config = ctx.store.load_config().into_diagnostic()?;
        if ctx.json {
            println!("{}", format::format_json(&config));
        } else {
            println!("{}", format::format_config(&config));
        }
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
        let config = ctx.store.load_config().into_diagnostic()?;
        println!("Default project: {}", config.project);
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
        ids::validate_project(&self.name).into_diagnostic()?;
        let name = self.name.clone();
        let config = ctx
            .store
            .update_config(|c| c.project = name.clone())
            .into_diagnostic()?;
        println!("Default project set to {:?}", config.project);
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
        let res = store::rename_project(&ctx.store, &self.old, &self.new).into_diagnostic()?;
        println!("Renamed project {:?} -> {:?}", self.old, self.new);
        println!("  Renamed {} tasks", res.renamed.len());
        println!("  Updated {} references", res.references_updated);
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
                ctx.store
                    .update_config(|c| {
                        if let Some(a) = c.aliases.as_mut() {
                            a.remove(&name);
                        }
                    })
                    .into_diagnostic()?;
                println!("Removed alias {name:?}");
                return Ok(());
            }
            if let Some(path) = self.path.clone() {
                let config = ctx
                    .store
                    .update_config(|c| {
                        c.aliases
                            .get_or_insert_default()
                            .insert(name.clone(), path.clone());
                    })
                    .into_diagnostic()?;
                let back = config.aliases.as_ref().and_then(|a| a.get(&name));
                println!("Alias {name:?} -> {back:?} set");
                return Ok(());
            }
        }
        let config = ctx.store.load_config().into_diagnostic()?;
        match config.aliases.as_ref().filter(|a| !a.is_empty()) {
            None => println!("No aliases configured."),
            Some(aliases) => {
                println!("Aliases:");
                for (k, v) in aliases {
                    println!("  {k:<10} -> {v}");
                }
            }
        }
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
        let config = ctx.store.load_config().into_diagnostic()?;
        println!("Defaults:");
        println!("  Priority:  {}", config.defaults.priority as u8);
        println!("  Labels:    {:?}", config.defaults.labels);
        println!("  Assignees: {:?}", config.defaults.assignees);
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
        ctx.store
            .update_config(|c| c.defaults.priority = p)
            .into_diagnostic()?;
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
        ctx.store
            .update_config(|c| c.defaults.labels = labels)
            .into_diagnostic()?;
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
        ctx.store
            .update_config(|c| c.defaults.assignees = assignees)
            .into_diagnostic()?;
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
        let config = ctx.store.load_config().into_diagnostic()?;
        let status = if config.clean_after.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("Clean After: {status} ({} days)", config.clean_after.days);
        Ok(())
    }
}

/// Enable auto-clean
#[derive(Args)]
pub struct CleanAfterEnable;

impl RunWith<AppCtx> for CleanAfterEnable {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.store
            .update_config(|c| c.clean_after.enabled = true)
            .into_diagnostic()?;
        Ok(())
    }
}

/// Disable auto-clean
#[derive(Args)]
pub struct CleanAfterDisable;

impl RunWith<AppCtx> for CleanAfterDisable {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.store
            .update_config(|c| c.clean_after.enabled = false)
            .into_diagnostic()?;
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
        ctx.store
            .update_config(|c| c.clean_after.days = days)
            .into_diagnostic()?;
        Ok(())
    }
}
