//! `tk list` / `tk ready` / `tk show`

use usage::{Args, RunWith};

use crate::cli::AppCtx;
use crate::format;
use crate::model::{EntryView, State};
use crate::store::{Filter, Result};

/// List tasks
#[derive(Args, Debug)]
pub struct List {
    /// Search titles, labels, and status
    #[usage(short = 'q', long)]
    pub search: Option<String>,
    /// Include done and dropped entries
    #[usage(short = 'a', long = "all")]
    pub all: bool,
    /// Only this state: open, done, or dropped
    #[usage(short = 's', long)]
    pub state: Option<String>,
    /// Only this label
    #[usage(short = 'l', long)]
    pub label: Option<String>,
    /// Only blocked entries; --no-blocked for unblocked only
    #[usage(long)]
    pub blocked: bool,
    /// Only unblocked entries
    #[usage(long = "unblocked")]
    pub unblocked: bool,
    /// Show at most this many
    #[usage(short = 'n', long)]
    pub limit: Option<usize>,
}

impl List {
    pub fn filter(&self) -> Result<Filter> {
        let state = match &self.state {
            Some(raw) => Some(State::parse(raw)?),
            None => None,
        };
        Ok(Filter {
            search: self.search.clone().unwrap_or_default(),
            state,
            label: self.label.clone().unwrap_or_default(),
            blocked: match (self.blocked, self.unblocked) {
                (true, true) => {
                    return Err(crate::store::StoreError::InvalidInput(
                        "--blocked and --unblocked are opposites; pick one".into(),
                    ));
                }
                (true, false) => Some(true),
                (false, true) => Some(false),
                (false, false) => None,
            },
            include_closed: self.all,
            limit: self.limit.unwrap_or(0),
        })
    }

    pub fn run(self, ctx: &AppCtx, command: &'static str) -> miette::Result<()> {
        ctx.require_store()?;
        let filter = self.filter()?;
        let store = ctx.store.store()?;
        let (views, issues) = store.list(&filter)?;
        let human = format::render_list(&views, empty_hint(command == "ready"), ctx.color);
        ctx.emit(command, &views, None, issues, || human);
        Ok(())
    }
}

fn empty_hint(ready: bool) -> &'static str {
    if ready {
        "nothing ready: no open, unblocked entries"
    } else {
        "no entries"
    }
}

impl RunWith<AppCtx> for List {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        self.run(&ctx, "list")
    }
}

/// List what can be started now: open, and not waiting on anything
#[derive(Args, Debug, Default)]
pub struct Ready {
    /// Search titles, labels, and status
    #[usage(short = 'q', long)]
    pub search: Option<String>,
    /// Only this label
    #[usage(short = 'l', long)]
    pub label: Option<String>,
    /// Show at most this many
    #[usage(short = 'n', long)]
    pub limit: Option<usize>,
}

impl RunWith<AppCtx> for Ready {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        // The same filter `list` would take, spelled out: open, and nothing in
        // the way. `ready` is not a concept of its own.
        let filter = Filter {
            search: self.search.clone().unwrap_or_default(),
            state: Some(State::Open),
            label: self.label.clone().unwrap_or_default(),
            blocked: Some(false),
            include_closed: false,
            limit: self.limit.unwrap_or(0),
        };
        let store = ctx.store.store()?;
        let (views, issues) = store.list(&filter)?;
        let human = format::render_list(&views, empty_hint(true), ctx.color);
        ctx.emit("ready", &views, None, issues, || human);
        Ok(())
    }
}

/// Show one entry
#[derive(Args, Debug)]
pub struct Show {
    /// Ref, or part of a title
    pub r#ref: String,
}

impl Show {
    pub fn view(ctx: &AppCtx, input: &str) -> Result<EntryView> {
        ctx.store.store()?.get(input)
    }
}

impl RunWith<AppCtx> for Show {
    type Output = miette::Result<()>;

    fn run_with(self, ctx: AppCtx) -> Self::Output {
        ctx.require_store()?;
        let view = Show::view(&ctx, &self.r#ref)?;
        let issues: Vec<String> = view
            .unresolved_blockers
            .iter()
            .map(|b| format!("{b} is named as a blocker but is not in this store"))
            .collect();
        let rev = Some(view.rev.clone());
        ctx.emit("show", &view, rev, issues, || {
            format::render_detail(&view, ctx.color)
        });
        Ok(())
    }
}
