//! Filesystem store: location, the format gate, the mutation lock, and the only
//! code that touches entry files.
//!
//! Three entry points, and the difference between them is the point:
//!
//! - [`Ctx::store`] gives a [`Store`]: reads, no lock.
//! - [`Ctx::txn`] gives a [`Txn`]: holds the store lock, and is the only thing
//!   that writes an entry. Every mutation goes through it, so the lock cannot be
//!   forgotten by accident — there is no unlocked write path.
//! - [`Ctx::txn_init`] additionally allows creating the store.
//!
//! One lock for the whole store, held across read-modify-write, because every
//! mutation is now a whole-file rewrite. v1's lock-free appends came from the
//! event log, which is gone; what replaced it is a short critical section and a
//! file rename that is either complete or absent.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::ids::{self, IdError, Known};
use crate::model::{Config, Entry, EntryView, State};
use crate::output::code;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("no {TASKS_DIR}/ directory found. Run 'tk init' to create one.\nSearched from: {0}")]
    TasksNotFound(String),
    #[error(
        "task store not found: {path}\n{origin} was set explicitly, so tk will not create a store there.\nCreate it deliberately with 'tk --tasks-dir {path} init', or point at an existing store."
    )]
    ExplicitStoreMissing { path: String, origin: String },
    #[error(
        "not a tk format-{format} store at {path}\n  {detail}\nThis binary reads only format {format} stores. Convert an older store with tools/migrate_to_v3.py, or start a fresh one with 'tk init'."
    )]
    NotStore {
        path: String,
        detail: String,
        format: i64,
    },
    #[error("no entry matches {0:?}: use its ref (like a7b3) or part of its title")]
    EntryNotFound(String),
    #[error(
        "entry {entry_ref} changed since it was read (expected revision {expected}, found {found}); re-read it and retry"
    )]
    StaleRevision {
        entry_ref: String,
        expected: String,
        found: String,
    },
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error("could not find a free ref after {0} attempts")]
    RefCollisions(u32),
    #[error("blocking {blocked} would make a loop: {path}")]
    WouldCycle { blocked: String, path: String },
    /// The request or command line itself is wrong. Distinct from a store
    /// failure, so a caller can fix its input rather than retry.
    #[error("{0}")]
    InvalidInput(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {what}: {err}")]
    Parse { what: String, err: String },
    #[error("{0}")]
    Id(#[from] IdError),
    /// A bad value in the request itself, such as `--state active`.
    #[error(transparent)]
    Model(#[from] crate::model::ModelError),
    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Satisfies the bound that lets `?` turn a store failure into a report.
///
/// Deliberately no `code()`: miette prints whatever that returns in front of the
/// message a human reads. The kind is recovered from the error's type in
/// `cli::run` and appears only in the JSON envelope.
impl miette::Diagnostic for StoreError {}

impl StoreError {
    /// Stable machine-readable kind, for the JSON envelope's `error_code`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TasksNotFound(_) | Self::ExplicitStoreMissing { .. } => code::STORE_NOT_FOUND,
            Self::NotStore { .. } => code::NOT_A_STORE,
            Self::EntryNotFound(_) => code::NOT_FOUND,
            Self::StaleRevision { .. } => code::STALE_REVISION,
            Self::EmptyTitle
            | Self::RefCollisions(_)
            | Self::WouldCycle { .. }
            | Self::InvalidInput(_) => code::INVALID_INPUT,
            Self::Model(_) => code::INVALID_INPUT,
            Self::Io(_) => code::IO,
            Self::Parse { .. } => code::PARSE,
            Self::Id(IdError::NotFound(_)) => code::NOT_FOUND,
            Self::Id(IdError::Ambiguous { .. }) => code::AMBIGUOUS,
            Self::Id(_) => code::INVALID_INPUT,
            Self::Msg(_) => code::ERROR,
        }
    }
}

pub fn parse_err(what: impl Into<String>, err: impl ToString) -> StoreError {
    StoreError::Parse {
        what: what.into(),
        err: err.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

pub const TASKS_DIR: &str = ".tasks";
/// The store file: the layout version, and nothing else.
pub const STORE_FILE: &str = ".tk.json";
/// The layout this binary reads and writes. One JSON document per entry.
pub const FORMAT: i64 = 3;
const LOCK_FILE: &str = ".lock";
/// Written by a migration, listing old handles.
const MIGRATION_FILE: &str = "MIGRATION.md";

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
        Ok(Self::walk(&cwd))
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

    pub fn lock_path(&self) -> PathBuf {
        self.tasks_dir.join(LOCK_FILE)
    }

    /// `<ref>-<slug>.json`, or `<ref>.json` when there is no slug.
    pub fn entry_path(&self, r#ref: &str, slug: &str) -> PathBuf {
        self.tasks_dir.join(ids::file_name_of(r#ref, slug))
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
    /// Every read and write path goes through here, so an older store is refused
    /// with an explanation instead of being read as empty and then written over.
    pub fn require(&self) -> Result<()> {
        // Checked live rather than from the cached `exists`: a store created
        // during this process (by `tk init`, or by a test) is still readable.
        if !self.tasks_dir.is_dir() {
            return Err(self.missing_store_error());
        }
        self.check_format()
    }

    /// Refuse anything that is not a `format: 3` store, naming the reason.
    pub fn check_format(&self) -> Result<()> {
        match fs::read(self.store_path()) {
            Ok(data) => match serde_json::from_slice::<Config>(&data) {
                Ok(config) if config.format == FORMAT => Ok(()),
                Ok(config) => Err(self.not_store(format!(
                    "{STORE_FILE} declares format {}, but this binary writes format {FORMAT}",
                    config.format
                ))),
                Err(e) => Err(self.not_store(format!("{STORE_FILE} could not be parsed: {e}"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(self.not_store(self.describe_non_store()))
            }
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    fn not_store(&self, detail: String) -> StoreError {
        StoreError::NotStore {
            path: self.tasks_dir.display().to_string(),
            detail,
            format: FORMAT,
        }
    }

    /// Why this directory is not a format-3 store, said specifically enough to
    /// tell a human which migration to run.
    fn describe_non_store(&self) -> String {
        if self.tasks_dir.join("store.json").exists() {
            return "found store.json and records/, the v1 layout (an appended event log)"
                .to_owned();
        }
        if self.tasks_dir.join("config.json").exists() {
            return "found config.json, the v0 layout (one JSON file per task, keyed by project)"
                .to_owned();
        }
        let strays = self.unrecognized_json();
        if !strays.is_empty() {
            // A v0 store may have no config.json at all — several real ones do
            // not — so its task documents are the store. One peek tells them
            // apart from a genuinely stray file and names the right migration.
            if let Some(task) = strays.iter().find(|name| self.is_task_document(name)) {
                return format!("found task documents from the v0 layout, starting with {task}");
            }
            let mut shown: Vec<String> = strays.iter().take(3).cloned().collect();
            if strays.len() > 3 {
                shown.push(format!("and {} more", strays.len() - 3));
            }
            return format!(
                "found files whose names are not entry names (<ref>-<slug>.json): {}",
                shown.join(", ")
            );
        }
        format!("{STORE_FILE} is missing")
    }

    /// True when a file in the store parses as a v0 task document: it has a
    /// `ref` and a `title`, which is what every version of that layout wrote.
    fn is_task_document(&self, name: &str) -> bool {
        let Ok(data) = fs::read(self.tasks_dir.join(name)) else {
            return false;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data) else {
            return false;
        };
        value.get("ref").is_some() && value.get("title").is_some()
    }

    /// `.json` files in the store that are not `.tk.json` and not entry names.
    fn unrecognized_json(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.tasks_dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") || name == STORE_FILE || !entry.path().is_file() {
                continue;
            }
            if ids::parse_file_name(&name).is_none() {
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
        serde_json::from_slice(&data)
            .map_err(|e| self.not_store(format!("{STORE_FILE} could not be parsed: {e}")))
    }

    fn write_config(&self, config: &Config) -> Result<()> {
        let data = serde_json::to_string_pretty(config).map_err(|e| parse_err(STORE_FILE, e))?;
        atomic_write(&self.store_path(), format!("{data}\n").as_bytes())
    }

    // -- entry points ------------------------------------------------------

    /// Reads only.
    pub fn store(&self) -> Result<Store<'_>> {
        self.require()?;
        Ok(Store { ctx: self })
    }

    /// Reads and writes, under the store lock.
    ///
    /// Do not open two transactions against one store in one process: the
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
        let lock = self.lock_mutation()?;
        let txn = Txn {
            store: Store { ctx: self },
            _lock: lock,
        };
        if !self.store_path().exists() {
            self.write_config(&Config::default())?;
        }
        Ok(txn)
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
}

// ---------------------------------------------------------------------------
// Mutation guard
// ---------------------------------------------------------------------------

/// Advisory exclusive lock over one store's mutation boundary.
///
/// Serializes cooperating `tk` processes, which matters more than it did in v1:
/// a mutation rewrites a whole file, so two writers that both read before either
/// wrote can lose one of the changes. It does not constrain editors, a Git
/// checkout, or an older `tk` binary, and it cannot coordinate separate clones or
/// machines.
pub struct StoreLock {
    file: fs::File,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

// ---------------------------------------------------------------------------
// Store: reads
// ---------------------------------------------------------------------------

/// Read operations on a store. Constructed only through [`Ctx::store`].
pub struct Store<'a> {
    ctx: &'a Ctx,
}

/// What a scan found, problems included.
#[derive(Debug, Default)]
pub struct Scan {
    /// Entry files that parsed, with the path each came from.
    pub entries: Vec<(PathBuf, Entry)>,
    /// Files that are not readable entries: unparseable JSON, an entry name whose
    /// ref disagrees with its content, a duplicate ref, a stray `.json`.
    pub issues: Vec<String>,
}

/// List filter. Defaults to open entries, oldest first.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Case-insensitive substring of the title, a label, or the status.
    pub search: String,
    /// An exact state; overrides `include_closed`.
    pub state: Option<State>,
    pub label: String,
    /// `Some(true)` blocked only, `Some(false)` unblocked only.
    pub blocked: Option<bool>,
    /// Include done and dropped entries.
    pub include_closed: bool,
    pub limit: usize,
}

impl Filter {
    fn matches(&self, view: &EntryView) -> bool {
        let entry = &view.entry;
        match self.state {
            Some(state) if entry.state != state => return false,
            Some(_) => {}
            None if !self.include_closed && entry.state.is_closed() => return false,
            None => {}
        }
        if let Some(blocked) = self.blocked
            && view.is_waiting() != blocked
        {
            return false;
        }
        if !self.label.is_empty()
            && !entry
                .labels
                .iter()
                .any(|l| l.eq_ignore_ascii_case(&self.label))
        {
            return false;
        }
        if !self.search.is_empty() {
            let needle = self.search.to_lowercase();
            let hit = entry.title.to_lowercase().contains(&needle)
                || entry
                    .labels
                    .iter()
                    .any(|l| l.to_lowercase().contains(&needle));
            if !hit {
                return false;
            }
        }
        true
    }
}

impl<'a> Store<'a> {
    pub fn ctx(&self) -> &'a Ctx {
        self.ctx
    }

    /// Read every entry file. Unreadable files become issues, never silence.
    pub fn scan(&self) -> Result<Scan> {
        let mut scan = Scan::default();
        let dir = fs::read_dir(&self.ctx.tasks_dir);
        let dir = match dir {
            Ok(dir) => dir,
            // An undiscovered store has nothing to show rather than an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(scan),
            Err(e) => return Err(StoreError::Io(e)),
        };

        let mut seen: HashMap<String, PathBuf> = HashMap::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // Files tk writes, and the two a migration leaves behind.
            if matches!(
                name.as_str(),
                STORE_FILE | LOCK_FILE | ".gitignore" | MIGRATION_FILE
            ) {
                continue;
            }
            if path.is_dir() {
                // Directories are never reported. tk writes files, so a directory
                // cannot be debris from a crashed write, and a store may
                // legitimately hold one: long-form documents attached to a task,
                // named from a log entry, are the convention in place of the
                // `links` field this design dropped. A migration's `legacy/` is
                // the same shape.
                continue;
            }
            if ids::parse_file_name(&name).is_none() {
                // Anything else is reported rather than ignored: a temp file
                // left by an interrupted write, or a stray note. Silence here is
                // how debris accumulates while `check` keeps saying the store is
                // fine.
                // `check` keeps saying the store is fine.
                scan.issues.push(format!(
                    "{name}: not a tk file (tk writes <ref>-<slug>.json and {STORE_FILE} here)"
                ));
                continue;
            }
            if !path.is_file() {
                scan.issues.push(format!("{name}: not a regular file"));
                continue;
            }
            paths.push(path);
        }
        paths.sort();

        for path in paths {
            let name = file_name(&path);
            let data = match fs::read(&path) {
                Ok(data) => data,
                Err(e) => {
                    scan.issues.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let entry: Entry = match serde_json::from_slice(&data) {
                Ok(entry) => entry,
                Err(e) => {
                    scan.issues.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let named = ids::parse_file_name(&name).expect("filtered above");
            if !ids::is_valid_ref(&entry.r#ref) {
                scan.issues.push(format!(
                    "{name}: ref {:?} is not four characters from the tk alphabet",
                    entry.r#ref
                ));
            } else if entry.r#ref != named.r#ref {
                scan.issues.push(format!(
                    "{name}: file name says {} but the document says {}",
                    named.r#ref, entry.r#ref
                ));
            }
            if let Some(previous) = seen.insert(entry.r#ref.clone(), path.clone()) {
                scan.issues.push(format!(
                    "ref {} appears twice: {} and {name}",
                    entry.r#ref,
                    file_name(&previous)
                ));
            }
            if entry.title.trim().is_empty() {
                scan.issues.push(format!("{name}: title is empty"));
            }
            scan.entries.push((path, entry));
        }
        Ok(scan)
    }

    /// What the resolver needs: ref, slug, and title for every entry.
    pub fn known(&self) -> Result<Vec<Known>> {
        Ok(self
            .scan()?
            .entries
            .into_iter()
            .map(|(path, entry)| Known {
                r#ref: entry.r#ref,
                slug: ids::parse_file_name(&file_name(&path))
                    .map(|k| k.slug)
                    .unwrap_or_default(),
                title: entry.title,
                state: entry.state,
            })
            .collect())
    }

    /// Resolve a ref or part of a title to exactly one ref.
    pub fn resolve(&self, input: &str) -> Result<String> {
        Ok(ids::resolve(&self.known()?, input)?)
    }

    /// Load one entry by ref.
    pub fn load(&self, r#ref: &str) -> Result<(PathBuf, Entry)> {
        let scan = self.scan()?;
        scan.entries
            .into_iter()
            .find(|(_, entry)| entry.r#ref == r#ref)
            .ok_or_else(|| StoreError::EntryNotFound(r#ref.to_owned()))
    }

    /// Resolve and load, with the computed view.
    pub fn get(&self, input: &str) -> Result<EntryView> {
        let known = self.known()?;
        let r#ref = ids::resolve(&known, input)?;
        let (path, entry) = self.load(&r#ref)?;
        Ok(view_of(&entry, &file_name(&path), &known))
    }

    /// Every entry matching `filter`, oldest first, with the scan's issues.
    ///
    /// The whole store is read and folded on every call: measured at 20ms for
    /// 161 entries, so there is no index and nothing to keep in sync.
    pub fn list(&self, filter: &Filter) -> Result<(Vec<EntryView>, Vec<String>)> {
        let scan = self.scan()?;
        let known: Vec<Known> = scan
            .entries
            .iter()
            .map(|(path, entry)| Known {
                r#ref: entry.r#ref.clone(),
                slug: ids::parse_file_name(&file_name(path))
                    .map(|k| k.slug)
                    .unwrap_or_default(),
                title: entry.title.clone(),
                state: entry.state,
            })
            .collect();

        let mut views: Vec<EntryView> = scan
            .entries
            .iter()
            .map(|(path, entry)| view_of(entry, &file_name(path), &known))
            .filter(|view| filter.matches(view))
            .collect();
        views.sort_by(|a, b| {
            a.entry
                .created
                .cmp(&b.entry.created)
                .then_with(|| a.entry.r#ref.cmp(&b.entry.r#ref))
        });
        if filter.limit > 0 {
            views.truncate(filter.limit);
        }
        Ok((views, scan.issues))
    }

    /// Fingerprint of an entry as it is now on disk.
    pub fn rev_of(&self, r#ref: &str) -> Result<String> {
        Ok(fingerprint(&self.load(r#ref)?.1))
    }

    /// Refuse a write when the entry changed since it was read.
    pub fn check_rev(&self, r#ref: &str, expected: Option<&str>) -> Result<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let found = self.rev_of(r#ref)?;
        if found != expected {
            return Err(StoreError::StaleRevision {
                entry_ref: r#ref.to_owned(),
                expected: expected.to_owned(),
                found,
            });
        }
        Ok(())
    }
}

/// Build the reader's view of an entry. Pure: no filesystem access.
pub fn view_of(entry: &Entry, file: &str, known: &[Known]) -> EntryView {
    let by_ref: HashMap<&str, &Known> = known.iter().map(|k| (k.r#ref.as_str(), k)).collect();
    let mut unresolved_blockers = Vec::new();
    let mut blocking = Vec::new();
    for blocker in &entry.blocked_by {
        match by_ref.get(blocker.as_str()) {
            // Not in the store: counts as blocking, never as done.
            None => {
                unresolved_blockers.push(blocker.clone());
                blocking.push(blocker.clone());
            }
            Some(known) if known.state == State::Open => blocking.push(blocker.clone()),
            Some(_) => {}
        }
    }
    EntryView {
        entry: entry.clone(),
        rev: fingerprint(entry),
        unresolved_blockers,
        blocking,
        file: file.to_owned(),
    }
}

/// Content fingerprint for `--if-rev`.
///
/// FNV-1a over the canonical serialization of the parsed document, so it is
/// stable for the same content regardless of key order on disk, and it changes
/// when any field does. It is not a cryptographic hash and carries no
/// compatibility promise across versions: both sides of the comparison are the
/// same binary.
pub fn fingerprint(entry: &Entry) -> String {
    // Serializing this type cannot fail: every string is UTF-8 and no map key is
    // anything but a string.
    let canonical = serde_json::to_vec(entry).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

// ---------------------------------------------------------------------------
// Txn: the only writer
// ---------------------------------------------------------------------------

/// Read-write operations on a store, holding the mutation lock.
pub struct Txn<'a> {
    store: Store<'a>,
    _lock: StoreLock,
}

impl<'a> Txn<'a> {
    pub fn store(&self) -> &Store<'a> {
        &self.store
    }

    pub fn ctx(&self) -> &'a Ctx {
        self.store.ctx
    }

    pub fn known(&self) -> Result<Vec<Known>> {
        self.store.known()
    }

    pub fn resolve(&self, input: &str) -> Result<String> {
        self.store.resolve(input)
    }

    pub fn load(&self, r#ref: &str) -> Result<(PathBuf, Entry)> {
        self.store.load(r#ref)
    }

    /// Mark the moment of a change. Every write should call this first.
    pub fn touch(entry: &mut Entry, now: &str) {
        entry.updated = now.to_owned();
    }

    /// Write one entry, keeping the file it came from when the name changed.
    ///
    /// `previous` is the path the entry was read from, if any: the slug lives in
    /// the filename, so preserving it means reading it back rather than
    /// re-deriving it from a title that may have changed.
    pub fn write(&self, entry: &Entry, previous: Option<&Path>) -> Result<PathBuf> {
        let slug = match previous.and_then(|p| ids::parse_file_name(&file_name(p))) {
            Some(named) => named.slug,
            None => ids::slug(&entry.title),
        };
        self.write_as(entry, &slug, previous)
    }

    /// Write one entry under an explicit slug.
    pub fn write_as(&self, entry: &Entry, slug: &str, previous: Option<&Path>) -> Result<PathBuf> {
        let path = self.ctx().entry_path(&entry.r#ref, slug);
        let data = serde_json::to_string_pretty(entry).map_err(|e| parse_err("entry", e))?;
        atomic_write(&path, format!("{data}\n").as_bytes())?;
        // After the new file exists, never before: a crash between the two
        // leaves two files sharing one ref, which `tk check` reports, rather
        // than no file at all.
        if let Some(old) = previous
            && old != path
            && old.exists()
        {
            fs::remove_file(old).map_err(StoreError::Io)?;
        }
        Ok(path)
    }

    /// Create an entry: allocate a free ref, derive the slug, write the file.
    pub fn create(&self, title: &str, now: &str) -> Result<EntryView> {
        let title = title.trim();
        if title.is_empty() {
            return Err(StoreError::EmptyTitle);
        }
        let known = self.known()?;
        let r#ref = self.free_ref(&known)?;
        let entry = Entry::new(r#ref, title.to_owned(), now.to_owned());
        let slug = ids::slug(&entry.title);
        let path = self.write_as(&entry, &slug, None)?;
        Ok(view_of(&entry, &file_name(&path), &known))
    }

    /// Delete an entry file, returning the path it lived at.
    pub fn delete(&self, r#ref: &str) -> Result<PathBuf> {
        let (path, _) = self.load(r#ref)?;
        fs::remove_file(&path).map_err(StoreError::Io)?;
        Ok(path)
    }

    fn free_ref(&self, known: &[Known]) -> Result<String> {
        for _ in 0..100 {
            let candidate = ids::new_ref();
            if !known.iter().any(|k| k.r#ref == candidate) {
                return Ok(candidate);
            }
        }
        Err(StoreError::RefCollisions(100))
    }
}

// ---------------------------------------------------------------------------
// Integrity
// ---------------------------------------------------------------------------

/// Everything wrong with the store, as sentences. Empty means clean.
///
/// Read-only: nothing is repaired. `tk check` prints these.
pub fn check_integrity(ctx: &Ctx) -> Result<Vec<String>> {
    ctx.require()?;
    let store = Store { ctx };
    let scan = store.scan()?;
    let mut issues = scan.issues;

    let refs: HashSet<&str> = scan.entries.iter().map(|(_, e)| e.r#ref.as_str()).collect();
    for (path, entry) in &scan.entries {
        let name = file_name(path);
        for blocker in &entry.blocked_by {
            if blocker == &entry.r#ref {
                issues.push(format!("{name}: blocks itself"));
            } else if !refs.contains(blocker.as_str()) {
                issues.push(format!(
                    "{name}: blocked_by names {blocker}, which is not here"
                ));
            }
        }
        if entry.state == State::Open && entry.done.is_some() {
            issues.push(format!("{name}: state is open but it has a done time"));
        }
        if entry.state.is_closed() && entry.done.is_none() {
            issues.push(format!(
                "{name}: state is {} but it has no done time",
                entry.state
            ));
        }
    }

    issues.extend(cycles(&scan.entries));
    Ok(issues)
}

/// Report every `blocked_by` loop once, as a readable path.
fn cycles(entries: &[(PathBuf, Entry)]) -> Vec<String> {
    let graph: HashMap<&str, &[String]> = entries
        .iter()
        .map(|(_, e)| (e.r#ref.as_str(), e.blocked_by.as_slice()))
        .collect();
    let mut found: BTreeMap<Vec<&str>, Vec<String>> = BTreeMap::new();

    fn walk<'g>(
        node: &'g str,
        graph: &'g HashMap<&'g str, &'g [String]>,
        stack: &mut Vec<&'g str>,
        found: &mut BTreeMap<Vec<&'g str>, Vec<String>>,
    ) {
        if stack.contains(&node) {
            let start = stack.iter().position(|n| *n == node).unwrap_or(0);
            let mut loop_refs: Vec<&str> = stack[start..].to_vec();
            loop_refs.push(node);
            let mut key = loop_refs.clone();
            key.sort_unstable();
            key.dedup();
            found
                .entry(key)
                .or_insert_with(|| loop_refs.iter().map(|r| (*r).to_owned()).collect());
            return;
        }
        if stack.len() > 64 {
            return;
        }
        stack.push(node);
        for next in graph.get(node).copied().unwrap_or_default() {
            if graph.contains_key(next.as_str()) {
                walk(next, graph, stack, found);
            }
        }
        stack.pop();
    }

    let mut out = Vec::new();
    for (_, entry) in entries {
        let mut stack = Vec::new();
        walk(&entry.r#ref, &graph, &mut stack, &mut found);
    }
    for (_key, cycle) in found {
        out.push(format!("blocking loop: {}", cycle.join(" -> ")));
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The file's own name, for messages and for `file` in a view.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Write a file so a reader sees either the old contents or the new ones.
///
/// Temp file in the same directory, `fsync` the data, rename over the target,
/// then `fsync` the directory so the rename itself survives a power loss.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Msg("no parent dir".into()))?;
    let tmp = parent.join(format!(".tmp.{}-{}", std::process::id(), ids::new_ref()));
    let mut f = fs::File::create(&tmp).map_err(StoreError::Io)?;
    f.write_all(content).map_err(StoreError::Io)?;
    f.sync_all().map_err(StoreError::Io)?;
    drop(f);
    fs::rename(&tmp, path).map_err(StoreError::Io)?;
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Ctx) {
        let dir = tempfile::tempdir().expect("temp dir");
        let ctx = Ctx::at_tasks_dir(dir.path().join(TASKS_DIR));
        ctx.txn_init().expect("init");
        (dir, ctx)
    }

    #[test]
    fn a_scan_on_a_missing_store_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Ctx::at_tasks_dir(dir.path().join(TASKS_DIR));
        let scan = Store { ctx: &ctx }.scan().expect("scan");
        assert!(scan.entries.is_empty());
        assert!(scan.issues.is_empty());
    }

    #[test]
    fn create_then_read_back() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let view = txn
            .create("Rewrite the auth layer", "2026-01-10T12:00:00Z")
            .unwrap();
        assert_eq!(view.entry.title, "Rewrite the auth layer");
        assert_eq!(view.entry.state, State::Open);
        assert!(ids::is_valid_ref(&view.entry.r#ref));
        assert_eq!(
            view.file,
            format!("{}-rewrite-the-auth-layer.json", view.entry.r#ref)
        );
        assert!(view.is_ready());

        let store = ctx.store().unwrap();
        let loaded = store.get(&view.entry.r#ref).unwrap();
        assert_eq!(loaded.entry, view.entry);
        assert_eq!(loaded.rev, view.rev, "rev is stable across a read");
    }

    #[test]
    fn the_document_on_disk_is_pretty_printed_with_a_trailing_newline() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let view = txn.create("One line", "2026-01-10T12:00:00Z").unwrap();
        let raw = fs::read_to_string(ctx.tasks_dir.join(&view.file)).unwrap();
        assert!(raw.ends_with("}\n"), "{raw:?}");
        assert!(
            raw.contains("\n  \"ref\""),
            "pretty-printed, two-space indent"
        );
    }

    #[test]
    fn an_empty_title_is_refused() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        assert!(matches!(
            txn.create("   ", "2026-01-10T12:00:00Z"),
            Err(StoreError::EmptyTitle)
        ));
    }

    #[test]
    fn writing_over_a_title_change_keeps_the_slug_and_the_file() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let view = txn
            .create("Rewrite the auth layer", "2026-01-10T12:00:00Z")
            .unwrap();
        let path = ctx.tasks_dir.join(&view.file);

        let (old_path, mut entry) = txn.load(&view.entry.r#ref).unwrap();
        entry.title = "Rewrite the auth layer, properly".into();
        Txn::touch(&mut entry, "2026-02-01T00:00:00Z");
        let new_path = txn.write(&entry, Some(&old_path)).unwrap();

        assert_eq!(new_path, path, "a title change does not move the file");
        let reread = ctx.store().unwrap().get(&view.entry.r#ref).unwrap();
        assert_eq!(reread.entry.title, "Rewrite the auth layer, properly");
        assert_eq!(
            reread.entry.created, view.entry.created,
            "created never moves"
        );
    }

    #[test]
    fn an_explicit_slug_moves_the_file_and_leaves_nothing_behind() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let view = txn.create("Widget", "2026-01-10T12:00:00Z").unwrap();
        let old_path = ctx.tasks_dir.join(&view.file);

        let (path, entry) = txn.load(&view.entry.r#ref).unwrap();
        let moved = txn.write_as(&entry, "gadget", Some(&path)).unwrap();
        assert_eq!(file_name(&moved), format!("{}-gadget.json", entry.r#ref));
        assert!(!old_path.exists(), "the old file is gone");
        assert_eq!(ctx.store().unwrap().known().unwrap().len(), 1);
    }

    #[test]
    fn refs_are_unique_and_resolvable_by_title() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let a = txn
            .create("Rewrite the auth layer", "2026-01-10T12:00:00Z")
            .unwrap();
        let b = txn
            .create("Write the parser", "2026-01-11T12:00:00Z")
            .unwrap();
        assert_ne!(a.entry.r#ref, b.entry.r#ref);

        let store = ctx.store().unwrap();
        assert_eq!(store.resolve(&a.entry.r#ref).unwrap(), a.entry.r#ref);
        assert_eq!(store.resolve("parser").unwrap(), b.entry.r#ref);
        assert!(matches!(
            store.resolve("nope"),
            Err(StoreError::Id(IdError::NotFound(_)))
        ));
    }

    #[test]
    fn listing_shows_open_entries_oldest_first_and_hides_closed_ones() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let newer = txn.create("Newer", "2026-02-01T00:00:00Z").unwrap();
        let older = txn.create("Older", "2026-01-01T00:00:00Z").unwrap();
        let mut done = txn.load(&newer.entry.r#ref).unwrap().1;
        done.state = State::Done;
        done.done = Some("2026-02-02T00:00:00Z".into());
        txn.write(&done, None).unwrap();

        let store = ctx.store().unwrap();
        let (views, issues) = store.list(&Filter::default()).unwrap();
        assert!(issues.is_empty());
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].entry.r#ref, older.entry.r#ref);

        let (all, _) = store
            .list(&Filter {
                include_closed: true,
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].entry.r#ref, older.entry.r#ref, "oldest first");
    }

    #[test]
    fn filter_by_label_search_state_and_readiness() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let mut a = txn
            .load(
                &txn.create("Alpha", "2026-01-01T00:00:00Z")
                    .unwrap()
                    .entry
                    .r#ref,
            )
            .unwrap()
            .1;
        a.labels = vec!["backend".into(), "needs-review".into()];
        txn.write(&a, None).unwrap();
        let b = txn.create("Beta", "2026-01-02T00:00:00Z").unwrap();
        let mut blocking = txn.load(&b.entry.r#ref).unwrap().1;
        blocking.blocked_by = vec![a.r#ref.clone()];
        txn.write(&blocking, None).unwrap();

        let store = ctx.store().unwrap();
        let count = |f: Filter| store.list(&f).unwrap().0.len();
        assert_eq!(
            count(Filter {
                label: "backend".into(),
                ..Default::default()
            }),
            1
        );
        assert_eq!(
            count(Filter {
                search: "ALPHA".into(),
                ..Default::default()
            }),
            1
        );
        assert_eq!(
            count(Filter {
                search: "review".into(),
                ..Default::default()
            }),
            1
        );
        assert_eq!(
            count(Filter {
                state: Some(State::Open),
                ..Default::default()
            }),
            2
        );
        assert_eq!(
            count(Filter {
                state: Some(State::Open),
                blocked: Some(false),
                ..Default::default()
            }),
            1,
            "a blocked entry is not ready"
        );
        assert_eq!(
            count(Filter {
                blocked: Some(true),
                ..Default::default()
            }),
            1
        );
        assert_eq!(
            count(Filter {
                limit: 1,
                ..Default::default()
            }),
            1
        );
    }

    #[test]
    fn check_reports_what_is_wrong_instead_of_hiding_it() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let a = txn.create("Alpha", "2026-01-01T00:00:00Z").unwrap();
        let b = txn.create("Beta", "2026-01-02T00:00:00Z").unwrap();

        // Dirty the store by hand, the way an editor or an interrupted write would.
        let path = ctx.tasks_dir.join(&a.file);
        let mut entry: Entry = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        entry.blocked_by = vec![b.entry.r#ref.clone(), "zzzz".into(), entry.r#ref.clone()];
        fs::write(&path, serde_json::to_string_pretty(&entry).unwrap()).unwrap();
        fs::write(ctx.tasks_dir.join("stray.json"), "{}").unwrap();
        fs::write(ctx.tasks_dir.join("a7b3-broken.json"), "{not json").unwrap();

        let issues = check_integrity(&ctx).unwrap();
        let all = issues.join("\n");
        for expected in [
            "blocks itself",
            "zzzz, which is not here",
            "stray.json",
            "a7b3-broken.json",
        ] {
            assert!(all.contains(expected), "{expected} missing from:\n{all}");
        }
    }

    #[test]
    fn check_reports_debris_a_crashed_write_left_behind() {
        // A temp file from an interrupted write and a stray file: neither is an
        // entry, and both used to be invisible to `check`.
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        txn.create("Alpha", "2026-01-01T00:00:00Z").unwrap();
        fs::write(ctx.tasks_dir.join(".tmp.9999-abcd"), "{").unwrap();
        fs::write(ctx.tasks_dir.join("NOTES.txt"), "scratch").unwrap();

        let issues = check_integrity(&ctx).unwrap();
        let all = issues.join("\n");
        for expected in [".tmp.9999-abcd", "NOTES.txt"] {
            assert!(all.contains(expected), "{expected} missing from:\n{all}");
        }

        // A directory is not debris: a store may hold documents beside its
        // entries, and a migration's `legacy/` and `MIGRATION.md` are expected.
        fs::remove_file(ctx.tasks_dir.join(".tmp.9999-abcd")).unwrap();
        fs::remove_file(ctx.tasks_dir.join("NOTES.txt")).unwrap();
        fs::create_dir_all(ctx.tasks_dir.join("reports")).unwrap();
        fs::create_dir_all(ctx.tasks_dir.join("legacy")).unwrap();
        fs::write(ctx.tasks_dir.join("MIGRATION.md"), "# map").unwrap();
        assert!(
            check_integrity(&ctx).unwrap().is_empty(),
            "{:?}",
            check_integrity(&ctx).unwrap()
        );
    }

    #[test]
    fn check_finds_a_blocking_loop_however_long() {
        let entries: Vec<(PathBuf, Entry)> = ["a7b3", "b7c4", "c7d5"]
            .iter()
            .enumerate()
            .map(|(i, r#ref)| {
                let mut entry = Entry::new((*r#ref).into(), "t".into(), "c".into());
                let next = ["a7b3", "b7c4", "c7d5"][(i + 1) % 3];
                entry.blocked_by = vec![next.to_owned()];
                (PathBuf::from(format!("{name}.json", name = r#ref)), entry)
            })
            .collect();
        let issues = cycles(&entries);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].contains("a7b3"), "{issues:?}");
        assert!(issues[0].contains("->"), "{issues:?}");
    }

    #[test]
    fn state_coherence_is_checked() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let a = txn.create("Alpha", "2026-01-01T00:00:00Z").unwrap();
        let path = ctx.tasks_dir.join(&a.file);
        let mut entry: Entry = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        entry.done = Some("2026-01-02T00:00:00Z".into());
        fs::write(&path, serde_json::to_string_pretty(&entry).unwrap()).unwrap();
        let all = check_integrity(&ctx).unwrap().join("\n");
        assert!(all.contains("open but it has a done time"), "{all}");

        entry.state = State::Done;
        entry.done = None;
        fs::write(&path, serde_json::to_string_pretty(&entry).unwrap()).unwrap();
        let all = check_integrity(&ctx).unwrap().join("\n");
        assert!(all.contains("no done time"), "{all}");
    }

    #[test]
    fn the_format_gate_names_what_it_found() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join(TASKS_DIR);
        fs::create_dir_all(&tasks).unwrap();
        let ctx = Ctx::at_tasks_dir(tasks.clone());

        // No store file at all.
        match ctx.check_format() {
            Err(StoreError::NotStore { detail, format, .. }) => {
                assert_eq!(format, FORMAT);
                assert!(detail.contains(STORE_FILE), "{detail}");
            }
            other => panic!("expected refusal, got {other:?}"),
        }

        // A v1 layout.
        fs::write(tasks.join("store.json"), r#"{"format":2}"#).unwrap();
        match ctx.check_format() {
            Err(StoreError::NotStore { detail, .. }) => assert!(detail.contains("v1"), "{detail}"),
            other => panic!("expected refusal, got {other:?}"),
        }
        assert_eq!(ctx.check_format().unwrap_err().code(), code::NOT_A_STORE);

        // A v0 layout.
        fs::remove_file(tasks.join("store.json")).unwrap();
        fs::write(tasks.join("config.json"), "{}").unwrap();
        match ctx.check_format() {
            Err(StoreError::NotStore { detail, .. }) => assert!(detail.contains("v0"), "{detail}"),
            other => panic!("expected refusal, got {other:?}"),
        }

        // A v0 store that never had a config file: its task documents are the
        // store, and the refusal must say so rather than calling them strays.
        fs::remove_file(tasks.join("config.json")).unwrap();
        fs::write(
            tasks.join("tk-a7b3.json"),
            r#"{"project":"tk","ref":"a7b3","title":"Alpha","status":"open"}"#,
        )
        .unwrap();
        match ctx.check_format() {
            Err(StoreError::NotStore { detail, .. }) => {
                assert!(detail.contains("task documents"), "{detail}");
                assert!(detail.contains("v0"), "{detail}");
            }
            other => panic!("expected refusal, got {other:?}"),
        }
        fs::remove_file(tasks.join("tk-a7b3.json")).unwrap();

        // A wrong format number in our own file.
        fs::write(tasks.join(STORE_FILE), r#"{"format":9}"#).unwrap();
        match ctx.check_format() {
            Err(StoreError::NotStore { detail, .. }) => {
                assert!(detail.contains("format 9"), "{detail}")
            }
            other => panic!("expected refusal, got {other:?}"),
        }

        fs::write(tasks.join(STORE_FILE), format!(r#"{{"format":{FORMAT}}}"#)).unwrap();
        assert!(ctx.check_format().is_ok());
    }

    #[test]
    fn init_is_idempotent_and_refuses_nothing_it_wrote() {
        let (_dir, ctx) = store();
        ctx.txn_init().unwrap();
        assert_eq!(ctx.load_config().unwrap().format, FORMAT);
        assert!(ctx.tasks_dir.join(".gitignore").exists());
    }

    #[test]
    fn revocation_changes_when_the_content_does() {
        let mut entry = Entry::new("a7b3".into(), "t".into(), "2026-01-01T00:00:00Z".into());
        let before = fingerprint(&entry);
        entry.labels = vec!["working".into()];
        assert_ne!(fingerprint(&entry), before);
    }

    #[test]
    fn stale_revision_is_refused() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let view = txn.create("Alpha", "2026-01-01T00:00:00Z").unwrap();
        let store = ctx.store().unwrap();
        assert!(store.check_rev(&view.entry.r#ref, Some(&view.rev)).is_ok());
        assert!(matches!(
            store.check_rev(&view.entry.r#ref, Some("deadbeef")),
            Err(StoreError::StaleRevision { .. })
        ));
        assert!(store.check_rev(&view.entry.r#ref, None).is_ok());
    }

    #[test]
    fn unknown_keys_survive_a_tk_write() {
        let (_dir, ctx) = store();
        let txn = ctx.txn().unwrap();
        let view = txn.create("Alpha", "2026-01-01T00:00:00Z").unwrap();
        let path = ctx.tasks_dir.join(&view.file);
        let mut raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        raw["assignee"] = serde_json::json!("nick");
        fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

        let (path, mut entry) = txn.load(&view.entry.r#ref).unwrap();
        entry.labels = vec!["picked-up".into()];
        Txn::touch(&mut entry, "2026-01-02T00:00:00Z");
        txn.write(&entry, Some(&path)).unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["assignee"], serde_json::json!("nick"));
        assert_eq!(raw["labels"], serde_json::json!(["picked-up"]));
    }
}
