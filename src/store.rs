//! Filesystem store: root discovery, atomic writes, config, and all task
//! operations. Everything hangs off [`Ctx`] — no global working directory.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::ids::{self, IdError, TaskId};
use crate::model::{Config, Status, Task, TaskView};
use crate::timeutil;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("no .tasks/ directory found. Run 'tk init' to create one.\nSearched from: {0}")]
    TasksNotFound(String),
    #[error("task not found: {0}")]
    TaskNotFound(String),
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
    Msg(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

fn parse_err(what: impl Into<String>, err: impl ToString) -> StoreError {
    StoreError::Parse {
        what: what.into(),
        err: err.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

pub const TASKS_DIR: &str = ".tasks";
const CONFIG_FILE: &str = "config.json";

/// Resolved filesystem location. Build once per invocation and pass around.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub cwd: PathBuf,
    pub root: PathBuf,
    pub tasks_dir: PathBuf,
    pub exists: bool,
}

impl Ctx {
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
                    root: cur.clone(),
                    exists: true,
                };
            }
            if cur.join(".git").exists() {
                return Self {
                    cwd: start.to_path_buf(),
                    tasks_dir: cur.join(TASKS_DIR),
                    root: cur.clone(),
                    exists: false,
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

    pub fn require(&self) -> Result<()> {
        if self.exists {
            Ok(())
        } else {
            Err(StoreError::TasksNotFound(self.cwd.display().to_string()))
        }
    }

    pub fn task_path(&self, id: &str) -> PathBuf {
        self.tasks_dir.join(format!("{id}.json"))
    }

    pub fn config_path(&self) -> PathBuf {
        self.tasks_dir.join(CONFIG_FILE)
    }

    // -- config ------------------------------------------------------------

    pub fn load_config(&self) -> Result<Config> {
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

    pub fn save_config(&self, config: &Config) -> Result<()> {
        fs::create_dir_all(&self.tasks_dir).map_err(StoreError::Io)?;
        let data =
            serde_json::to_string_pretty(config).map_err(|e| parse_err("marshal config", e))?;
        atomic_write(&self.config_path(), data.as_bytes())
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) -> Result<Config> {
        let mut c = self.load_config()?;
        f(&mut c);
        self.save_config(&c)?;
        Ok(c)
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

pub fn enrich(ctx: &Ctx, task: &Task, status_map: &HashMap<String, Status>) -> TaskView {
    let mut blocked_by_incomplete = false;
    for b in &task.blocked_by {
        if !ids::is_safe_id(b) {
            continue;
        }
        let status = match status_map.get(b) {
            Some(s) => Some(*s),
            None => read_task_file(&ctx.task_path(b)).ok().map(|t| t.status),
        };
        if status.is_some_and(|s| !s.is_terminal()) {
            blocked_by_incomplete = true;
            break;
        }
    }
    let done = task.status.is_terminal();
    TaskView {
        id: task.id(),
        blocked_by_incomplete,
        is_overdue: timeutil::is_overdue(task.due_date.as_deref(), done),
        days_until_due: timeutil::days_until_due(task.due_date.as_deref(), done),
        task: task.clone(),
    }
}

// ---------------------------------------------------------------------------
// Create / get / update / delete
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct CreateOptions {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<crate::model::Priority>,
    pub project: Option<String>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub parent: Option<String>,
    pub estimate: Option<i64>,
    pub due_date: Option<String>,
}

pub fn create_task(ctx: &Ctx, opts: CreateOptions) -> Result<TaskView> {
    if opts.title.trim().is_empty() {
        return Err(StoreError::EmptyTitle);
    }
    // Ensure the store exists (creates `.tasks/` + default config on first add,
    // mirroring the Go behavior of bootstrapping from `tk add`).
    let mut config = ctx.load_config().unwrap_or_default();
    if !ctx.exists {
        fs::create_dir_all(&ctx.tasks_dir).map_err(StoreError::Io)?;
    }
    let project = opts.project.unwrap_or_else(|| config.project.clone());
    ids::validate_project(&project).map_err(|e| match e {
        IdError::BadProject(p) => StoreError::Msg(format!(
            "invalid project name {p:?}: use lowercase letters, digits, and internal hyphens"
        )),
        e => StoreError::Msg(e.to_string()),
    })?;
    if !ctx.config_path().exists() {
        config.project.clone_from(&project);
        ctx.save_config(&config)?;
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
        let path = ctx.task_path(&task.id());
        if path.exists() {
            continue; // ref collision — retry
        }
        // Exclusive create.
        let data = serde_json::to_string_pretty(&task).map_err(|e| parse_err("marshal task", e))?;
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
        return Ok(enrich(ctx, &task, &HashMap::new()));
    }
    Err(StoreError::RefCollisions(10))
}

/// Self-healing read: drops dangling blockers/parents and repairs filename
/// ↔ content ID mismatches, persisting the fix.
#[derive(Debug, Default)]
pub struct CleanupReport {
    pub orphaned_blockers: Vec<String>,
    pub orphaned_parent: Option<String>,
    pub id_mismatch: Option<(String, String)>,
}

impl CleanupReport {
    pub fn is_empty(&self) -> bool {
        self.orphaned_blockers.is_empty()
            && self.orphaned_parent.is_none()
            && self.id_mismatch.is_none()
    }

    pub fn message(&self, id: &str) -> String {
        let mut parts = Vec::new();
        if !self.orphaned_blockers.is_empty() {
            let noun = if self.orphaned_blockers.len() == 1 {
                "reference"
            } else {
                "references"
            };
            parts.push(format!("{} orphaned {noun}", self.orphaned_blockers.len()));
        }
        if self.orphaned_parent.is_some() {
            parts.push("orphaned parent".to_owned());
        }
        if let Some((was, _)) = &self.id_mismatch {
            parts.push(format!("ID mismatch (was {was})"));
        }
        format!("(cleaned {} from {id})", parts.join(", "))
    }
}

/// Plain read without self-healing (for callers that only inspect fields).
pub fn read_task(ctx: &Ctx, id: &str) -> Result<Task> {
    read_task_file(&ctx.task_path(id))
}

pub fn get_task(ctx: &Ctx, id: &str) -> Result<(TaskView, Option<CleanupReport>)> {
    ctx.require()?;
    let mut task = read_task_file(&ctx.task_path(id))?;
    let report = heal(ctx, &mut task, id)?;
    let view = enrich(ctx, &task, &HashMap::new());
    Ok((view, report))
}

fn heal(ctx: &Ctx, task: &mut Task, expected_id: &str) -> Result<Option<CleanupReport>> {
    let mut report = CleanupReport::default();
    let mut modified = false;

    if task.id() != expected_id
        && let Some(parsed) = TaskId::parse(expected_id)
    {
        report.id_mismatch = Some((task.id(), expected_id.to_owned()));
        task.project = parsed.project;
        task.r#ref = parsed.r#ref;
        modified = true;
    }

    let before = task.blocked_by.len();
    task.blocked_by.retain(|b| {
        if !ids::is_safe_id(b) || !ctx.task_path(b).exists() {
            report.orphaned_blockers.push(b.clone());
            false
        } else {
            true
        }
    });
    modified |= task.blocked_by.len() != before;

    if let Some(p) = task.parent.clone()
        && (!ids::is_safe_id(&p) || !ctx.task_path(&p).exists())
    {
        report.orphaned_parent = Some(p);
        task.parent = None;
        modified = true;
    }

    if modified {
        task.updated_at = timeutil::now_rfc3339_nano();
        write_task(ctx, task)?;
    }
    Ok((!report.is_empty()).then_some(report))
}

#[derive(Debug, Default)]
pub struct UpdateOptions {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<crate::model::Priority>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub parent: Option<Option<String>>,
    pub estimate: Option<Option<i64>>,
    pub due_date: Option<Option<String>>,
}

pub fn update_status(ctx: &Ctx, id: &str, status: Status) -> Result<TaskView> {
    let mut task = read_task_file(&ctx.task_path(id))?;
    if task.status != status {
        let now = timeutil::now_rfc3339_nano();
        task.status = status;
        task.updated_at = now.clone();
        task.completed_at = if status == Status::Done {
            Some(now)
        } else {
            None
        };
        write_task(ctx, &task)?;
    }
    Ok(enrich(ctx, &task, &HashMap::new()))
}

pub fn update_task(ctx: &Ctx, id: &str, u: UpdateOptions) -> Result<TaskView> {
    let mut task = read_task_file(&ctx.task_path(id))?;
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
        write_task(ctx, &task)?;
    }
    Ok(enrich(ctx, &task, &HashMap::new()))
}

pub fn delete_task(ctx: &Ctx, id: &str) -> Result<()> {
    if !ids::is_safe_id(id) {
        return Err(StoreError::UnsafeId(id.to_owned()));
    }
    fs::remove_file(ctx.task_path(id)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StoreError::TaskNotFound(id.to_owned())
        } else {
            StoreError::Io(e)
        }
    })?;
    // Scrub references in survivors.
    for mut t in all_tasks(ctx)? {
        let mut modified = false;
        let before = t.blocked_by.len();
        t.blocked_by.retain(|b| b != id);
        modified |= t.blocked_by.len() != before;
        if t.parent.as_deref() == Some(id) {
            t.parent = None;
            modified = true;
        }
        if modified {
            write_task(ctx, &t)?;
        }
    }
    Ok(())
}

pub fn add_blocker(ctx: &Ctx, id: &str, blocker: &str) -> Result<Task> {
    let mut t = read_task_file(&ctx.task_path(id))?;
    t.blocked_by.push(blocker.to_owned());
    t.updated_at = timeutil::now_rfc3339_nano();
    write_task(ctx, &t)?;
    Ok(t)
}

pub fn remove_blocker(ctx: &Ctx, id: &str, blocker: &str) -> Result<(Task, bool)> {
    let mut t = read_task_file(&ctx.task_path(id))?;
    let before = t.blocked_by.len();
    t.blocked_by.retain(|b| b != blocker);
    let found = t.blocked_by.len() != before;
    if found {
        t.updated_at = timeutil::now_rfc3339_nano();
        write_task(ctx, &t)?;
    }
    Ok((t, found))
}

pub fn add_log(ctx: &Ctx, id: &str, msg: &str) -> Result<Task> {
    use crate::model::LogEntry;
    let mut t = read_task_file(&ctx.task_path(id))?;
    let now = timeutil::now_rfc3339_nano();
    t.logs.push(LogEntry {
        ts: now.clone(),
        msg: msg.to_owned(),
    });
    t.updated_at = now;
    write_task(ctx, &t)?;
    Ok(t)
}

// ---------------------------------------------------------------------------
// List / filter / sort
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ListOptions {
    pub search: String,
    pub hide_terminal: bool,
    pub status: Option<Status>,
    pub priority: Option<crate::model::Priority>,
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
    let mut visited = std::collections::HashSet::new();
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
    let mut visited = std::collections::HashSet::new();
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
// Moves & renames
// ---------------------------------------------------------------------------

pub struct RenameResult {
    pub renamed: Vec<String>,
    pub references_updated: usize,
}

pub fn rename_project(ctx: &Ctx, old: &str, new: &str) -> Result<RenameResult> {
    ids::validate_project(new).map_err(|e| StoreError::Msg(e.to_string()))?;
    if old == new {
        return Err(StoreError::Msg(format!(
            "project {old:?} is already named that"
        )));
    }
    ctx.require()?;
    let config = ctx.load_config()?;
    let tasks = all_tasks(ctx)?;

    let to_rename: Vec<&Task> = tasks.iter().filter(|t| t.project == old).collect();
    if to_rename.is_empty() {
        return Err(StoreError::Msg(format!(
            "no tasks found with project {old:?}"
        )));
    }
    let existing: std::collections::HashSet<String> = tasks.iter().map(|t| t.id()).collect();
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
            let old_path = ctx.task_path(&t.id());
            t.project = new.to_owned();
            let new_path = ctx.task_path(&t.id());
            res.renamed.push(t.id());
            // Move first so a later write failure can't leave both IDs behind.
            fs::rename(&old_path, &new_path).map_err(StoreError::Io)?;
            write_task(ctx, &t)
                .map_err(|e| StoreError::Msg(format!("update moved task {}: {e}", t.id())))?;
        } else if modified {
            write_task(ctx, &t)?;
        }
    }
    if config.project == old {
        ctx.update_config(|c| c.project = new.to_owned())?;
    }
    Ok(res)
}

pub struct MoveResult {
    pub old_id: String,
    pub new_id: String,
    pub references_updated: usize,
}

pub fn move_task(ctx: &Ctx, id: &str, new_project: &str) -> Result<MoveResult> {
    ids::validate_project(new_project).map_err(|e| StoreError::Msg(e.to_string()))?;
    if !ids::is_safe_id(id) {
        return Err(StoreError::UnsafeId(id.to_owned()));
    }
    ctx.require()?;
    let mut task = read_task_file(&ctx.task_path(id))?;
    if task.project == new_project {
        return Err(StoreError::Msg(format!(
            "task {id} is already in project {new_project:?}"
        )));
    }
    let new_id = format!("{new_project}-{}", task.r#ref);
    if !ids::is_safe_id(&new_id) {
        return Err(StoreError::UnsafeId(new_id));
    }
    if ctx.task_path(&new_id).exists() {
        return Err(StoreError::Msg(format!(
            "cannot move: {new_id:?} already exists"
        )));
    }
    task.project = new_project.to_owned();
    task.updated_at = timeutil::now_rfc3339_nano();
    fs::rename(ctx.task_path(id), ctx.task_path(&new_id)).map_err(StoreError::Io)?;
    write_task(ctx, &task)
        .map_err(|e| StoreError::Msg(format!("update moved task {}: {e}", task.id())))?;

    let mut res = MoveResult {
        old_id: id.to_owned(),
        new_id: new_id.clone(),
        references_updated: 0,
    };
    for mut t in all_tasks(ctx)? {
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
            write_task(ctx, &t)?;
        }
    }
    Ok(res)
}

// ---------------------------------------------------------------------------
// Clean & check
// ---------------------------------------------------------------------------

pub fn clean_tasks(ctx: &Ctx, days: i64) -> Result<usize> {
    if days < 0 {
        return Err(StoreError::Msg(
            "clean threshold must be non-negative".into(),
        ));
    }
    let tasks = all_tasks(ctx)?;
    let now = chrono::Utc::now();
    let threshold = chrono::Duration::days(days);

    let mut doomed = std::collections::HashSet::new();
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
        let p = ctx.task_path(id);
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
            write_task(ctx, &t)?;
        }
    }
    Ok(doomed.len())
}

pub fn check_integrity(ctx: &Ctx) -> Result<Vec<String>> {
    let mut issues = Vec::new();
    let mut known = std::collections::HashSet::new();
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
            if !known.contains(b) {
                issues.push(format!("Task {} is blocked by missing task {b}", t.id()));
            }
        }
        if let Some(p) = &t.parent
            && !known.contains(p)
        {
            issues.push(format!("Task {} has missing parent {p}", t.id()));
        }
    }
    Ok(issues)
}
