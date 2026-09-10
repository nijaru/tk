//! Filesystem store: location resolution, the format gate, the mutation lock,
//! and every task operation.
//!
//! Two entry points, and the difference between them is the point:
//!
//! - [`Ctx::store`] gives a [`Store`]: reads, plus appends that are
//!   last-writer-wins or commutative. No lock, so `tk log` from one agent never
//!   blocks another.
//! - [`Ctx::txn`] gives a [`Txn`]: holds the store lock and additionally
//!   exposes the operations that must validate *other* records (`block`),
//!   enforce uniqueness (`create`), read-then-write one record conditionally
//!   (`--if-rev`), or touch several records (`rename_project`, `purge
//!   --scrub`, `clean --purge`, `recover`).
//!
//! Composite operations live only on `Txn`, so the lock cannot be forgotten by
//! accident: there is no unlocked path to them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::ids::{self, IdError};
use crate::model::{self, Config, Priority, Status, TaskState, TaskView};
use crate::output::code;
use crate::record::{self, Event, Record, op};
use crate::timeutil;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("no .tasks/ directory found. Run 'tk init' to create one.\nSearched from: {0}")]
    TasksNotFound(String),
    #[error(
        "task store not found: {path}\n{origin} was set explicitly, so tk will not create a store there.\nCreate it deliberately with 'tk --tasks-dir {path} init', or point at an existing store."
    )]
    ExplicitStoreMissing { path: String, origin: String },
    #[error(
        "not a tk v1 store at {path}\n  {detail}\nThis binary reads only format {format} stores. Convert a legacy store with tools/migrate-v0.py, or create a fresh one with 'tk init'."
    )]
    NotV1Store {
        path: String,
        detail: String,
        format: i64,
    },
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error(
        "task {id} changed since it was read (expected revision {expected}, found {found}); re-read the task and retry"
    )]
    StaleRevision {
        id: String,
        expected: String,
        found: String,
    },
    #[error("task title cannot be empty")]
    EmptyTitle,
    /// The request or command line itself is wrong (bad batch intent, a guard
    /// that needs a flag). Distinct from a store failure, so a caller can fix
    /// its input rather than retry.
    #[error("{0}")]
    InvalidInput(String),
    #[error("could not allocate a unique alias after {0} attempts")]
    AliasCollisions(u32),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {what}: {err}")]
    Parse { what: String, err: String },
    #[error("{0}")]
    Id(#[from] IdError),
    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl miette::Diagnostic for StoreError {
    /// Lets a `--json` failure carry its kind through miette's report wrapper.
    fn code(&self) -> Option<Box<dyn std::fmt::Display + '_>> {
        Some(Box::new(StoreError::code(self)))
    }
}

impl StoreError {
    /// Stable machine-readable kind, used as the JSON envelope's `error_code`
    /// so a caller can branch on the failure without reading prose.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TasksNotFound(_) | Self::ExplicitStoreMissing { .. } => code::STORE_NOT_FOUND,
            Self::NotV1Store { .. } => code::NOT_A_V1_STORE,
            Self::TaskNotFound(_) => code::NOT_FOUND,
            Self::StaleRevision { .. } => code::STALE_REVISION,
            Self::EmptyTitle | Self::AliasCollisions(_) => code::INVALID_INPUT,
            Self::InvalidInput(_) => code::INVALID_INPUT,
            Self::Io(_) => code::IO,
            Self::Parse { .. } => code::PARSE,
            Self::Id(IdError::NotFound(_)) => code::NOT_FOUND,
            Self::Id(IdError::Ambiguous { .. }) => code::AMBIGUOUS,
            Self::Id(_) => code::INVALID_INPUT,
            Self::Msg(_) => code::ERROR,
        }
    }
}

fn parse_err(what: impl Into<String>, err: impl ToString) -> StoreError {
    StoreError::Parse {
        what: what.into(),
        err: err.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

pub const TASKS_DIR: &str = ".tasks";
const STORE_FILE: &str = "store.json";
const LEGACY_CONFIG_FILE: &str = "config.json";
const RECORDS_DIR: &str = "records";
const LOCK_FILE: &str = ".lock";

/// Environment variable naming the task store directory (see `--tasks-dir`).
pub const TASKS_DIR_ENV: &str = "TK_TASKS_DIR";

/// Keep lock and temp artifacts out of Git for stores that are tracked.
///
/// Written once, when the store is created; an existing `.gitignore` is never
/// touched.
fn ensure_gitignore(tasks_dir: &Path) {
    let path = tasks_dir.join(".gitignore");
    if path.exists() {
        return;
    }
    let _ = fs::write(&path, "# tk lock artifacts\n.lock\n.tmp.*\n");
}

/// How the store location was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreSource {
    /// Walked up from the working directory to the nearest `.tasks`/`.git`.
    Discovered,
    /// `--tasks-dir`: exact path, no walking, never created implicitly.
    Flag,
    /// `TK_TASKS_DIR`: same strict contract as the flag.
    Env,
}

impl StoreSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Flag => "flag",
            Self::Env => "env",
        }
    }

    /// An explicitly designated store must already exist.
    pub fn is_explicit(self) -> bool {
        matches!(self, Self::Flag | Self::Env)
    }

    fn origin(self) -> &'static str {
        match self {
            Self::Flag => "--tasks-dir",
            Self::Env => TASKS_DIR_ENV,
            Self::Discovered => "discovery",
        }
    }
}

/// Resolved filesystem location. Build once per invocation and pass around.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub cwd: PathBuf,
    pub root: PathBuf,
    pub tasks_dir: PathBuf,
    pub exists: bool,
    pub source: StoreSource,
    /// Discovery stopped at a `.git` *file*: a linked worktree or submodule.
    pub worktree: bool,
}

impl Ctx {
    /// Resolve the store from an optional working directory and an optional
    /// explicit store path (flag first, then `TK_TASKS_DIR`, then discovery).
    pub fn resolve(dir: Option<&str>, tasks_dir: Option<&str>) -> Result<Self> {
        if let Some(path) = tasks_dir {
            return Self::explicit(path, StoreSource::Flag);
        }
        if let Some(path) = std::env::var_os(TASKS_DIR_ENV).filter(|v| !v.is_empty()) {
            return Self::explicit(&path.to_string_lossy(), StoreSource::Env);
        }
        Self::discover(dir)
    }

    /// Walk up from `dir` (or the process cwd) looking for `.tasks/` or `.git/`.
    pub fn discover(dir: Option<&str>) -> Result<Self> {
        let cwd = match dir {
            Some(d) => PathBuf::from(d),
            None => std::env::current_dir().map_err(StoreError::Io)?,
        };
        let cwd = Self::absolutize(&cwd)?;
        let mut ctx = Self::walk(&cwd);
        // Resolve `-C` against directory aliases (mirrors the Go behavior).
        if let Some(d) = dir
            && let Some(alias_target) = ctx.read_alias(d)
        {
            let resolved = if Path::new(&alias_target).is_absolute() {
                PathBuf::from(&alias_target)
            } else {
                ctx.root.join(&alias_target)
            };
            ctx = Self::walk(&resolved);
        }
        Ok(ctx)
    }

    /// Exact store directory: no walking, never created implicitly.
    fn explicit(path: &str, source: StoreSource) -> Result<Self> {
        let tasks_dir = Self::absolutize(Path::new(path))?;
        let root = tasks_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| tasks_dir.clone());
        let cwd = std::env::current_dir()
            .map(|p| Self::absolutize(&p).unwrap_or(p))
            .unwrap_or_else(|_| tasks_dir.clone());
        Ok(Self {
            cwd,
            root,
            exists: tasks_dir.is_dir(),
            tasks_dir,
            source,
            worktree: false,
        })
    }

    fn absolutize(p: &Path) -> Result<PathBuf> {
        if p.is_absolute() {
            return Ok(p.to_path_buf());
        }
        let cwd = std::env::current_dir().map_err(StoreError::Io)?;
        Ok(cwd.join(p))
    }

    fn walk(start: &Path) -> Self {
        let mut cur = start.to_path_buf();
        loop {
            if cur.join(TASKS_DIR).is_dir() {
                return Self {
                    cwd: start.to_path_buf(),
                    tasks_dir: cur.join(TASKS_DIR),
                    root: cur,
                    exists: true,
                    source: StoreSource::Discovered,
                    worktree: false,
                };
            }
            let git = cur.join(".git");
            if git.is_file() {
                // Linked worktree or submodule: deliberately do not look at the
                // parent checkout's store from here.
                return Self {
                    cwd: start.to_path_buf(),
                    tasks_dir: cur.join(TASKS_DIR),
                    root: cur,
                    exists: false,
                    source: StoreSource::Discovered,
                    worktree: true,
                };
            }
            if git.exists() {
                return Self {
                    cwd: start.to_path_buf(),
                    tasks_dir: cur.join(TASKS_DIR),
                    root: cur,
                    exists: false,
                    source: StoreSource::Discovered,
                    worktree: false,
                };
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
        Self {
            cwd: start.to_path_buf(),
            tasks_dir: start.join(TASKS_DIR),
            root: start.to_path_buf(),
            exists: false,
            source: StoreSource::Discovered,
            worktree: false,
        }
    }

    fn read_alias(&self, name: &str) -> Option<String> {
        if !self.exists {
            return None;
        }
        let data = fs::read(self.store_path()).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&data).ok()?;
        v.get("aliases")?.get(name)?.as_str().map(str::to_owned)
    }

    /// Why this store is missing, phrased for the way it was selected.
    pub fn missing_store_error(&self) -> StoreError {
        if self.source.is_explicit() {
            StoreError::ExplicitStoreMissing {
                path: self.tasks_dir.display().to_string(),
                origin: self.source.origin().to_owned(),
            }
        } else {
            StoreError::TasksNotFound(self.cwd.display().to_string())
        }
    }

    // -- layout ------------------------------------------------------------

    pub fn store_path(&self) -> PathBuf {
        self.tasks_dir.join(STORE_FILE)
    }

    pub fn legacy_config_path(&self) -> PathBuf {
        self.tasks_dir.join(LEGACY_CONFIG_FILE)
    }

    pub fn records_dir(&self) -> PathBuf {
        self.tasks_dir.join(RECORDS_DIR)
    }

    pub fn record_path(&self, id: &str) -> PathBuf {
        self.records_dir().join(format!("{id}.jsonl"))
    }

    pub fn lock_path(&self) -> PathBuf {
        self.tasks_dir.join(LOCK_FILE)
    }

    /// Build a context for an already-resolved store directory (no discovery).
    pub fn at_tasks_dir(tasks_dir: PathBuf) -> Self {
        let root = tasks_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| tasks_dir.clone());
        let exists = tasks_dir.is_dir();
        Self {
            cwd: tasks_dir.clone(),
            root,
            tasks_dir,
            exists,
            source: StoreSource::Flag,
            worktree: false,
        }
    }

    // -- the format gate ---------------------------------------------------

    /// Require an existing store this binary can read.
    ///
    /// Every read and write path goes through here, so a legacy store is
    /// refused with an explanation instead of being read as empty and then
    /// written over.
    pub fn require(&self) -> Result<()> {
        if !self.exists {
            return Err(self.missing_store_error());
        }
        self.check_format()
    }

    /// Guard read commands: a store selected explicitly must exist, while an
    /// undiscovered store simply has nothing to show.
    pub fn require_for_read(&self) -> Result<()> {
        if !self.tasks_dir.is_dir() {
            if self.source.is_explicit() {
                return Err(self.missing_store_error());
            }
            return Ok(());
        }
        self.check_format()
    }

    /// Refuse anything that is not a `format: 2` store, naming the reason.
    pub fn check_format(&self) -> Result<()> {
        match fs::read(self.store_path()) {
            Ok(data) => match serde_json::from_slice::<Config>(&data) {
                Ok(config) if config.format == model::FORMAT => Ok(()),
                Ok(config) => Err(self.not_v1(format!(
                    "store.json declares format {}, but this binary writes format {}",
                    config.format,
                    model::FORMAT
                ))),
                Err(e) => Err(self.not_v1(format!("store.json could not be parsed: {e}"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(self.not_v1(self.describe_non_v1()))
            }
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    fn not_v1(&self, detail: String) -> StoreError {
        StoreError::NotV1Store {
            path: self.tasks_dir.display().to_string(),
            detail,
            format: model::FORMAT,
        }
    }

    /// Why this directory is not a v1 store, said specifically.
    fn describe_non_v1(&self) -> String {
        if self.legacy_config_path().exists() {
            return format!(
                "found {LEGACY_CONFIG_FILE}, the configuration file of the previous layout \
                 (one JSON file per task at the top level)"
            );
        }
        let strays = self.top_level_task_files();
        if !strays.is_empty() {
            let mut shown: Vec<String> = strays.iter().take(3).cloned().collect();
            if strays.len() > 3 {
                shown.push(format!("and {} more", strays.len() - 3));
            }
            return format!(
                "found task files from the previous layout at the top level: {}",
                shown.join(", ")
            );
        }
        format!("{STORE_FILE} is missing")
    }

    /// Top-level `*.json` files other than `store.json`.
    fn top_level_task_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.tasks_dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") && name != STORE_FILE && entry.path().is_file() {
                out.push(name);
            }
        }
        out.sort();
        out
    }

    // -- config ------------------------------------------------------------

    pub fn load_config(&self) -> Result<Config> {
        self.require()?;
        let data = fs::read(self.store_path()).map_err(StoreError::Io)?;
        let mut config: Config = serde_json::from_slice(&data)
            .map_err(|e| self.not_v1(format!("store.json could not be parsed: {e}")))?;
        if config.project.is_empty() {
            config.project = "tk".to_owned();
        }
        Ok(config)
    }

    fn write_config(&self, config: &Config) -> Result<()> {
        let data =
            serde_json::to_string_pretty(config).map_err(|e| parse_err("marshal store.json", e))?;
        atomic_write(&self.store_path(), data.as_bytes())
    }

    // -- entry points ------------------------------------------------------

    /// Read side plus lock-free appends.
    pub fn store(&self) -> Result<Store<'_>> {
        self.require()?;
        Ok(Store { ctx: self })
    }

    /// Mutation boundary for operations that must see a consistent store.
    ///
    /// Do not open two transactions against one store in a process: the
    /// advisory lock is not reentrant across file handles.
    pub fn txn(&self) -> Result<Txn<'_>> {
        self.require()?;
        let lock = self.lock_mutation()?;
        Ok(Txn {
            store: Store { ctx: self },
            _lock: lock,
        })
    }

    /// Create the store deliberately. The only path that may do so.
    pub fn txn_init(&self) -> Result<Txn<'_>> {
        fs::create_dir_all(&self.tasks_dir).map_err(StoreError::Io)?;
        ensure_gitignore(&self.tasks_dir);
        fs::create_dir_all(self.records_dir()).map_err(StoreError::Io)?;
        let lock = self.lock_mutation()?;
        Ok(Txn {
            store: Store { ctx: self },
            _lock: lock,
        })
    }

    /// Acquire just this store's mutation lock, without a transaction.
    pub fn lock_store(&self) -> Result<StoreLock> {
        self.lock_mutation()
    }

    fn lock_mutation(&self) -> Result<StoreLock> {
        fs::create_dir_all(&self.tasks_dir).map_err(StoreError::Io)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(self.lock_path())
            .map_err(StoreError::Io)?;
        file.lock().map_err(StoreError::Io)?;
        Ok(StoreLock { file })
    }

    /// Write `store.json` for a new store.
    fn init_config(&self, config: &Config) -> Result<()> {
        if self.store_path().exists() {
            return Err(StoreError::Msg(format!(
                "task store already initialized at {}",
                self.tasks_dir.display()
            )));
        }
        self.write_config(config)
    }
}

// ---------------------------------------------------------------------------
// Mutation guard
// ---------------------------------------------------------------------------

/// Advisory exclusive lock over one store's mutation boundary.
///
/// Serializes cooperating `tk` processes for operations that read other records
/// or read-then-write one record. It does not constrain editors, a Git
/// checkout, or an older `tk` binary, and it cannot coordinate separate clones
/// or machines.
pub struct StoreLock {
    file: fs::File,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

// ---------------------------------------------------------------------------
// Store: reads and lock-free appends
// ---------------------------------------------------------------------------

/// Read and lock-free-append operations on a store.
pub struct Store<'a> {
    ctx: &'a Ctx,
}

/// Everything a fold needs, plus the problems found while reading.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub records: Vec<Record>,
    /// Record-level problems (unreadable files, bad names, duplicate aliases).
    /// Per-record line damage stays on the [`Record`].
    pub issues: Vec<String>,
}

impl Snapshot {
    /// Alias and status for every record, so references render as handles.
    pub fn index(&self) -> Index {
        Index::from_records(&self.records)
    }

    pub fn by_id(&self) -> HashMap<&str, &Record> {
        self.records.iter().map(|r| (r.id.as_str(), r)).collect()
    }
}

/// What enrichment needs to describe a *referenced* task.
#[derive(Debug, Default)]
pub struct Index {
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub alias: String,
    pub status: Status,
}

impl Index {
    pub fn from_records(records: &[Record]) -> Self {
        Self {
            entries: records
                .iter()
                .map(|r| {
                    (
                        r.id.clone(),
                        Entry {
                            alias: r.state.alias.clone(),
                            status: r.state.status,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Look up a referenced task, reading its record when the index is cold.
    ///
    /// A single-task view should not pay for a whole-store scan, and a list
    /// should not re-read every blocker it already has in memory.
    fn lookup(&self, ctx: &Ctx, id: &str) -> Option<Entry> {
        if let Some(entry) = self.entries.get(id) {
            return Some(entry.clone());
        }
        let record = record::read(&ctx.record_path(id)).ok()?;
        Some(Entry {
            alias: record.state.alias.clone(),
            status: record.state.status,
        })
    }
}

impl<'a> Store<'a> {
    /// A handle over an already-validated context.
    ///
    /// [`Ctx::store`] and [`Ctx::txn`] apply the format gate; this does not, so
    /// it is for callers that already hold a `Store` or `Txn`.
    pub fn new(ctx: &'a Ctx) -> Self {
        Self { ctx }
    }

    pub fn ctx(&self) -> &'a Ctx {
        self.ctx
    }

    pub fn config(&self) -> Result<Config> {
        self.ctx.load_config()
    }

    /// Identities of every record, for reference resolution.
    pub fn identities(&self) -> Result<Vec<ids::Known>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.ctx.records_dir()) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(StoreError::Io(e)),
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if !ids::is_valid_id(id) {
                continue;
            }
            let identity = record::read_identity(&entry.path()).unwrap_or_default();
            out.push(ids::Known {
                id: id.to_owned(),
                alias: identity.alias,
                legacy_aliases: identity.legacy_aliases,
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Resolve an alias, exact ID, or unique ID prefix.
    pub fn resolve(&self, input: &str) -> Result<String> {
        Ok(ids::resolve(&self.identities()?, input)?)
    }

    /// A task's alias, for rendering a reference the way it was typed.
    pub fn alias_of(&self, id: &str) -> Option<String> {
        record::read_identity(&self.ctx.record_path(id))
            .ok()
            .map(|identity| identity.alias)
    }

    pub fn load(&self, id: &str) -> Result<Record> {
        if !ids::is_valid_id(id) {
            return Err(StoreError::TaskNotFound(id.to_owned()));
        }
        record::read(&self.ctx.record_path(id))
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut out = Snapshot::default();
        let entries = match fs::read_dir(self.ctx.records_dir()) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                out.issues
                    .push(format!("{} is missing", self.ctx.records_dir().display()));
                return Ok(out);
            }
            Err(e) => return Err(StoreError::Io(e)),
        };
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl") else {
                out.issues
                    .push(format!("unexpected file in records/: {name}"));
                continue;
            };
            if !ids::is_valid_id(id) {
                out.issues
                    .push(format!("record file {name} is not named after a task ID"));
                continue;
            }
            match record::read(&entry.path()) {
                Ok(record) => out.records.push(record),
                Err(e) => out.issues.push(format!("record {id} is unreadable: {e}")),
            }
        }
        out.records.sort_by(|a, b| {
            a.state
                .created_at
                .cmp(&b.state.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out.issues.extend(duplicate_aliases(&out.records));
        Ok(out)
    }

    /// Append an event without taking the lock. Valid only for operations that
    /// are last-writer-wins or commutative; do not use it to replace a value
    /// that was read first.
    pub fn append(&self, id: &str, op_name: &str, data: Value) -> Result<()> {
        let path = self.ctx.record_path(id);
        if !path.is_file() {
            return Err(StoreError::TaskNotFound(id.to_owned()));
        }
        record::append(&path, &Event::new(op_name, data))
    }

    /// Append and return the folded result, reading the record back so the
    /// caller sees exactly what landed.
    pub fn append_and_view(&self, id: &str, op_name: &str, data: Value) -> Result<TaskView> {
        self.append(id, op_name, data)?;
        self.view_of(id)
    }

    /// Read one task into its computed view.
    pub fn view_of(&self, id: &str) -> Result<TaskView> {
        let record = self.load(id)?;
        Ok(self.view(&record, &Index::default()))
    }

    /// Reject an operation unless the record still carries `expected`.
    ///
    /// Only meaningful while the store lock is held: without it the record can
    /// change between this check and the write that follows.
    pub fn check_rev(&self, id: &str, expected: Option<&str>) -> Result<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let found = self.load(id)?.rev();
        if found != expected {
            return Err(StoreError::StaleRevision {
                id: id.to_owned(),
                expected: expected.to_owned(),
                found,
            });
        }
        Ok(())
    }

    pub fn view(&self, record: &Record, index: &Index) -> TaskView {
        enrich(self.ctx, record, index)
    }

    pub fn list(&self, opts: &ListOptions) -> Result<Vec<TaskView>> {
        if !self.ctx.tasks_dir.is_dir() {
            self.ctx.require_for_read()?;
            return Ok(Vec::new());
        }
        let snapshot = self.snapshot()?;
        let index = snapshot.index();
        let mut filtered: Vec<&Record> = snapshot
            .records
            .iter()
            .filter(|r| matches_list(r, opts))
            .collect();
        filtered.sort_by(|a, b| compare_records(&a.state, &b.state));
        if opts.limit > 0 && filtered.len() > opts.limit {
            filtered.truncate(opts.limit);
        }
        Ok(filtered
            .into_iter()
            .map(|r| enrich(self.ctx, r, &index))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Txn: everything that needs a consistent view
// ---------------------------------------------------------------------------

/// A mutation operation holding the store lock until dropped.
pub struct Txn<'a> {
    store: Store<'a>,
    _lock: StoreLock,
}

impl<'a> std::ops::Deref for Txn<'a> {
    type Target = Store<'a>;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl<'a> Txn<'a> {
    pub fn store(&self) -> &Store<'a> {
        &self.store
    }

    // -- config ------------------------------------------------------------

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) -> Result<Config> {
        let mut config = self.store.config()?;
        f(&mut config);
        self.store.ctx.write_config(&config)?;
        Ok(config)
    }

    pub fn init_store(&self, config: &Config) -> Result<()> {
        let _ = &self._lock;
        self.store.ctx.init_config(config)
    }

    // -- create ------------------------------------------------------------

    /// Create a task. Locked, because alias uniqueness is a store-wide
    /// invariant and two concurrent `add`s must not pick the same alias.
    pub fn create(&self, opts: CreateOptions) -> Result<TaskView> {
        let title = opts.title.trim().to_owned();
        if title.is_empty() {
            return Err(StoreError::EmptyTitle);
        }
        let config = self.store.config()?;
        let project = opts.project.unwrap_or_else(|| config.project.clone());
        ids::validate_project(&project)?;

        if let Some(parent) = &opts.parent {
            self.require_task(parent)?;
        }
        let known = self.store.identities()?;
        let now = timeutil::now_rfc3339_nano();
        for _ in 0..32u32 {
            let alias = ids::new_alias();
            if known
                .iter()
                .any(|k| k.alias == alias || k.legacy_aliases.contains(&alias))
            {
                continue;
            }
            let id = ids::new_id();
            let path = self.store.ctx.record_path(&id);
            if path.exists() {
                continue;
            }
            let state = TaskState {
                id: id.clone(),
                alias,
                legacy_aliases: Vec::new(),
                project: project.clone(),
                title: title.clone(),
                description: opts.description.clone(),
                status: Status::Open,
                priority: opts.priority.unwrap_or(config.defaults.priority),
                labels: opts
                    .labels
                    .clone()
                    .unwrap_or_else(|| config.defaults.labels.clone()),
                assignees: opts
                    .assignees
                    .clone()
                    .unwrap_or_else(|| config.defaults.assignees.clone()),
                assignee: None,
                attempt: 0,
                parent: opts.parent.clone(),
                blocked_by: Vec::new(),
                related: Vec::new(),
                estimate: opts.estimate,
                due_date: opts.due_date.clone(),
                logs: Vec::new(),
                created_at: now.clone(),
                updated_at: now.clone(),
                completed_at: None,
                archived_at: None,
                checkpoint: None,
                links: Vec::new(),
                acceptance: Vec::new(),
                evidence: Vec::new(),
            };
            let data =
                serde_json::to_value(&state).map_err(|e| parse_err("marshal new task", e))?;
            // `append` creates the file exclusively enough for our purposes:
            // the ID is freshly generated and 80 bits of randomness wide.
            record::append(&path, &Event::new(op::CREATED, data))?;
            return self.store.view_of(&id);
        }
        Err(StoreError::AliasCollisions(32))
    }

    // -- conditional writes ------------------------------------------------

    /// Append unless the record no longer has the revision the caller saw.
    ///
    /// The read, the comparison, and the append happen under one lock, which
    /// is what makes `--if-rev` meaningful rather than decorative.
    pub fn append_if_rev(
        &self,
        id: &str,
        op_name: &str,
        data: Value,
        expect_rev: Option<&str>,
    ) -> Result<TaskView> {
        self.store.check_rev(id, expect_rev)?;
        self.store.append(id, op_name, data)?;
        self.store.view_of(id)
    }

    /// Replace or clear a value, then report the result.
    pub fn set_field(
        &self,
        id: &str,
        op_name: &str,
        data: Value,
        expect_rev: Option<&str>,
    ) -> Result<TaskView> {
        self.append_if_rev(id, op_name, data, expect_rev)
    }

    // -- graph -------------------------------------------------------------

    pub fn add_blocker(&self, id: &str, blocker: &str) -> Result<TaskView> {
        if id == blocker {
            return Err(StoreError::Msg("task cannot block itself".into()));
        }
        self.require_task(blocker)?;
        if would_block_cycle(&self.store, id, blocker)? {
            return Err(StoreError::Msg(format!(
                "would create circular dependency: {id} is already blocked by {blocker} transitively"
            )));
        }
        self.store
            .append_and_view(id, op::BLOCK_ADD, serde_json::json!([blocker]))
    }

    /// Would adding this blocker create a cycle? Exposed so a batch can reject
    /// the whole request before writing anything.
    pub fn would_cycle(&self, id: &str, blocker: &str) -> Result<bool> {
        would_block_cycle(&self.store, id, blocker)
    }

    pub fn remove_blocker(&self, id: &str, blocker: &str) -> Result<(TaskView, bool)> {
        let record = self.store.load(id)?;
        let found = record.state.blocked_by.iter().any(|b| b == blocker);
        if found {
            self.store
                .append(id, op::BLOCK_REMOVE, serde_json::json!([blocker]))?;
        }
        Ok((self.store.view_of(id)?, found))
    }

    /// Record a non-blocking relationship with another task.
    ///
    /// Both endpoints must exist. Stored one-way and not cycle-checked: it is a
    /// "see also", not a constraint.
    pub fn add_related(&self, id: &str, other: &str) -> Result<TaskView> {
        if id == other {
            return Err(StoreError::Msg("a task cannot be related to itself".into()));
        }
        self.require_task(other)?;
        self.store
            .append_and_view(id, op::RELATED_ADD, serde_json::json!([other]))
    }

    pub fn remove_related(&self, id: &str, other: &str) -> Result<(TaskView, bool)> {
        let record = self.store.load(id)?;
        let found = record.state.related.iter().any(|r| r == other);
        if found {
            self.store
                .append(id, op::RELATED_REMOVE, serde_json::json!([other]))?;
        }
        Ok((self.store.view_of(id)?, found))
    }

    /// Set or clear the parent, validating existence and acyclicity.
    pub fn set_parent(&self, id: &str, parent: Option<&str>) -> Result<TaskView> {
        match parent {
            None => self
                .store
                .append_and_view(id, op::PARENT_CLEAR, Value::Null),
            Some(parent) => {
                if parent == id {
                    return Err(StoreError::Msg("task cannot be its own parent".into()));
                }
                self.require_task(parent)?;
                if would_parent_cycle(&self.store, id, parent)? {
                    return Err(StoreError::Msg(
                        "would create circular parent relationship".into(),
                    ));
                }
                self.store
                    .append_and_view(id, op::PARENT_SET, serde_json::json!(parent))
            }
        }
    }

    fn require_task(&self, id: &str) -> Result<()> {
        if self.store.ctx.record_path(id).is_file() {
            Ok(())
        } else {
            Err(StoreError::TaskNotFound(id.to_owned()))
        }
    }

    // -- project rename ----------------------------------------------------

    /// Change every task's display project. Identity is untouched, so no
    /// references need rewriting — that is the point of ULID IDs.
    pub fn rename_project(&self, old: &str, new: &str) -> Result<RenameResult> {
        ids::validate_project(new)?;
        if old == new {
            return Err(StoreError::Msg(format!(
                "project {old:?} is already named that"
            )));
        }
        let snapshot = self.store.snapshot()?;
        let affected: Vec<&Record> = snapshot
            .records
            .iter()
            .filter(|r| r.state.project == old)
            .collect();
        if affected.is_empty() {
            return Err(StoreError::Msg(format!(
                "no tasks found with project {old:?}"
            )));
        }
        let mut renamed = Vec::new();
        for record in &affected {
            self.store
                .append(&record.id, op::PROJECT, serde_json::json!(new))?;
            renamed.push(record.id.clone());
        }
        let mut config = self.store.config()?;
        if config.project == old {
            config.project = new.to_owned();
            self.store.ctx.write_config(&config)?;
        }
        Ok(RenameResult { renamed })
    }

    // -- deletion ----------------------------------------------------------

    /// Delete a record. Refuses while other records still reference it unless
    /// `scrub` is set; scrubbing rewrites those references, which is why this
    /// operation needs the lock.
    pub fn purge(&self, id: &str, scrub: bool) -> Result<PurgeOutcome> {
        if !ids::is_valid_id(id) {
            return Err(StoreError::TaskNotFound(id.to_owned()));
        }
        let snapshot = self.store.snapshot()?;
        let mut referrers = Vec::new();
        for record in &snapshot.records {
            if record.id == id {
                continue;
            }
            if record.state.blocked_by.iter().any(|b| b == id)
                || record.state.parent.as_deref() == Some(id)
                || record.state.related.iter().any(|r| r == id)
            {
                referrers.push(record.id.clone());
            }
        }
        if !referrers.is_empty() && !scrub {
            return Err(StoreError::Msg(format!(
                "task {id} is referenced by {}; delete it anyway with --scrub, or keep the record",
                referrers.join(", ")
            )));
        }
        let mut references_scrubbed = 0usize;
        for referrer in &referrers {
            let record = self.store.load(referrer)?;
            if record.state.blocked_by.iter().any(|b| b == id) {
                self.store
                    .append(referrer, op::BLOCK_REMOVE, serde_json::json!([id]))?;
                references_scrubbed += 1;
            }
            if record.state.parent.as_deref() == Some(id) {
                self.store.append(referrer, op::PARENT_CLEAR, Value::Null)?;
                references_scrubbed += 1;
            }
            if record.state.related.iter().any(|r| r == id) {
                self.store
                    .append(referrer, op::RELATED_REMOVE, serde_json::json!([id]))?;
                references_scrubbed += 1;
            }
        }
        fs::remove_file(self.store.ctx.record_path(id)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StoreError::TaskNotFound(id.to_owned())
            } else {
                StoreError::Io(e)
            }
        })?;
        if let Some(parent) = self.store.ctx.records_dir().parent()
            && let Ok(dir) = fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }
        Ok(PurgeOutcome {
            deleted: id.to_owned(),
            references_scrubbed,
            referrers,
        })
    }

    // -- retention ---------------------------------------------------------

    /// Retire old terminal tasks: archive by default, delete with `purge`.
    pub fn clean(&self, days: i64, purge: bool) -> Result<CleanOutcome> {
        if days < 0 {
            return Err(StoreError::Msg(
                "clean threshold must be non-negative".into(),
            ));
        }
        let snapshot = self.store.snapshot()?;
        let now = chrono::Utc::now();
        let threshold = chrono::Duration::days(days);

        let doomed: Vec<String> = snapshot
            .records
            .iter()
            .filter(|r| {
                r.state.status.is_terminal()
                    && r.state
                        .completed_at
                        .as_deref()
                        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
                        .is_some_and(|c| {
                            now.signed_duration_since(c.with_timezone(&chrono::Utc)) > threshold
                        })
            })
            .map(|r| r.id.clone())
            .collect();

        let mut out = CleanOutcome::default();
        if purge {
            for id in &doomed {
                let outcome = self.purge(id, true)?;
                out.references_scrubbed += outcome.references_scrubbed;
                out.purged += 1;
            }
        } else {
            for record in &snapshot.records {
                if !doomed.contains(&record.id) || record.state.is_archived() {
                    continue;
                }
                self.store.append(&record.id, op::ARCHIVED, Value::Null)?;
                out.archived += 1;
            }
        }
        Ok(out)
    }

    // -- recovery ----------------------------------------------------------

    /// Truncate torn tails left by interrupted writes.
    pub fn recover(&self, id: Option<&str>, dry_run: bool) -> Result<RecoverOutcome> {
        let targets: Vec<String> = match id {
            Some(id) => vec![id.to_owned()],
            None => self
                .store
                .snapshot()?
                .records
                .iter()
                .map(|r| r.id.clone())
                .collect(),
        };
        let mut out = RecoverOutcome {
            dry_run,
            ..Default::default()
        };
        for target in targets {
            let path = self.store.ctx.record_path(&target);
            let Some((_, dropped)) = record::torn_tail(&path)? else {
                continue;
            };
            out.repaired.push(target.clone());
            out.bytes_dropped += dropped;
            if !dry_run {
                record::truncate_torn(&path)?;
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Atomic write
// ---------------------------------------------------------------------------

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Msg("no parent dir".into()))?;
    let tmp = parent.join(format!(".tmp.{}-{}", std::process::id(), ids::new_alias()));
    let mut f = fs::File::create(&tmp).map_err(StoreError::Io)?;
    f.write_all(content).map_err(StoreError::Io)?;
    f.sync_all().map_err(StoreError::Io)?;
    drop(f);
    fs::rename(&tmp, path).map_err(StoreError::Io)?;
    // Durability: fsync the directory entry too.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Enrichment and graph helpers
// ---------------------------------------------------------------------------

/// Read a task into its computed view plus any inconsistencies on disk.
///
/// Read-only: nothing is repaired or persisted.
pub fn get_task(ctx: &Ctx, id: &str) -> Result<(TaskView, Vec<String>)> {
    let store = ctx.store()?;
    let record = store.load(id)?;
    let issues = inconsistencies(ctx, &record, id)?;
    Ok((store.view(&record, &Index::default()), issues))
}

/// Report what is wrong with one record. Read-only; `tk check` reports the same
/// findings for the whole store, and only `tk recover` changes anything.
pub fn inconsistencies(ctx: &Ctx, record: &Record, expected_id: &str) -> Result<Vec<String>> {
    let mut issues = Vec::new();
    if record.id != expected_id {
        issues.push(format!(
            "file name {expected_id} does not match the record ID {}",
            record.id
        ));
    }
    if record.torn {
        issues.push("the last line was interrupted mid-write and is ignored".to_owned());
    }
    for damage in &record.damaged {
        issues.push(format!("unparseable event {damage}"));
    }
    if !ids::is_valid_alias(&record.state.alias) {
        issues.push(format!("invalid alias {:?}", record.state.alias));
    }
    for blocker in &record.state.blocked_by {
        if !ctx.record_path(blocker).is_file() {
            issues.push(format!("blocked by missing task {blocker}"));
        }
    }
    for related in &record.state.related {
        if !ctx.record_path(related).is_file() {
            issues.push(format!("related to missing task {related}"));
        }
    }
    if let Some(parent) = &record.state.parent
        && !ctx.record_path(parent).is_file()
    {
        issues.push(format!("parent task {parent} is missing"));
    }
    Ok(issues)
}

fn duplicate_aliases(records: &[Record]) -> Vec<String> {
    let mut seen: HashMap<&str, &str> = HashMap::new();
    let mut issues = Vec::new();
    for record in records {
        let alias = record.state.alias.as_str();
        if alias.is_empty() {
            continue;
        }
        match seen.get(alias) {
            Some(first) => issues.push(format!(
                "alias {alias} is claimed by both {first} and {}",
                record.id
            )),
            None => {
                seen.insert(alias, record.id.as_str());
            }
        }
    }
    issues
}

pub fn enrich(ctx: &Ctx, record: &Record, index: &Index) -> TaskView {
    let state = &record.state;
    let mut blocked_by_incomplete = false;
    let mut unresolved_blockers = Vec::new();
    let mut blocker_refs = Vec::new();
    for blocker in &state.blocked_by {
        match index.lookup(ctx, blocker) {
            Some(entry) => {
                blocker_refs.push(entry.alias);
                if !entry.status.is_terminal() {
                    blocked_by_incomplete = true;
                }
            }
            None => {
                // A missing prerequisite is unresolved, not completed.
                unresolved_blockers.push(blocker.clone());
                blocker_refs.push(short_id(blocker));
                blocked_by_incomplete = true;
            }
        }
    }
    let parent_ref = state.parent.as_ref().map(|p| {
        index
            .lookup(ctx, p)
            .map(|e| e.alias)
            .unwrap_or_else(|| short_id(p))
    });
    let related_refs: Vec<String> = state
        .related
        .iter()
        .map(|r| {
            index
                .lookup(ctx, r)
                .map(|e| e.alias)
                .unwrap_or_else(|| short_id(r))
        })
        .collect();
    let done = state.status.is_terminal();
    TaskView {
        rev: record.rev(),
        blocked_by_incomplete,
        unresolved_blockers,
        blocker_refs,
        related_refs,
        parent_ref,
        is_overdue: timeutil::is_overdue(state.due_date.as_deref(), done),
        days_until_due: timeutil::days_until_due(state.due_date.as_deref(), done),
        task: state.clone(),
    }
}

/// Enough of an ID to identify it in passing.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn would_block_cycle(store: &Store<'_>, task_id: &str, blocker_id: &str) -> Result<bool> {
    let mut visited = HashSet::new();
    let mut stack = vec![blocker_id.to_owned()];
    while let Some(current) = stack.pop() {
        if current == task_id {
            return Ok(true);
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        if let Ok(record) = store.load(&current) {
            stack.extend(record.state.blocked_by);
        }
    }
    Ok(false)
}

fn would_parent_cycle(store: &Store<'_>, task_id: &str, parent_id: &str) -> Result<bool> {
    let mut visited = HashSet::new();
    let mut current = parent_id.to_owned();
    loop {
        if current == task_id || !visited.insert(current.clone()) {
            return Ok(true);
        }
        match store.load(&current) {
            Ok(record) => match record.state.parent {
                Some(parent) => current = parent,
                None => return Ok(false),
            },
            Err(_) => return Ok(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Option and outcome types
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct CreateOptions {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    pub project: Option<String>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub parent: Option<String>,
    pub estimate: Option<i64>,
    pub due_date: Option<String>,
}

#[derive(Debug, Default)]
pub struct CleanOutcome {
    pub archived: usize,
    pub purged: usize,
    pub references_scrubbed: usize,
}

#[derive(Debug)]
pub struct PurgeOutcome {
    pub deleted: String,
    pub references_scrubbed: usize,
    pub referrers: Vec<String>,
}

#[derive(Debug, Default)]
pub struct RecoverOutcome {
    pub repaired: Vec<String>,
    pub bytes_dropped: u64,
    pub dry_run: bool,
}

pub struct RenameResult {
    pub renamed: Vec<String>,
}

// ---------------------------------------------------------------------------
// List / filter / sort
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ListOptions {
    pub search: String,
    pub hide_terminal: bool,
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    pub project: String,
    pub label: String,
    pub assignee: String,
    pub parent: Option<Option<String>>,
    pub roots: bool,
    pub overdue: bool,
    /// Include archived tasks (they are hidden by default).
    pub include_archived: bool,
    /// Show archived tasks only.
    pub archived_only: bool,
    pub limit: usize,
}

fn matches_list(record: &Record, opts: &ListOptions) -> bool {
    let state = &record.state;
    match (
        state.is_archived(),
        opts.archived_only,
        opts.include_archived,
    ) {
        (true, false, false) => return false,
        (false, true, _) => return false,
        _ => {}
    }
    if let Some(status) = opts.status
        && state.status != status
    {
        return false;
    }
    if opts.hide_terminal && state.status.is_terminal() {
        return false;
    }
    if !opts.search.is_empty() {
        let q = opts.search.to_lowercase();
        let hit = state.title.to_lowercase().contains(&q)
            || state
                .description
                .as_deref()
                .is_some_and(|d| d.to_lowercase().contains(&q))
            || state.id.to_lowercase().contains(&q)
            || state.alias.to_lowercase().contains(&q);
        if !hit {
            return false;
        }
    }
    if let Some(priority) = opts.priority
        && state.priority != priority
    {
        return false;
    }
    if !opts.project.is_empty() && state.project != opts.project {
        return false;
    }
    if !opts.label.is_empty() && !state.labels.iter().any(|l| l == &opts.label) {
        return false;
    }
    if !opts.assignee.is_empty() && !state.assignees.iter().any(|a| a == &opts.assignee) {
        return false;
    }
    if opts.roots && state.parent.is_some() {
        return false;
    }
    if let Some(parent) = &opts.parent
        && state.parent.as_ref() != parent.as_ref()
    {
        return false;
    }
    if opts.overdue && !timeutil::is_overdue(state.due_date.as_deref(), state.status.is_terminal())
    {
        return false;
    }
    true
}

fn status_rank(status: Status) -> u8 {
    match status {
        Status::Active => 0,
        Status::Open => 1,
        Status::Deferred => 2,
        Status::Done => 3,
        Status::Closed => 4,
    }
}

fn compare_records(a: &TaskState, b: &TaskState) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let ord = status_rank(a.status).cmp(&status_rank(b.status));
    if ord != Ordering::Equal {
        return ord;
    }
    if !a.status.is_terminal() {
        // Overdue first.
        let oa = timeutil::is_overdue(a.due_date.as_deref(), false);
        let ob = timeutil::is_overdue(b.due_date.as_deref(), false);
        let ord = ob.cmp(&oa);
        if ord != Ordering::Equal {
            return ord;
        }
        // Priority (1-4, none last).
        let ord = a.priority.sort_key().cmp(&b.priority.sort_key());
        if ord != Ordering::Equal {
            return ord;
        }
        // Due date (soonest first, undated last).
        match (&a.due_date, &b.due_date) {
            (Some(x), Some(y)) if x != y => return x.cmp(y),
            (Some(_), None) => return Ordering::Less,
            (None, Some(_)) => return Ordering::Greater,
            _ => {}
        }
        // Oldest first: a task list is a queue, and stable order helps agents.
        return a.created_at.cmp(&b.created_at);
    }
    // Terminal: newest completion first.
    b.completed_at.cmp(&a.completed_at)
}

// ---------------------------------------------------------------------------
// Integrity
// ---------------------------------------------------------------------------

/// Everything wrong with the store, as a sorted list of human sentences.
pub fn check_integrity(ctx: &Ctx) -> Result<Vec<String>> {
    ctx.require()?;
    let mut issues = Vec::new();

    for stray in ctx.top_level_task_files() {
        issues.push(format!(
            "{stray} is a task file from the previous layout; the v1 store keeps records in {RECORDS_DIR}/"
        ));
    }
    if ctx.legacy_config_path().exists() {
        issues.push(format!(
            "{LEGACY_CONFIG_FILE} is left over from the previous layout and is ignored"
        ));
    }

    let store = ctx.store()?;
    let snapshot = store.snapshot()?;
    issues.extend(snapshot.issues.iter().cloned());

    for record in &snapshot.records {
        issues.extend(inconsistencies(ctx, record, &record.id)?);
    }

    // Aliases and IDs must be usable as typed handles.
    let known: HashSet<&str> = snapshot.records.iter().map(|r| r.id.as_str()).collect();
    for record in &snapshot.records {
        if !ids::is_valid_id(&record.state.id) {
            issues.push(format!(
                "record {} carries invalid ID {:?}",
                record.id, record.state.id
            ));
        }
        if record.state.id != record.id {
            issues.push(format!(
                "record {} carries ID {}",
                record.id, record.state.id
            ));
        }
        for alias in &record.state.legacy_aliases {
            if snapshot
                .records
                .iter()
                .any(|other| other.id != record.id && other.state.alias == *alias)
            {
                issues.push(format!(
                    "record {} keeps legacy alias {alias}, which is also a live alias",
                    record.id
                ));
            }
        }
    }

    // Cycles are valid JSON but not a valid graph.
    let blocked: HashMap<String, Vec<String>> = snapshot
        .records
        .iter()
        .map(|r| {
            (
                r.id.clone(),
                r.state
                    .blocked_by
                    .iter()
                    .filter(|b| known.contains(b.as_str()))
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    let parents: HashMap<String, Vec<String>> = snapshot
        .records
        .iter()
        .map(|r| {
            (
                r.id.clone(),
                r.state
                    .parent
                    .iter()
                    .filter(|p| known.contains(p.as_str()))
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    for id in cyclic_nodes(&blocked) {
        issues.push(format!("record {id} is part of a dependency cycle"));
    }
    for id in cyclic_nodes(&parents) {
        issues.push(format!("record {id} is part of a parent cycle"));
    }

    issues.sort();
    issues.dedup();
    Ok(issues)
}

/// Nodes that participate in a cycle of `id -> targets`: a node is in a cycle
/// exactly when it can reach itself.
fn cyclic_nodes(edges: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for start in edges.keys() {
        let mut seen = HashSet::new();
        let mut stack: Vec<&String> = edges.get(start).into_iter().flatten().collect();
        let mut cyclic = false;
        while let Some(node) = stack.pop() {
            if node == start {
                cyclic = true;
                break;
            }
            if !seen.insert(node.clone()) {
                continue;
            }
            if let Some(next) = edges.get(node) {
                stack.extend(next.iter());
            }
        }
        if cyclic {
            out.push(start.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every `.tasks/` directory under `root`, for multi-store locking.
///
/// Does not follow symlinks and skips `.git`; results are not sorted.
pub fn find_stores(root: &Path) -> Result<Vec<PathBuf>> {
    const MAX_DEPTH: usize = 8;
    let mut out = Vec::new();
    let mut queue = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = queue.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(StoreError::Io(e)),
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            if name == TASKS_DIR {
                out.push(entry.path());
                continue;
            }
            queue.push((entry.path(), depth + 1));
        }
    }
    Ok(out)
}

/// Sorted, de-duplicated set of values, for deterministic output.
pub fn sorted_unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyclic_nodes_finds_loops() {
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert("a".into(), vec!["b".into()]);
        edges.insert("b".into(), vec!["a".into()]);
        edges.insert("c".into(), vec!["d".into()]);
        edges.insert("d".into(), vec![]);
        assert_eq!(cyclic_nodes(&edges), vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn duplicate_aliases_are_reported() {
        // Two records claiming one alias would make every typed reference
        // ambiguous, so the resolver must never have to choose.
        let mk = |id: &str, alias: &str| Record {
            id: id.to_owned(),
            events: Vec::new(),
            state: TaskState {
                id: id.to_owned(),
                alias: alias.to_owned(),
                legacy_aliases: Vec::new(),
                project: "p".into(),
                title: "t".into(),
                description: None,
                status: Status::Open,
                priority: Priority::Medium,
                labels: Vec::new(),
                assignees: Vec::new(),
                assignee: None,
                attempt: 0,
                parent: None,
                blocked_by: Vec::new(),
                related: Vec::new(),
                estimate: None,
                due_date: None,
                logs: Vec::new(),
                created_at: "x".into(),
                updated_at: "y".into(),
                completed_at: None,
                archived_at: None,
                checkpoint: None,
                links: Vec::new(),
                acceptance: Vec::new(),
                evidence: Vec::new(),
            },
            torn: false,
            damaged: Vec::new(),
        };
        let issues = duplicate_aliases(&[mk("a", "zzzz"), mk("b", "zzzz"), mk("c", "yyyy")]);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("zzzz"), "{issues:?}");
    }
}
