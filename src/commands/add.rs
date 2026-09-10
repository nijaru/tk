//! Creating entries, and creating the store.

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::ops;
use crate::store::Result;

/// Initialize .tasks/ here
#[derive(Args, Debug)]
pub struct Init {
    /// Initialize even if a store already exists here
    #[usage(long)]
    pub force: bool,
}

impl RunWith<AppCtx> for Init {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        if ctx.store.exists && !self.force {
            let existing = ctx.store.tasks_dir.display().to_string();
            return Err(ctx.fail(
                "init",
                crate::output::code::INVALID_INPUT,
                &format!("a store already exists at {existing}"),
                &serde_json::Value::Null,
                Vec::new(),
            ));
        }
        ctx.store.txn_init()?;
        let path = ctx.store.tasks_dir.display().to_string();
        ctx.emit(
            "init",
            &serde_json::json!({ "store": path, "format": crate::store::FORMAT }),
            None,
            Vec::new(),
            || format!("initialized {path}/"),
        );
        Ok(())
    }
}

/// Create a task
#[derive(Args, Debug)]
pub struct Add {
    /// What needs doing
    pub title: String,
    /// Labels, comma-separated
    #[usage(short = 'l', long, delimiter = ',')]
    pub label: Vec<String>,
    /// A ref this task waits on
    #[usage(short = 'b', long = "blocked-by", value_name = "REF")]
    pub blocked_by: Vec<String>,
    /// Print only the new ref
    #[usage(short = 'q', long)]
    pub quiet: bool,
}

impl Add {
    /// Build the entry, then apply everything else that came with it.
    pub fn apply(self, txn: &crate::store::Txn<'_>, now: &str) -> Result<crate::model::EntryView> {
        let mut view = ops::create(txn, &self.title, now)?;
        let edit = ops::Edit {
            labels: (!self.label.is_empty()).then(|| self.label.clone()),
            ..Default::default()
        };
        if !edit.is_empty() {
            view = ops::apply_edit(txn, &view.entry.r#ref, &edit, now)?;
        }
        for blocker in &self.blocked_by {
            view = ops::add_blocker(txn, &view.entry.r#ref, blocker, now)?;
        }
        Ok(view)
    }
}

impl RunWith<AppCtx> for Add {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let now = ctx.now();
        let quiet = self.quiet;
        let view = {
            let txn = ctx.store.txn()?;
            self.apply(&txn, &now)?
        };
        let new_ref = view.entry.r#ref.clone();
        let rev = view.rev.clone();
        let title = format::truncate(&view.entry.title, 70);
        ctx.emit("add", &view, Some(rev), Vec::new(), move || {
            if quiet {
                new_ref
            } else {
                format!("{new_ref}  {title}")
            }
        });
        Ok(())
    }
}
