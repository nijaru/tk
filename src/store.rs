//! Filesystem store: store resolution, mutation locking, atomic writes, config,
//! and all task operations. Everything hangs off [`Ctx`]; mutations run inside a
//! [`Txn`], which holds one advisory lock over the whole
//! resolve → read → validate → edit → persist operation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::ids::{self, IdError, TaskId};
use crate::model::{Config, Priority, Status, Task, TaskView};
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
        "no .tasks/ store for {start}\n{boundary} is a linked worktree or submodule checkout, so tk will not create a separate store there.\nRun 'tk init' to create one deliberately, or set --tasks-dir/TK_TASKS_DIR to a shared store."
    )]
    WorktreeBootstrap { start: String, boundary: String },
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
    #[error("failed to create task after {0} attempts (ref collisions)")]
    RefCollisions(u32),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {what}: {err}")]
    Parse { what: String, err: String },
    #[error("invalid task ID: {0}")]
    UnsafeId(String),
    #[error("{0}")]
    Id(#[from] IdError),
    #[error("{0}")]
    Msg(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

fn parse_err(what: impl Into<String>, err: impl ToString) -> StoreError {
    StoreError::Parse {
        what: what.into(),
        err: err.to_string(),
    }
}

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

// ---------------------------------------------------------------------------
// Context: where the store is
// ---------------------------------------------------------------------------

pub const TASKS_DIR: &str = ".tasks";
const CONFIG_FILE: &str = "config.json";
const LOCK_FILE: &str = ".lock";

/// Environment variable naming the task store directory (see `--tasks-dir`).
pub const TASKS_DIR_ENV: &str = "TK_TASKS_DIR";

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
                // Linked worktree or submodule: deliberately do not bootstrap a
                // second store here.
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
        let data = fs::read(self.config_path()).ok()?;
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

    pub fn require(&self) -> Result<()> {
        if self.exists {
            Ok(())
        } else {
            Err(self.missing_store_error())
        }
    }

    /// Guard read commands: a store selected explicitly must exist, while an
    /// undiscovered store simply has nothing to show.
    pub fn require_for_read(&self) -> Result<()> {
        if self.source.is_explicit() && !self.tasks_dir.is_dir() {
            return Err(self.missing_store_error());
        }
        Ok(())
    }

    fn bootstrap_allowed(&self) -> Result<()> {
        match self.source {
            StoreSource::Flag | StoreSource::Env => Err(self.missing_store_error()),
            StoreSource::Discovered if self.worktree => Err(StoreError::WorktreeBootstrap {
                start: self.cwd.display().to_string(),
                boundary: self.root.display().to_string(),
            }),
            StoreSource::Discovered => Ok(()),
        }
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.tasks_dir.join(format!("{id}.json"))
    }

    pub fn config_path(&self) -> PathBuf {
        self.tasks_dir.join(CONFIG_FILE)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.tasks_dir.join(LOCK_FILE)
    }

    // -- transactions ------------------------------------------------------

    /// Open a mutation transaction on an existing store.
    pub fn txn(&self) -> Result<Txn<'_>> {
        self.require()?;
        let lock = self.lock_mutation()?;
        Ok(Txn {
            ctx: self,
            _lock: lock,
        })
    }

    /// Open a transaction for a store that may not exist yet (`tk add`).
    ///
    /// An explicitly designated store is never created implicitly, and a linked
    /// worktree never silently grows its own store.
    pub fn txn_bootstrap(&self) -> Result<Txn<'_>> {
        if !self.exists {
            self.bootstrap_allowed()?;
            fs::create_dir_all(&self.tasks_dir).map_err(StoreError::Io)?;
            ensure_gitignore(&self.tasks_dir);
        }
        let lock = self.lock_mutation()?;
        Ok(Txn {
            ctx: self,
            _lock: lock,
        })
    }

    /// Open a transaction for `tk init`, which creates the store deliberately —
    /// including an explicitly designated one.
    pub fn txn_init(&self) -> Result<Txn<'_>> {
        fs::create_dir_all(&self.tasks_dir).map_err(StoreError::Io)?;
        ensure_gitignore(&self.tasks_dir);
        let lock = self.lock_mutation()?;
        Ok(Txn {
            ctx: self,
            _lock: lock,
        })
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

    // -- config ------------------------------------------------------------

    pub fn load_config(&self) -> Result<Config> {
        self.require_for_read()?;
        match fs::read(self.config_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(StoreError::Io(e)),
            Ok(data) => serde_json::from_slice(&data)
                .map_err(|e| parse_err("config", e))
                .map(|mut c: Config| {
                    // Fill defaults for missing keys (serde `default` mostly
                    // covers this; belt-and-braces for empty objects).
                    if c.project.is_empty() {
                        c.project = "tk".to_owned();
                    }
                    c
                }),
        }
    }
}

// ---------------------------------------------------------------------------
// Mutation guard
// ---------------------------------------------------------------------------

/// Advisory exclusive lock over one store's mutation boundary.
///
/// Serializes cooperating `tk` processes for the whole read/modify/write
/// operation. It does not constrain editors, a Git checkout, or an older `tk`
/// binary, and it cannot coordinate separate clones or machines.
pub struct StoreLock {
    file: fs::File,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// A complete mutation operation: holds the store lock until dropped.
///
/// Do not open two transactions against one store in a process; the advisory
/// lock is not reentrant across file handles.
pub struct Txn<'a> {
    ctx: &'a Ctx,
    _lock: StoreLock,
}

impl<'a> Txn<'a> {
    pub fn ctx(&self) -> &'a Ctx {
        self.ctx
    }

    /// Resolve a user-supplied ID/prefix/ref against the store.
    pub fn resolve(&self, input: &str) -> Result<String> {
        Ok(ids::resolve_id(&self.ctx.tasks_dir, input)?)
    }

    /// Read a task without repairing anything.
    pub fn load(&self, id: &str) -> Result<Task> {
        read_task_file(&self.ctx.task_path(id))
    }

    /// Read a task into its computed view plus any inconsistencies on disk.
    pub fn get(&self, id: &str) -> Result<(TaskView, Vec<String>)> {
        let task = self.load(id)?;
        let issues = inconsistencies(self.ctx, &task, id);
        Ok((self.view(&task), issues))
    }

    /// Persist a task record. Callers set `updated_at` themselves.
    pub fn save(&self, task: &Task) -> Result<()> {
        write_task(self.ctx, task)
    }

    /// Reject the operation unless the record still carries `expected`.
    pub fn check_rev(&self, task: &Task, expected: Option<&str>) -> Result<()> {
        if let Some(expected) = expected {
            let found = fingerprint(task);
            if found != expected {
                return Err(StoreError::StaleRevision {
                    id: task.id(),
                    expected: expected.to_owned(),
                    found,
                });
            }
        }
        Ok(())
    }

    pub fn view(&self, task: &Task) -> TaskView {
        enrich(self.ctx, task, &HashMap::new())
    }

    // -- config ------------------------------------------------------------

    pub fn load_config(&self) -> Result<Config> {
        self.ctx.load_config()
    }

    pub fn save_config(&self, config: &Config) -> Result<()> {
        let data =
            serde_json::to_string_pretty(config).map_err(|e| parse_err("marshal config", e))?;
        atomic_write(&self.ctx.config_path(), data.as_bytes())
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) -> Result<Config> {
        let mut c = self.load_config()?;
        f(&mut c);
        self.save_config(&c)?;
        Ok(c)
    }

    // -- create ------------------------------------------------------------

    pub fn create(&self, opts: CreateOptions) -> Result<TaskView> {
        if opts.title.trim().is_empty() {
            return Err(StoreError::EmptyTitle);
        }
        let mut config = self.load_config().unwrap_or_default();
        let project = opts.project.unwrap_or_else(|| config.project.clone());
        ids::validate_project(&project).map_err(|e| match e {
            IdError::BadProject(p) => StoreError::Msg(format!(
                "invalid project name {p:?}: use lowercase letters, digits, and internal hyphens"
            )),
            e => StoreError::Msg(e.to_string()),
        })?;
        if !self.ctx.config_path().exists() {
            config.project.clone_from(&project);
            self.save_config(&config)?;
        }

        for _ in 0..10u32 {
            let r#ref = ids::generate_ref();
            let now = timeutil::now_rfc3339_nano();
            let task = Task {
                project: project.clone(),
                r#ref,
                title: opts.title.clone(),
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
                parent: opts.parent.clone(),
                blocked_by: Vec::new(),
                estimate: opts.estimate,
                due_date: opts.due_date.clone(),
                logs: Vec::new(),
                created_at: now.clone(),
                updated_at: now,
                completed_at: None,
            };
            let path = self.ctx.task_path(&task.id());
            if path.exists() {
                continue; // ref collision — retry
            }
            // Exclusive create.
            let data =
                serde_json::to_string_pretty(&task).map_err(|e| parse_err("marshal task", e))?;
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(StoreError::Io(e)),
                Ok(mut f) => {
                    f.write_all(data.as_bytes()).map_err(StoreError::Io)?;
                    f.sync_all().map_err(StoreError::Io)?;
                }
            }
            return Ok(self.view(&task));
        }
        Err(StoreError::RefCollisions(10))
    }

    // -- edit --------------------------------------------------------------

    pub fn update_status(&self, id: &str, status: Status) -> Result<TaskView> {
        let mut task = self.load(id)?;
        if task.status != status {
            let now = timeutil::now_rfc3339_nano();
            task.status = status;
            task.updated_at = now.clone();
            task.completed_at = if status == Status::Done {
                Some(now)
            } else {
                None
            };
            self.save(&task)?;
        }
        Ok(self.view(&task))
    }

    pub fn update(&self, id: &str, u: UpdateOptions) -> Result<TaskView> {
        let mut task = self.load(id)?;
        self.check_rev(&task, u.expect_rev.as_deref())?;
        let mut modified = false;
        if let Some(t) = u.title {
            task.title = t;
            modified = true;
        }
        if let Some(d) = u.description {
            task.description = d;
            modified = true;
        }
        if let Some(p) = u.priority {
            task.priority = p;
            modified = true;
        }
        if let Some(l) = u.labels {
            task.labels = l;
            modified = true;
        }
        if let Some(a) = u.assignees {
            task.assignees = a;
            modified = true;
        }
        if let Some(p) = u.parent {
            task.parent = p;
            modified = true;
        }
        if let Some(e) = u.estimate {
            task.estimate = e;
            modified = true;
        }
        if let Some(d) = u.due_date {
            task.due_date = d;
            modified = true;
        }
        if modified {
            task.updated_at = timeutil::now_rfc3339_nano();
            self.save(&task)?;
        }
        Ok(self.view(&task))
    }

    pub fn add_log(&self, id: &str, msg: &str) -> Result<TaskView> {
        use crate::model::LogEntry;
        let mut task = self.load(id)?;
        let now = timeutil::now_rfc3339_nano();
        task.logs.push(LogEntry {
            ts: now.clone(),
            msg: msg.to_owned(),
        });
        task.updated_at = now;
        self.save(&task)?;
        Ok(self.view(&task))
    }

    pub fn add_blocker(&self, id: &str, blocker: &str) -> Result<TaskView> {
        let mut task = self.load(id)?;
        task.blocked_by.push(blocker.to_owned());
        task.updated_at = timeutil::now_rfc3339_nano();
        self.save(&task)?;
        Ok(self.view(&task))
    }

    pub fn remove_blocker(&self, id: &str, blocker: &str) -> Result<(TaskView, bool)> {
        let mut task = self.load(id)?;
        let before = task.blocked_by.len();
        task.blocked_by.retain(|b| b != blocker);
        let found = task.blocked_by.len() != before;
        if found {
            task.updated_at = timeutil::now_rfc3339_nano();
            self.save(&task)?;
        }
        Ok((self.view(&task), found))
    }

    // -- delete & repair ---------------------------------------------------

    /// Delete a task and scrub references to it from survivors.
    pub fn remove(&self, id: &str, expect_rev: Option<&str>) -> Result<RemoveOutcome> {
        if !ids::is_safe_id(id) {
            return Err(StoreError::UnsafeId(id.to_owned()));
        }
        let task = self.load(id)?;
        self.check_rev(&task, expect_rev)?;
        fs::remove_file(self.ctx.task_path(id)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StoreError::TaskNotFound(id.to_owned())
            } else {
                StoreError::Io(e)
            }
        })?;
        let mut scrubbed = 0usize;
        for mut t in all_tasks(self.ctx)? {
            let mut modified = false;
            let before = t.blocked_by.len();
            t.blocked_by.retain(|b| b != id);
            if t.blocked_by.len() != before {
                scrubbed += before - t.blocked_by.len();
                modified = true;
            }
            if t.parent.as_deref() == Some(id) {
                t.parent = None;
                scrubbed += 1;
                modified = true;
            }
            if modified {
                write_task(self.ctx, &t)?;
            }
        }
        Ok(RemoveOutcome {
            id: id.to_owned(),
            references_scrubbed: scrubbed,
        })
    }

    /// Apply the mechanical repairs a read-only query only reports:
    /// filename ↔ content ID mismatch, and (when asked) dangling references.
    pub fn repair(&self, id: &str, drop_missing: bool) -> Result<RepairOutcome> {
        let mut task = self.load(id)?;
        let mut out = RepairOutcome {
            id: id.to_owned(),
            ..Default::default()
        };
        let mut modified = false;

        if task.id() != id
            && let Some(parsed) = TaskId::parse(id)
        {
            out.renamed_id = Some(task.id());
            task.project = parsed.project;
            task.r#ref = parsed.r#ref;
            modified = true;
        }

        if drop_missing {
            let before = task.blocked_by.len();
            task.blocked_by.retain(|b| {
                let ok = ids::is_safe_id(b) && self.ctx.task_path(b).exists();
                if !ok {
                    out.dropped_blockers.push(b.clone());
                }
                ok
            });
            modified |= task.blocked_by.len() != before;

            if let Some(p) = task.parent.clone()
                && (!ids::is_safe_id(&p) || !self.ctx.task_path(&p).exists())
            {
                out.dropped_parent = Some(p);
                task.parent = None;
                modified = true;
            }
        }

        if modified {
            task.updated_at = timeutil::now_rfc3339_nano();
            self.save(&task)?;
        }
        Ok(out)
    }

    // -- moves & renames ---------------------------------------------------

    pub fn rename_project(&self, old: &str, new: &str) -> Result<RenameResult> {
        ids::validate_project(new).map_err(|e| StoreError::Msg(e.to_string()))?;
        if old == new {
            return Err(StoreError::Msg(format!(
                "project {old:?} is already named that"
            )));
        }
        let config = self.load_config()?;
        let tasks = all_tasks(self.ctx)?;

        let to_rename: Vec<&Task> = tasks.iter().filter(|t| t.project == old).collect();
        if to_rename.is_empty() {
            return Err(StoreError::Msg(format!(
                "no tasks found with project {old:?}"
            )));
        }
        let existing: HashSet<String> = tasks.iter().map(|t| t.id()).collect();
        let mut id_map = HashMap::new();
        for t in &to_rename {
            let new_id = format!("{new}-{}", t.r#ref);
            if existing.contains(&new_id) {
                return Err(StoreError::Msg(format!(
                    "cannot rename: {new_id:?} already exists"
                )));
            }
            id_map.insert(t.id(), new_id);
        }

        let mut res = RenameResult {
            renamed: Vec::with_capacity(to_rename.len()),
            references_updated: 0,
        };
        for mut t in tasks {
            if !ids::is_safe_id(&t.id()) {
                return Err(StoreError::UnsafeId(t.id()));
            }
            let mut modified = false;
            for b in &mut t.blocked_by {
                if let Some(n) = id_map.get(b) {
                    *b = n.clone();
                    res.references_updated += 1;
                    modified = true;
                }
            }
            if let Some(p) = &t.parent
                && let Some(n) = id_map.get(p)
            {
                t.parent = Some(n.clone());
                res.references_updated += 1;
                modified = true;
            }
            if t.project == old {
                let old_path = self.ctx.task_path(&t.id());
                t.project = new.to_owned();
                let new_path = self.ctx.task_path(&t.id());
                res.renamed.push(t.id());
                // Move first so a later write failure can't leave both IDs behind.
                fs::rename(&old_path, &new_path).map_err(StoreError::Io)?;
                write_task(self.ctx, &t)
                    .map_err(|e| StoreError::Msg(format!("update moved task {}: {e}", t.id())))?;
            } else if modified {
                write_task(self.ctx, &t)?;
            }
        }
        if config.project == old {
            self.update_config(|c| c.project = new.to_owned())?;
        }
        Ok(res)
    }

    pub fn move_task(&self, id: &str, new_project: &str) -> Result<MoveResult> {
        ids::validate_project(new_project).map_err(|e| StoreError::Msg(e.to_string()))?;
        if !ids::is_safe_id(id) {
            return Err(StoreError::UnsafeId(id.to_owned()));
        }
        let mut task = self.load(id)?;
        if task.project == new_project {
            return Err(StoreError::Msg(format!(
                "task {id} is already in project {new_project:?}"
            )));
        }
        let new_id = format!("{new_project}-{}", task.r#ref);
        if !ids::is_safe_id(&new_id) {
            return Err(StoreError::UnsafeId(new_id));
        }
        if self.ctx.task_path(&new_id).exists() {
            return Err(StoreError::Msg(format!(
                "cannot move: {new_id:?} already exists"
            )));
        }
        task.project = new_project.to_owned();
        task.updated_at = timeutil::now_rfc3339_nano();
        fs::rename(self.ctx.task_path(id), self.ctx.task_path(&new_id)).map_err(StoreError::Io)?;
        write_task(self.ctx, &task)
            .map_err(|e| StoreError::Msg(format!("update moved task {}: {e}", task.id())))?;

        let mut res = MoveResult {
            old_id: id.to_owned(),
            new_id: new_id.clone(),
            references_updated: 0,
        };
        for mut t in all_tasks(self.ctx)? {
            let mut modified = false;
            for b in &mut t.blocked_by {
                if b == id {
                    *b = new_id.clone();
                    res.references_updated += 1;
                    modified = true;
                }
            }
            if t.parent.as_deref() == Some(id) {
                t.parent = Some(new_id.clone());
                res.references_updated += 1;
                modified = true;
            }
            if modified {
                write_task(self.ctx, &t)?;
            }
        }
        Ok(res)
    }

    // -- clean -------------------------------------------------------------

    pub fn clean(&self, days: i64) -> Result<usize> {
        if days < 0 {
            return Err(StoreError::Msg(
                "clean threshold must be non-negative".into(),
            ));
        }
        let tasks = all_tasks(self.ctx)?;
        let now = chrono::Utc::now();
        let threshold = chrono::Duration::days(days);

        let mut doomed = HashSet::new();
        for t in &tasks {
            if !ids::is_safe_id(&t.id()) {
                return Err(StoreError::UnsafeId(t.id()));
            }
            if t.status.is_terminal()
                && let Some(c) = t.completed_at.as_deref()
                && let Ok(comp) = chrono::DateTime::parse_from_rfc3339(c)
                && now.signed_duration_since(comp.with_timezone(&chrono::Utc)) > threshold
            {
                doomed.insert(t.id());
            }
        }
        for id in &doomed {
            let p = self.ctx.task_path(id);
            if p.exists() {
                fs::remove_file(p).map_err(StoreError::Io)?;
            }
        }
        for mut t in tasks {
            if doomed.contains(&t.id()) {
                continue;
            }
            let mut modified = false;
            let before = t.blocked_by.len();
            t.blocked_by.retain(|b| !doomed.contains(b));
            modified |= t.blocked_by.len() != before;
            if t.parent.as_deref().is_some_and(|p| doomed.contains(p)) {
                t.parent = None;
                modified = true;
            }
            if modified {
                write_task(self.ctx, &t)?;
            }
        }
        Ok(doomed.len())
    }
}

// ---------------------------------------------------------------------------
// Atomic write (temp file + fsync + rename + dir fsync)
// ---------------------------------------------------------------------------

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Msg("no parent dir".into()))?;
    let tmp = parent.join(format!(
        ".tmp.{}-{}",
        std::process::id(),
        ids::generate_ref()
    ));
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
// Task files
// ---------------------------------------------------------------------------

/// Content fingerprint used for stale-intent checks (`--if-rev`).
///
/// Derived from the record's canonical JSON, so it tracks task content rather
/// than file formatting. FNV-1a 64-bit: a change detector, not a security
/// primitive.
pub fn fingerprint(task: &Task) -> String {
    let bytes = serde_json::to_vec(task).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn read_task_file(path: &Path) -> Result<Task> {
    let data = fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            StoreError::TaskNotFound(name)
        } else {
            StoreError::Io(e)
        }
    })?;
    serde_json::from_slice(&data).map_err(|e| {
        parse_err(
            format!(
                "task {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            e,
        )
    })
}

fn write_task(ctx: &Ctx, task: &Task) -> Result<()> {
    if !ids::is_safe_id(&task.id()) {
        return Err(StoreError::UnsafeId(task.id()));
    }
    let data = serde_json::to_string_pretty(task).map_err(|e| parse_err("marshal task", e))?;
    atomic_write(&ctx.task_path(&task.id()), data.as_bytes())
}

/// Read a task into its computed view plus any inconsistencies on disk.
///
/// Read-only: nothing is repaired or persisted.
pub fn get_task(ctx: &Ctx, id: &str) -> Result<(TaskView, Vec<String>)> {
    ctx.require()?;
    let task = read_task_file(&ctx.task_path(id))?;
    let issues = inconsistencies(ctx, &task, id);
    Ok((enrich(ctx, &task, &HashMap::new()), issues))
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

pub fn all_tasks(ctx: &Ctx) -> Result<Vec<Task>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(&ctx.tasks_dir).map_err(StoreError::Io)?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() || !name.ends_with(".json") || name == CONFIG_FILE {
            continue;
        }
        out.push(read_task_file(&entry.path()));
    }
    // Propagate the first read error with context.
    let mut tasks = Vec::with_capacity(out.len());
    for t in out {
        tasks.push(t?);
    }
    Ok(tasks)
}

// ---------------------------------------------------------------------------
// Enrichment (computed view fields)
// ---------------------------------------------------------------------------

/// Report the inconsistencies a task record has on disk. Read-only: repairing
/// them is [`Txn::repair`].
pub fn inconsistencies(ctx: &Ctx, task: &Task, expected_id: &str) -> Vec<String> {
    let mut issues = Vec::new();
    if task.id() != expected_id {
        issues.push(format!(
            "file name {expected_id} does not match task ID {}",
            task.id()
        ));
    }
    for b in &task.blocked_by {
        if !ids::is_safe_id(b) || !ctx.task_path(b).exists() {
            issues.push(format!("blocked by missing task {b}"));
        }
    }
    if let Some(p) = &task.parent
        && (!ids::is_safe_id(p) || !ctx.task_path(p).exists())
    {
        issues.push(format!("parent task {p} is missing"));
    }
    issues
}

pub fn enrich(ctx: &Ctx, task: &Task, status_map: &HashMap<String, Status>) -> TaskView {
    let mut blocked_by_incomplete = false;
    let mut unresolved_blockers = Vec::new();
    for b in &task.blocked_by {
        if !ids::is_safe_id(b) {
            unresolved_blockers.push(b.clone());
            blocked_by_incomplete = true;
            continue;
        }
        let status = match status_map.get(b) {
            Some(s) => Some(*s),
            None => read_task_file(&ctx.task_path(b)).ok().map(|t| t.status),
        };
        match status {
            Some(s) if !s.is_terminal() => blocked_by_incomplete = true,
            Some(_) => {}
            None => {
                // A missing prerequisite is unresolved, not completed.
                unresolved_blockers.push(b.clone());
                blocked_by_incomplete = true;
            }
        }
    }
    let done = task.status.is_terminal();
    TaskView {
        id: task.id(),
        rev: fingerprint(task),
        blocked_by_incomplete,
        unresolved_blockers,
        is_overdue: timeutil::is_overdue(task.due_date.as_deref(), done),
        days_until_due: timeutil::days_until_due(task.due_date.as_deref(), done),
        task: task.clone(),
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
pub struct UpdateOptions {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<Priority>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub parent: Option<Option<String>>,
    pub estimate: Option<Option<i64>>,
    pub due_date: Option<Option<String>>,
    /// Reject the update unless the record still has this revision.
    pub expect_rev: Option<String>,
}

#[derive(Debug)]
pub struct RemoveOutcome {
    pub id: String,
    pub references_scrubbed: usize,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct RepairOutcome {
    pub id: String,
    /// Content ID that was rewritten to match the file name.
    pub renamed_id: Option<String>,
    pub dropped_blockers: Vec<String>,
    pub dropped_parent: Option<String>,
}

impl RepairOutcome {
    pub fn changed(&self) -> bool {
        self.renamed_id.is_some()
            || !self.dropped_blockers.is_empty()
            || self.dropped_parent.is_some()
    }
}

pub struct RenameResult {
    pub renamed: Vec<String>,
    pub references_updated: usize,
}

pub struct MoveResult {
    pub old_id: String,
    pub new_id: String,
    pub references_updated: usize,
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
    pub limit: usize,
}

fn status_rank(s: Status) -> u8 {
    match s {
        Status::Active => 0,
        Status::Open => 1,
        Status::Deferred => 2,
        Status::Done => 3,
        Status::Closed => 4,
    }
}

fn compare_tasks(a: &Task, b: &Task) -> std::cmp::Ordering {
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
        // Newest first.
        return b.created_at.cmp(&a.created_at);
    }
    // Terminal: newest completion first.
    b.completed_at.cmp(&a.completed_at)
}

pub fn list_tasks(ctx: &Ctx, opts: &ListOptions) -> Result<Vec<TaskView>> {
    if !ctx.exists {
        ctx.require_for_read()?;
        return Ok(Vec::new());
    }
    let tasks = all_tasks(ctx)?;
    let status_map: HashMap<String, Status> = tasks.iter().map(|t| (t.id(), t.status)).collect();

    let mut filtered: Vec<&Task> = tasks
        .iter()
        .filter(|t| {
            if let Some(s) = opts.status
                && t.status != s
            {
                return false;
            }
            if opts.hide_terminal && t.status.is_terminal() {
                return false;
            }
            if !opts.search.is_empty() {
                let q = opts.search.to_lowercase();
                let in_title = t.title.to_lowercase().contains(&q);
                let in_desc = t
                    .description
                    .as_deref()
                    .is_some_and(|d| d.to_lowercase().contains(&q));
                let in_id = t.id().to_lowercase().contains(&q);
                if !(in_title || in_desc || in_id) {
                    return false;
                }
            }
            if let Some(p) = opts.priority
                && t.priority != p
            {
                return false;
            }
            if !opts.project.is_empty() && t.project != opts.project {
                return false;
            }
            if !opts.label.is_empty() && !t.labels.iter().any(|l| l == &opts.label) {
                return false;
            }
            if !opts.assignee.is_empty() && !t.assignees.iter().any(|a| a == &opts.assignee) {
                return false;
            }
            if opts.roots && t.parent.is_some() {
                return false;
            }
            if let Some(p) = &opts.parent
                && t.parent.as_ref() != p.as_ref()
            {
                return false;
            }
            if opts.overdue && !timeutil::is_overdue(t.due_date.as_deref(), t.status.is_terminal())
            {
                return false;
            }
            true
        })
        .collect();

    filtered.sort_by(|a, b| compare_tasks(a, b));
    if opts.limit > 0 && filtered.len() > opts.limit {
        filtered.truncate(opts.limit);
    }
    Ok(filtered
        .into_iter()
        .map(|t| enrich(ctx, t, &status_map))
        .collect())
}

// ---------------------------------------------------------------------------
// Dependencies & cycles
// ---------------------------------------------------------------------------

pub fn would_block_cycle(ctx: &Ctx, task_id: &str, blocker_id: &str) -> bool {
    if !ids::is_safe_id(task_id) || !ids::is_safe_id(blocker_id) {
        return false;
    }
    let mut visited = HashSet::new();
    let mut stack = vec![blocker_id.to_owned()];
    while let Some(cur) = stack.pop() {
        if cur == task_id {
            return true;
        }
        if !visited.insert(cur.clone()) || !ids::is_safe_id(&cur) {
            continue;
        }
        if let Ok(t) = read_task_file(&ctx.task_path(&cur)) {
            stack.extend(t.blocked_by);
        }
    }
    false
}

pub fn would_parent_cycle(ctx: &Ctx, task_id: &str, parent_id: &str) -> bool {
    if !ids::is_safe_id(task_id) || !ids::is_safe_id(parent_id) {
        return false;
    }
    let mut visited = HashSet::new();
    let mut cur = parent_id.to_owned();
    loop {
        if cur == task_id || !visited.insert(cur.clone()) {
            return true;
        }
        if !ids::is_safe_id(&cur) {
            return false;
        }
        match read_task_file(&ctx.task_path(&cur)) {
            Ok(t) => match t.parent {
                Some(p) => cur = p,
                None => return false,
            },
            Err(_) => return false,
        }
    }
}

pub fn validate_parent(ctx: &Ctx, parent_id: &str, current: &str) -> Result<()> {
    if !ids::is_safe_id(parent_id) {
        return Err(StoreError::Msg(format!(
            "invalid parent ID format: {parent_id}"
        )));
    }
    if !current.is_empty() && parent_id == current {
        return Err(StoreError::Msg("task cannot be its own parent".into()));
    }
    let p = read_task_file(&ctx.task_path(parent_id)).map_err(|e| match e {
        StoreError::TaskNotFound(_) => {
            StoreError::Msg(format!("parent task not found: {parent_id}"))
        }
        e => e,
    })?;
    if p.id() != parent_id {
        return Err(StoreError::Msg(format!(
            "parent task ID mismatch: {parent_id}"
        )));
    }
    if !current.is_empty() && would_parent_cycle(ctx, current, parent_id) {
        return Err(StoreError::Msg(
            "would create circular parent relationship".into(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Integrity check
// ---------------------------------------------------------------------------

pub fn check_integrity(ctx: &Ctx) -> Result<Vec<String>> {
    let mut issues = Vec::new();
    let mut known = HashSet::new();
    let mut tasks = Vec::new();
    let entries = fs::read_dir(&ctx.tasks_dir).map_err(StoreError::Io)?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() || !name.ends_with(".json") || name == CONFIG_FILE {
            continue;
        }
        match read_task_file(&entry.path()) {
            Err(e) => issues.push(format!("Task file {name} is invalid: {e}")),
            Ok(t) => {
                let expected = name.trim_end_matches(".json");
                if ids::validate_project(&t.project).is_err() {
                    issues.push(format!("Task {} has invalid project name", t.id()));
                }
                if t.id() != expected {
                    issues.push(format!("Task file {name} contains task ID {}", t.id()));
                }
                known.insert(t.id());
                tasks.push(t);
            }
        }
    }
    for t in &tasks {
        for b in &t.blocked_by {
            if !ids::is_safe_id(b) {
                issues.push(format!("Task {} has unsafe blocker reference {b}", t.id()));
            } else if !known.contains(b) {
                issues.push(format!("Task {} is blocked by missing task {b}", t.id()));
            }
        }
        if let Some(p) = &t.parent
            && !known.contains(p)
        {
            issues.push(format!("Task {} has missing parent {p}", t.id()));
        }
    }

    // Cycles are valid JSON but not a valid graph.
    let mut blocked_edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut parent_edges: HashMap<String, Vec<String>> = HashMap::new();
    for t in &tasks {
        let id = t.id();
        blocked_edges.insert(
            id.clone(),
            t.blocked_by
                .iter()
                .filter(|b| known.contains(*b))
                .cloned()
                .collect(),
        );
        parent_edges.insert(
            id,
            t.parent
                .iter()
                .filter(|p| known.contains(*p))
                .cloned()
                .collect(),
        );
    }
    for id in cyclic_nodes(&blocked_edges) {
        issues.push(format!("Task {id} is part of a dependency cycle"));
    }
    for id in cyclic_nodes(&parent_edges) {
        issues.push(format!("Task {id} is part of a parent cycle"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_tracks_content_not_formatting() {
        let mut t: Task = serde_json::from_str(
            r#"{"project":"tk","ref":"a7b3","title":"t","status":"open",
                "priority":3,"created_at":"x","updated_at":"y"}"#,
        )
        .unwrap();
        let first = fingerprint(&t);
        assert_eq!(first, fingerprint(&t));
        t.title = "other".to_owned();
        assert_ne!(first, fingerprint(&t));
    }

    #[test]
    fn cyclic_nodes_finds_loops() {
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert("a".into(), vec!["b".into()]);
        edges.insert("b".into(), vec!["a".into()]);
        edges.insert("c".into(), vec!["d".into()]);
        edges.insert("d".into(), vec![]);
        assert_eq!(cyclic_nodes(&edges), vec!["a".to_owned(), "b".to_owned()]);
    }
}
