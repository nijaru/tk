//! `tk config` — the store's settings.
//!
//! Bare `tk config` shows everything; `tk config set` changes one key. The keys
//! are few enough that a nested subcommand per key (and a sub-subcommand per
//! `clean-after` field) was more interface than data.

use usage::{Args, RunWith, Subcommands};

use crate::cli::AppCtx;
use crate::format;
use crate::ids;
use crate::model::Priority;
use crate::ops::{self, Mutation};

/// Show or change store settings
#[derive(Args)]
pub struct Config {
    #[usage(subcommand)]
    pub command: Option<ConfigCmd>,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum ConfigCmd {
    /// Set one setting: project, priority, labels, or clean-after
    Set(ConfigSet),
    /// Manage directory aliases for -C
    Alias(ConfigAlias),
    /// Rename a project in every task that uses it
    Project(ProjectArgs),
}

impl RunWith<AppCtx> for Config {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(cmd) => cmd.run_with(ctx),
            None => {
                let config = ctx.store.load_config()?;
                let human = format::format_config(&config);
                ctx.emit("config", &config, None, Vec::new(), || human);
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// set
// ---------------------------------------------------------------------------

/// Set one setting
#[derive(Args)]
pub struct ConfigSet {
    /// project | priority | labels | clean-after
    pub key: String,
    /// New value ("off" disables clean-after; an empty labels list clears them)
    pub value: String,
}

const KEYS: &str = "project, priority, labels, clean-after";

impl RunWith<AppCtx> for ConfigSet {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let key = self.key.trim().to_lowercase();
        let value = self.value.trim().to_owned();
        let txn = ctx.store.txn()?;
        let (data, human) = match key.as_str() {
            "project" => {
                ids::validate_project(&value)?;
                let config = txn.update_config(|c| c.project = value.clone())?;
                (
                    serde_json::json!({"project": config.project}),
                    format!("Default project set to {:?}.", config.project),
                )
            }
            "priority" => {
                let priority = Priority::parse(&value)?;
                txn.update_config(|c| c.defaults.priority = priority)?;
                (
                    serde_json::json!({"priority": priority as u8}),
                    format!("Default priority set to {}.", priority.name()),
                )
            }
            "labels" => {
                let labels: Vec<String> = value
                    .split(',')
                    .map(|l| l.trim().to_owned())
                    .filter(|l| !l.is_empty())
                    .collect();
                txn.update_config(|c| c.defaults.labels = labels.clone())?;
                (
                    serde_json::json!({"labels": labels}),
                    if labels.is_empty() {
                        "Default labels cleared.".to_owned()
                    } else {
                        format!("Default labels set to {}.", labels.join(", "))
                    },
                )
            }
            "clean-after" => {
                let (enabled, days) = match value.as_str() {
                    "off" | "false" | "never" | "0" => (false, 0),
                    other => {
                        let days: i64 = other.parse().map_err(|_| {
                            crate::output::invalid(format!(
                                "invalid clean-after value {other:?}: use a number of days or \"off\""
                            ))
                        })?;
                        if days < 0 {
                            return Err(crate::output::invalid(
                                "clean-after must be a non-negative number of days, or \"off\"",
                            ));
                        }
                        (true, days)
                    }
                };
                txn.update_config(|c| {
                    c.clean_after.enabled = enabled;
                    c.clean_after.days = days;
                })?;
                (
                    serde_json::json!({"clean_after": {"enabled": enabled, "days": days}}),
                    if enabled {
                        format!("Auto-clean enabled: archive completed tasks after {days} days.")
                    } else {
                        "Auto-clean disabled.".to_owned()
                    },
                )
            }
            other => {
                return Err(crate::output::invalid(format!(
                    "unknown setting {other:?}: expected one of {KEYS}"
                )));
            }
        };
        ctx.emit("config", &data, None, Vec::new(), || human);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// alias
// ---------------------------------------------------------------------------

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
            let txn = ctx.store.txn()?;
            if self.rm {
                txn.update_config(|c| {
                    if let Some(aliases) = c.aliases.as_mut() {
                        aliases.remove(&name);
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
                let config = txn.update_config(|c| {
                    c.aliases
                        .get_or_insert_default()
                        .insert(name.clone(), path.clone());
                })?;
                let stored = config.aliases.as_ref().and_then(|a| a.get(&name)).cloned();
                let human = format!("Alias {name:?} -> {stored:?} set");
                ctx.emit(
                    "config",
                    &serde_json::json!({"alias": name, "path": stored}),
                    None,
                    Vec::new(),
                    || human,
                );
                return Ok(());
            }
        }
        let aliases = ctx.store.load_config()?.aliases.unwrap_or_default();
        let human = if aliases.is_empty() {
            "No aliases configured.".to_owned()
        } else {
            let mut out = "Aliases:".to_owned();
            for (name, path) in &aliases {
                out.push_str(&format!("\n  {name:<10} -> {path}"));
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

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

/// Rename a project in every task that uses it
#[derive(Args)]
pub struct ProjectArgs {
    #[usage(subcommand)]
    pub command: ProjectCmd,
}

#[derive(Subcommands)]
#[usage(run_with)]
pub enum ProjectCmd {
    /// Rename a project in every task that uses it
    Rename(ProjectRename),
}

impl RunWith<AppCtx> for ProjectArgs {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        self.command.run_with(ctx)
    }
}

/// Rename a project in every task that uses it
///
/// A project is a display field, so this changes one field per task and rewrites
/// no references.
#[derive(Args)]
pub struct ProjectRename {
    /// Current project name
    pub old: String,
    /// New project name
    pub new: String,
}

impl RunWith<AppCtx> for ProjectRename {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        let m = Mutation::locked(&ctx.store)?;
        let result = ops::rename_project(&m, &self.old, &self.new)?;
        let data = serde_json::json!({
            "old": self.old,
            "new": self.new,
            "renamed": result.renamed,
        });
        let human = format!(
            "Renamed project {:?} -> {:?}\n  Updated {} tasks",
            self.old,
            self.new,
            result.renamed.len()
        );
        ctx.emit("config", &data, None, Vec::new(), || human);
        Ok(())
    }
}
