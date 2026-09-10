//! `tk config`: show the store file, and name directories.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;

/// Show or change configuration
#[derive(Args, Debug)]
pub struct Config {
    #[usage(subcommand)]
    pub command: Option<ConfigCommand>,
}

#[derive(usage::Subcommands, Debug)]
#[usage(run_with)]
pub enum ConfigCommand {
    /// Name a directory so -C can find it
    Alias(Alias),
}

/// Name a directory so -C can find it
#[derive(Args, Debug)]
pub struct Alias {
    /// The name to type after -C
    pub name: String,
    /// The directory it points at; omit to remove the alias
    pub path: Option<String>,
}

impl RunWith<AppCtx> for Config {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        match self.command {
            Some(ConfigCommand::Alias(alias)) => alias.run_with(ctx),
            None => {
                ctx.require_store()?;
                let store = ctx.store.store()?;
                let config = store.config()?;
                let entries = store.scan()?.entries.len();
                let data = serde_json::json!({
                    "store": ctx.store.tasks_dir.display().to_string(),
                    "format": config.format,
                    "aliases": config.aliases.clone().unwrap_or_default(),
                    "entries": entries,
                });
                ctx.emit("config", &data, None, Vec::new(), || {
                    format::render_config(&ctx.store, &config, entries)
                });
                Ok(())
            }
        }
    }
}

impl RunWith<AppCtx> for Alias {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let name = self.name.clone();
        let path = self.path.clone();
        let config = {
            let txn = ctx.store.txn()?;
            txn.update_config(|config| {
                let aliases = config.aliases.get_or_insert_with(Default::default);
                match &path {
                    Some(path) => {
                        aliases.insert(name.clone(), path.clone());
                    }
                    None => {
                        aliases.remove(&name);
                    }
                }
                if aliases.is_empty() {
                    config.aliases = None;
                }
            })?
        };
        let aliases = config.aliases.clone().unwrap_or_default();
        let removed = path.is_none();
        let message = match (&path, removed) {
            (Some(path), _) => format!("{name} -> {path}"),
            (None, true) => format!("removed {name}"),
            (None, false) => unreachable!(),
        };
        ctx.emit(
            "config",
            &serde_json::json!({ "aliases": aliases }),
            None,
            Vec::new(),
            move || message,
        );
        Ok(())
    }
}
