//! `tk apply` — a batch of intents under one lock.
//!
//! The gap this closes: an agent blocking a task, checkpointing it, completing
//! it, and logging why is four invocations, four lock acquisitions, four
//! revision checks, and four places to stop halfway. A batch states the whole
//! change once.
//!
//! What it guarantees, stated precisely: every intent is resolved and validated
//! before anything is written, so a batch that is wrong is rejected whole. What
//! it does not guarantee is a cross-record transaction — records are separate
//! append-only files, and a batch that fails on I/O partway through has already
//! applied what came before it. The response says exactly which intents landed,
//! and `dry_run` reports the plan without writing anything.

use miette::IntoDiagnostic;
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Priority, Status, TaskView};
use crate::record::op;
use crate::store::{CreateOptions, StoreError, Txn};
use crate::timeutil;

/// The request body: `{"intents": [...]}`.
#[derive(Debug, Deserialize)]
pub struct ApplyRequest {
    pub intents: Vec<Intent>,
}

/// One intent. `op` selects the variant and the remaining keys are its fields.
///
/// Unknown fields are rejected: a misspelled key silently doing nothing is
/// worse than a failed batch.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Intent {
    Add(AddIntent),
    Checkpoint(TextIntent),
    Status(StatusIntent),
    Log(LogIntent),
    Edit(EditIntent),
    Block(EdgeIntent),
    Unblock(EdgeIntent),
    Relate(EdgeIntent),
    Unrelate(EdgeIntent),
    Link(ListIntent),
    Unlink(ListIntent),
    Accept(ListIntent),
    Evidence(ListIntent),
    Archive(IdIntent),
    Unarchive(IdIntent),
    Mv(MvIntent),
    Purge(PurgeIntent),
}

impl Intent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Checkpoint(_) => "checkpoint",
            Self::Status(_) => "status",
            Self::Log(_) => "log",
            Self::Edit(_) => "edit",
            Self::Block(_) => "block",
            Self::Unblock(_) => "unblock",
            Self::Relate(_) => "relate",
            Self::Unrelate(_) => "unrelate",
            Self::Link(_) => "link",
            Self::Unlink(_) => "unlink",
            Self::Accept(_) => "accept",
            Self::Evidence(_) => "evidence",
            Self::Archive(_) => "archive",
            Self::Unarchive(_) => "unarchive",
            Self::Mv(_) => "mv",
            Self::Purge(_) => "purge",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddIntent {
    pub title: String,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default, deserialize_with = "de_priority")]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub estimate: Option<i64>,
    /// `YYYY-MM-DD` or relative (`+7d`).
    #[serde(default)]
    pub due: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextIntent {
    pub id: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub if_rev: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusIntent {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogIntent {
    pub id: String,
    pub msg: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditIntent {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default, deserialize_with = "de_priority")]
    pub priority: Option<Priority>,
    /// `YYYY-MM-DD`, relative, or `-` to clear.
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub estimate: Option<i64>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub remove_labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default)]
    pub remove_assignees: Vec<String>,
    #[serde(default)]
    pub if_rev: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeIntent {
    pub id: String,
    #[serde(alias = "blocker", alias = "other")]
    pub target: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListIntent {
    pub id: String,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default)]
    pub clear: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdIntent {
    pub id: String,
    #[serde(default)]
    pub if_rev: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MvIntent {
    pub id: String,
    pub project: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurgeIntent {
    pub id: String,
    #[serde(default)]
    pub scrub: bool,
    #[serde(default)]
    pub if_rev: Option<String>,
}

/// Accept `1`, `"1"`, `"p1"`, or `"high"` for a priority.
fn de_priority<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Priority>, D::Error> {
    use serde::de::Error as _;
    let raw = Option::<Value>::deserialize(d)?;
    match raw {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .and_then(Priority::from_u8)
            .map(Some)
            .ok_or_else(|| D::Error::custom("priority must be 0-4")),
        Some(Value::String(s)) => Priority::parse(&s).map(Some).map_err(D::Error::custom),
        Some(_) => Err(D::Error::custom(
            "priority must be a number 0-4 or one of none/urgent/high/medium/low",
        )),
    }
}

/// What one intent resolved to.
#[derive(Debug, serde::Serialize)]
pub struct IntentResult {
    pub index: usize,
    pub op: &'static str,
    /// The task the intent addressed (its alias, when it resolved to one).
    pub id: Option<String>,
    pub rev: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ApplyOutcome {
    pub dry_run: bool,
    pub applied: Vec<IntentResult>,
}

/// Apply a validated batch.
///
/// Validation happens first and touches nothing, so a semantically invalid
/// batch is rejected whole. Writes then run in order; an I/O failure partway
/// through reports how many intents had landed.
pub fn apply(
    txn: &Txn<'_>,
    request: &ApplyRequest,
    dry_run: bool,
) -> Result<ApplyOutcome, StoreError> {
    let plan = plan(txn, request)?;
    let mut applied = Vec::with_capacity(plan.len());
    for step in &plan {
        if !dry_run {
            execute(txn, step.intent).map_err(|e| {
                StoreError::Msg(format!(
                    "intent {} ({}) failed after {} applied: {e}",
                    step.index,
                    step.intent.name(),
                    applied.len()
                ))
            })?;
        }
        applied.push(IntentResult {
            index: step.index,
            op: step.intent.name(),
            id: step.target.clone(),
            rev: step.rev.clone(),
        });
    }
    Ok(ApplyOutcome { dry_run, applied })
}

/// A validated intent: everything resolved, nothing written.
struct Step<'a> {
    index: usize,
    intent: &'a Intent,
    target: Option<String>,
    rev: Option<String>,
}

fn plan<'a>(txn: &Txn<'_>, request: &'a ApplyRequest) -> Result<Vec<Step<'a>>, StoreError> {
    let mut steps = Vec::with_capacity(request.intents.len());
    let invalid = |index: usize, op: &str, why: String| {
        StoreError::InvalidInput(format!("intent {index} ({op}) is invalid: {why}"))
    };
    // Validate against the graph this batch is building, not only what is on
    // disk: two `block` intents can close a loop between them.
    let mut graph = blocked_graph(txn)?;
    for (index, intent) in request.intents.iter().enumerate() {
        let op = intent.name();
        let target = match intent {
            Intent::Add(add) => {
                if add.title.trim().is_empty() {
                    return Err(invalid(index, op, "title cannot be empty".into()));
                }
                if let Some(project) = &add.project {
                    crate::ids::validate_project(project)
                        .map_err(|e| invalid(index, op, e.to_string()))?;
                }
                if let Some(due) = &add.due {
                    timeutil::parse_due_date(due).map_err(|e| invalid(index, op, e.to_string()))?;
                }
                if let Some(parent) = &add.parent {
                    txn.resolve(parent)
                        .map_err(|e| invalid(index, op, e.to_string()))?;
                }
                None
            }
            Intent::Checkpoint(t) => Some(resolve_id(txn, index, op, &t.id)?),
            Intent::Status(s) => {
                Status::parse(&s.status).map_err(|e| invalid(index, op, e.to_string()))?;
                Some(resolve_id(txn, index, op, &s.id)?)
            }
            Intent::Log(l) => {
                if l.msg.trim().is_empty() {
                    return Err(invalid(index, op, "message cannot be empty".into()));
                }
                Some(resolve_id(txn, index, op, &l.id)?)
            }
            Intent::Edit(e) => {
                if let Some(priority) = e.priority
                    && Priority::from_u8(priority as u8).is_none()
                {
                    return Err(invalid(index, op, "unknown priority".into()));
                }
                if let Some(due) = &e.due
                    && due != "-"
                {
                    timeutil::parse_due_date(due).map_err(|e| invalid(index, op, e.to_string()))?;
                }
                if let Some(parent) = &e.parent
                    && parent != "-"
                {
                    txn.resolve(parent)
                        .map_err(|e| invalid(index, op, e.to_string()))?;
                }
                Some(resolve_id(txn, index, op, &e.id)?)
            }
            Intent::Block(e) => {
                let id = resolve_id(txn, index, op, &e.id)?;
                let target = txn
                    .resolve(&e.target)
                    .map_err(|err| invalid(index, op, err.to_string()))?;
                // Validate the edge here, so a cycle is a rejected batch rather
                // than a failure halfway through writing.
                if id == target {
                    return Err(invalid(index, op, "a task cannot block itself".into()));
                }
                if !blocked_by(&graph, &id).contains(&target) && reaches(&graph, &target, &id) {
                    return Err(invalid(
                        index,
                        op,
                        format!("would create a circular dependency with {}", short(&target)),
                    ));
                }
                graph.entry(id.clone()).or_default().push(target);
                steps.push(Step {
                    index,
                    intent,
                    target: Some(id),
                    rev: None,
                });
                continue;
            }
            Intent::Unblock(e) => {
                let id = resolve_id(txn, index, op, &e.id)?;
                let target = txn
                    .resolve(&e.target)
                    .map_err(|err| invalid(index, op, err.to_string()))?;
                if let Some(edges) = graph.get_mut(&id) {
                    edges.retain(|b| b != &target);
                }
                Some(id)
            }
            Intent::Relate(e) | Intent::Unrelate(e) => {
                let id = resolve_id(txn, index, op, &e.id)?;
                txn.resolve(&e.target)
                    .map_err(|err| invalid(index, op, err.to_string()))?;
                Some(id)
            }
            Intent::Link(l) | Intent::Unlink(l) | Intent::Accept(l) | Intent::Evidence(l) => {
                if !l.values.is_empty() && (!l.remove.is_empty() || l.clear) {
                    return Err(invalid(index, op, "add or remove/clear, not both".into()));
                }
                Some(resolve_id(txn, index, op, &l.id)?)
            }
            Intent::Archive(a) => {
                let id = resolve_id(txn, index, op, &a.id)?;
                let record = txn.load(&id)?;
                if !record.state.status.is_terminal() {
                    return Err(invalid(
                        index,
                        op,
                        format!("{} is {}", record.state.alias, record.state.status),
                    ));
                }
                Some(id)
            }
            Intent::Unarchive(u) => Some(resolve_id(txn, index, op, &u.id)?),
            Intent::Mv(m) => {
                crate::ids::validate_project(&m.project)
                    .map_err(|e| invalid(index, op, e.to_string()))?;
                Some(resolve_id(txn, index, op, &m.id)?)
            }
            Intent::Purge(p) => Some(resolve_id(txn, index, op, &p.id)?),
        };
        let rev = match (&target, intent) {
            (Some(id), _) => Some(txn.load(id)?.rev()),
            _ => None,
        };
        steps.push(Step {
            index,
            intent,
            target,
            rev,
        });
    }
    Ok(steps)
}

fn resolve_id(txn: &Txn<'_>, index: usize, op: &str, input: &str) -> Result<String, StoreError> {
    txn.resolve(input)
        .map_err(|e| StoreError::InvalidInput(format!("intent {index} ({op}) is invalid: {e}")))
}

/// `id -> blockers` for every record: the base for batch validation.
fn blocked_graph(
    txn: &Txn<'_>,
) -> Result<std::collections::HashMap<String, Vec<String>>, StoreError> {
    Ok(txn
        .snapshot()?
        .records
        .iter()
        .map(|r| (r.id.clone(), r.state.blocked_by.clone()))
        .collect())
}

fn blocked_by<'a>(
    graph: &'a std::collections::HashMap<String, Vec<String>>,
    id: &str,
) -> &'a [String] {
    graph.get(id).map(Vec::as_slice).unwrap_or(&[])
}

/// Can `goal` be reached from `start` by following blocker edges?
fn reaches(
    graph: &std::collections::HashMap<String, Vec<String>>,
    start: &str,
    goal: &str,
) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![start.to_owned()];
    while let Some(node) = stack.pop() {
        if node == goal {
            return true;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        stack.extend(blocked_by(graph, &node).iter().cloned());
    }
    false
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn execute(txn: &Txn<'_>, intent: &Intent) -> Result<Option<TaskView>, StoreError> {
    let view = match intent {
        Intent::Add(add) => {
            let priority = add.priority;
            let due_date = add
                .due
                .as_ref()
                .map(|d| timeutil::parse_due_date(d))
                .transpose()
                .map_err(|e| StoreError::Msg(e.to_string()))?
                .flatten();
            let parent = add.parent.as_ref().map(|p| txn.resolve(p)).transpose()?;
            Some(txn.create(CreateOptions {
                title: add.title.clone(),
                description: add.desc.clone(),
                priority,
                project: add.project.clone(),
                labels: (!add.labels.is_empty()).then(|| add.labels.clone()),
                assignees: (!add.assignees.is_empty()).then(|| add.assignees.clone()),
                parent,
                estimate: add.estimate,
                due_date,
            })?)
        }
        Intent::Checkpoint(t) => {
            let id = txn.resolve(&t.id)?;
            txn.check_rev(&id, t.if_rev.as_deref())?;
            let value = t.text.clone().filter(|_| !t.clear);
            txn.append_if_rev(&id, op::CHECKPOINT, serde_json::json!(value), None)?
                .into()
        }
        Intent::Status(s) => {
            let id = txn.resolve(&s.id)?;
            let status = Status::parse(&s.status).map_err(|e| StoreError::Msg(e.to_string()))?;
            txn.append_if_rev(&id, op::STATUS, serde_json::json!(status), None)?
                .into()
        }
        Intent::Log(l) => {
            let id = txn.resolve(&l.id)?;
            let entry = crate::model::LogEntry {
                ts: String::new(),
                msg: l.msg.clone(),
            };
            let data = serde_json::to_value(&entry).map_err(parse)?;
            txn.append_if_rev(&id, op::LOG, data, None)?.into()
        }
        Intent::Edit(e) => {
            let id = txn.resolve(&e.id)?;
            txn.check_rev(&id, e.if_rev.as_deref())?;
            if let Some(title) = &e.title {
                txn.append(&id, op::TITLE, serde_json::json!(title))?;
            }
            if let Some(desc) = &e.desc {
                let value = (desc != "-").then(|| desc.clone());
                txn.append(&id, op::DESCRIPTION, serde_json::json!(value))?;
            }
            if let Some(priority) = e.priority {
                txn.append(&id, op::PRIORITY, serde_json::json!(priority as u8))?;
            }
            if let Some(due) = &e.due {
                let value = if due == "-" {
                    None
                } else {
                    timeutil::parse_due_date(due).map_err(|err| StoreError::Msg(err.to_string()))?
                };
                txn.append(&id, op::DUE_DATE, serde_json::json!(value))?;
            }
            if let Some(estimate) = e.estimate {
                let value = (estimate != 0).then_some(estimate);
                txn.append(&id, op::ESTIMATE, serde_json::json!(value))?;
            }
            if let Some(parent) = &e.parent {
                if parent == "-" {
                    txn.set_parent(&id, None)?;
                } else {
                    let pid = txn.resolve(parent)?;
                    txn.set_parent(&id, Some(&pid))?;
                }
            }
            let mut labels = e.labels.clone();
            labels.extend(e.remove_labels.iter().map(|l| format!("-{l}")));
            apply_list(txn, &id, ListKind::Labels, &labels)?;
            let mut assignees = e.assignees.clone();
            assignees.extend(e.remove_assignees.iter().map(|a| format!("-{a}")));
            apply_list(txn, &id, ListKind::Assignees, &assignees)?;
            Some(txn.view_of(&id)?)
        }
        Intent::Block(e) => {
            let id = txn.resolve(&e.id)?;
            let target = txn.resolve(&e.target)?;
            Some(txn.add_blocker(&id, &target)?)
        }
        Intent::Unblock(e) => {
            let id = txn.resolve(&e.id)?;
            let target = txn.resolve(&e.target)?;
            Some(txn.remove_blocker(&id, &target)?.0)
        }
        Intent::Relate(e) => {
            let id = txn.resolve(&e.id)?;
            let target = txn.resolve(&e.target)?;
            Some(txn.add_related(&id, &target)?)
        }
        Intent::Unrelate(e) => {
            let id = txn.resolve(&e.id)?;
            let target = txn.resolve(&e.target)?;
            Some(txn.remove_related(&id, &target)?.0)
        }
        Intent::Link(l) => apply_list_keyed(txn, l, ListKind::Links)?,
        Intent::Unlink(l) => apply_list_keyed(txn, l, ListKind::Links)?,
        Intent::Accept(l) => apply_list_keyed(txn, l, ListKind::Acceptance)?,
        Intent::Evidence(l) => apply_list_keyed(txn, l, ListKind::Evidence)?,
        Intent::Archive(a) => {
            let id = txn.resolve(&a.id)?;
            let record = txn.load(&id)?;
            if !record.state.is_archived() {
                txn.append_if_rev(&id, op::ARCHIVED, Value::Null, None)?;
            }
            Some(txn.view_of(&id)?)
        }
        Intent::Unarchive(u) => {
            let id = txn.resolve(&u.id)?;
            txn.append_if_rev(&id, op::UNARCHIVED, Value::Null, None)?;
            Some(txn.view_of(&id)?)
        }
        Intent::Mv(m) => {
            let id = txn.resolve(&m.id)?;
            txn.append(&id, op::PROJECT, serde_json::json!(m.project))?;
            Some(txn.view_of(&id)?)
        }
        Intent::Purge(p) => {
            let id = txn.resolve(&p.id)?;
            txn.check_rev(&id, p.if_rev.as_deref())?;
            txn.purge(&id, p.scrub)?;
            None
        }
    };
    Ok(view)
}

fn apply_list_keyed(
    txn: &Txn<'_>,
    intent: &ListIntent,
    kind: ListKind,
) -> Result<Option<TaskView>, StoreError> {
    let id = txn.resolve(&intent.id)?;
    if intent.clear {
        txn.append(&id, kind.set_op(), serde_json::json!([]))?;
    } else if !intent.remove.is_empty() {
        txn.append(&id, kind.remove_op(), serde_json::json!(intent.remove))?;
    } else if !intent.values.is_empty() {
        txn.append(&id, kind.add_op(), serde_json::json!(intent.values))?;
    }
    Ok(Some(txn.view_of(&id)?))
}

/// Apply `+`/`-` deltas, and bare values as a replacement computed from the
/// current state (the batch holds the lock, so the read is consistent).
fn apply_list(txn: &Txn<'_>, id: &str, kind: ListKind, ops: &[String]) -> Result<(), StoreError> {
    if ops.is_empty() {
        return Ok(());
    }
    let replaces = ops.iter().any(|v| !v.starts_with(['+', '-']));
    if !replaces {
        let add: Vec<&String> = ops
            .iter()
            .filter_map(|v| v.strip_prefix('+').map(|_| v))
            .collect();
        let mut add_values = Vec::new();
        for v in add {
            add_values.push(v.trim_start_matches('+').to_owned());
        }
        let remove_values: Vec<String> = ops
            .iter()
            .filter_map(|v| v.strip_prefix('-').map(|s| s.to_owned()))
            .collect();
        if !add_values.is_empty() {
            txn.append(id, kind.add_op(), serde_json::json!(add_values))?;
        }
        if !remove_values.is_empty() {
            txn.append(id, kind.remove_op(), serde_json::json!(remove_values))?;
        }
        return Ok(());
    }
    let record = txn.load(id)?;
    let mut set: std::collections::BTreeSet<String> =
        kind.current(&record).iter().cloned().collect();
    let mut replaced = false;
    for value in ops {
        if let Some(add) = value.strip_prefix('+') {
            set.insert(add.to_owned());
        } else if let Some(remove) = value.strip_prefix('-') {
            set.remove(remove);
        } else {
            if !replaced {
                set.clear();
                replaced = true;
            }
            set.insert(value.clone());
        }
    }
    let merged: Vec<String> = set.into_iter().collect();
    if merged != kind.current(&record) {
        txn.append(id, kind.set_op(), serde_json::json!(merged))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ListKind {
    Labels,
    Assignees,
    Links,
    Acceptance,
    Evidence,
}

impl ListKind {
    fn current(self, record: &crate::record::Record) -> &[String] {
        match self {
            Self::Labels => &record.state.labels,
            Self::Assignees => &record.state.assignees,
            Self::Links => &record.state.links,
            Self::Acceptance => &record.state.acceptance,
            Self::Evidence => &record.state.evidence,
        }
    }
    fn add_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_ADD,
            Self::Assignees => op::ASSIGNEES_ADD,
            Self::Links => op::LINKS_ADD,
            Self::Acceptance => op::ACCEPTANCE_ADD,
            Self::Evidence => op::EVIDENCE_ADD,
        }
    }
    fn remove_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_REMOVE,
            Self::Assignees => op::ASSIGNEES_REMOVE,
            Self::Links => op::LINKS_REMOVE,
            Self::Acceptance => op::ACCEPTANCE_REMOVE,
            Self::Evidence => op::EVIDENCE_REMOVE,
        }
    }
    fn set_op(self) -> &'static str {
        match self {
            Self::Labels => op::LABELS_SET,
            Self::Assignees => op::ASSIGNEES_SET,
            Self::Links => op::LINKS_SET,
            Self::Acceptance => op::ACCEPTANCE_SET,
            Self::Evidence => op::EVIDENCE_SET,
        }
    }
}

fn parse(err: serde_json::Error) -> StoreError {
    StoreError::Parse {
        what: "intent".into(),
        err: err.to_string(),
    }
}

/// Parse a request body.
pub fn parse_request(input: &str) -> Result<ApplyRequest, StoreError> {
    serde_json::from_str(input).map_err(parse)
}

/// The batch, as a command runs it.
pub fn run(ctx: &crate::cli::AppCtx, dry_run: bool) -> miette::Result<()> {
    use std::io::Read as _;
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .into_diagnostic()?;
    if input.trim().is_empty() {
        return Err(miette::miette!(
            "no intents on stdin; pipe a JSON body like {{\"intents\": [{{\"op\": \"log\", \"id\": \"a7b3\", \"msg\": \"...\"}}]}}"
        ));
    }
    let request = parse_request(&input).into_diagnostic()?;
    ctx.require_store()?;
    let txn = ctx.store.txn()?;
    let outcome = apply(&txn, &request, dry_run)?;
    let human = if dry_run {
        format!(
            "Validated {} intent(s); nothing written.",
            outcome.applied.len()
        )
    } else {
        format!("Applied {} intent(s).", outcome.applied.len())
    };
    ctx.emit("apply", &outcome, None, Vec::new(), || human);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_parse_and_reject_typos() {
        let request = parse_request(
            r#"{"intents":[
                {"op":"add","title":"t","priority":"p1","labels":["a"]},
                {"op":"status","id":"a7b3","status":"done"},
                {"op":"block","id":"a7b3","blocker":"b7c4"},
                {"op":"log","id":"a7b3","msg":"hi"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(request.intents.len(), 4);
        assert_eq!(request.intents[0].name(), "add");
        match &request.intents[2] {
            Intent::Block(edge) => assert_eq!(edge.target, "b7c4"),
            other => panic!("expected block, got {}", other.name()),
        }

        // A misspelled field must fail rather than silently do nothing.
        let err =
            parse_request(r#"{"intents":[{"op":"log","id":"a7b3","message":"hi"}]}"#).unwrap_err();
        assert!(err.to_string().contains("message"), "{err}");

        // An unknown op is a bad request, not a silent no-op.
        assert!(parse_request(r#"{"intents":[{"op":"teleport","id":"x"}]}"#).is_err());
    }

    #[test]
    fn priority_accepts_every_spelling() {
        let one: AddIntent = serde_json::from_str(r#"{"title":"t","priority":1}"#).unwrap();
        assert_eq!(one.priority, Some(Priority::Urgent));
        let two: AddIntent = serde_json::from_str(r#"{"title":"t","priority":"p2"}"#).unwrap();
        assert_eq!(two.priority, Some(Priority::High));
        let three: AddIntent = serde_json::from_str(r#"{"title":"t","priority":"low"}"#).unwrap();
        assert_eq!(three.priority, Some(Priority::Low));
        assert!(serde_json::from_str::<AddIntent>(r#"{"title":"t","priority":9}"#).is_err());
    }
}
