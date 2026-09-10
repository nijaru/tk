//! `tk apply` — a batch of intents under one lock.
//!
//! The gap this closes: an agent blocking a task, checkpointing it, completing
//! it, and logging why is four invocations, four lock acquisitions, four
//! revision checks, and four places to stop halfway. A batch states the whole
//! change once.
//!
//! Every intent dispatches to [`crate::ops`], the same functions the CLI calls,
//! so a batch cannot drift from what `tk log` or `tk archive` do.
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
use crate::ops::{self, Edit, ListEdit, ListField, Mutation};
use crate::store::{CreateOptions, StoreError};
use crate::{cli::AppCtx, ids};

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
    pub parent: Option<String>,
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
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub remove_labels: Vec<String>,
    #[serde(default)]
    pub if_rev: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeIntent {
    pub id: String,
    #[serde(alias = "blocker")]
    pub blocker: String,
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
    /// The task the intent addressed.
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
    m: &Mutation<'_>,
    request: &ApplyRequest,
    dry_run: bool,
) -> Result<ApplyOutcome, StoreError> {
    let plan = plan(m, request)?;
    let mut applied = Vec::with_capacity(plan.len());
    for step in &plan {
        if !dry_run {
            execute(m, step.intent).map_err(|e| {
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

fn plan<'a>(m: &Mutation<'_>, request: &'a ApplyRequest) -> Result<Vec<Step<'a>>, StoreError> {
    let mut steps = Vec::with_capacity(request.intents.len());
    let invalid = |index: usize, op: &str, why: String| {
        StoreError::InvalidInput(format!("intent {index} ({op}) is invalid: {why}"))
    };
    // Validate against the graph this batch is building, not only what is on
    // disk: two `block` intents can close a loop between them.
    let mut graph = blocked_graph(m)?;
    for (index, intent) in request.intents.iter().enumerate() {
        let op = intent.name();
        match intent {
            Intent::Add(add) => {
                if add.title.trim().is_empty() {
                    return Err(invalid(index, op, "title cannot be empty".into()));
                }
                if let Some(project) = &add.project {
                    ids::validate_project(project)
                        .map_err(|e| invalid(index, op, e.to_string()))?;
                }
                if let Some(parent) = &add.parent {
                    m.store()
                        .resolve(parent)
                        .map_err(|e| invalid(index, op, e.to_string()))?;
                }
                steps.push(Step {
                    index,
                    intent,
                    target: None,
                    rev: None,
                });
            }
            Intent::Checkpoint(t) => steps.push(resolved(m, index, intent, &t.id)?),
            Intent::Status(s) => {
                Status::parse(&s.status).map_err(|e| invalid(index, op, e.to_string()))?;
                steps.push(resolved(m, index, intent, &s.id)?);
            }
            Intent::Log(l) => {
                if l.msg.trim().is_empty() {
                    return Err(invalid(index, op, "message cannot be empty".into()));
                }
                steps.push(resolved(m, index, intent, &l.id)?);
            }
            Intent::Edit(e) => {
                if let Some(parent) = &e.parent {
                    m.store()
                        .resolve(parent)
                        .map_err(|err| invalid(index, op, err.to_string()))?;
                }
                steps.push(resolved(m, index, intent, &e.id)?);
            }
            Intent::Block(e) => {
                let id = resolve(m, index, op, &e.id)?;
                let blocker = resolve(m, index, op, &e.blocker)?;
                if id == blocker {
                    return Err(invalid(index, op, "a task cannot block itself".into()));
                }
                // Checked against the batch's own edges as they accumulate.
                if !blocked_by(&graph, &id).contains(&blocker) && reaches(&graph, &blocker, &id) {
                    return Err(invalid(
                        index,
                        op,
                        format!(
                            "would create a circular dependency with {}",
                            short(&blocker)
                        ),
                    ));
                }
                graph.entry(id.clone()).or_default().push(blocker);
                steps.push(Step {
                    index,
                    intent,
                    rev: Some(m.load(&id)?.rev()),
                    target: Some(id),
                });
            }
            Intent::Unblock(e) => {
                let id = resolve(m, index, op, &e.id)?;
                let blocker = resolve(m, index, op, &e.blocker)?;
                if let Some(edges) = graph.get_mut(&id) {
                    edges.retain(|b| b != &blocker);
                }
                steps.push(Step {
                    index,
                    intent,
                    rev: Some(m.load(&id)?.rev()),
                    target: Some(id),
                });
            }
            Intent::Link(l) | Intent::Unlink(l) | Intent::Accept(l) | Intent::Evidence(l) => {
                if !l.values.is_empty() && (!l.remove.is_empty() || l.clear) {
                    return Err(invalid(index, op, "add or remove/clear, not both".into()));
                }
                steps.push(resolved(m, index, intent, &l.id)?);
            }
            Intent::Archive(a) => {
                let id = resolve(m, index, op, &a.id)?;
                let record = m.load(&id)?;
                if !record.state.status.is_terminal() {
                    return Err(invalid(
                        index,
                        op,
                        format!("{} is {}", record.state.alias, record.state.status),
                    ));
                }
                steps.push(Step {
                    index,
                    intent,
                    rev: Some(record.rev()),
                    target: Some(id),
                });
            }
            Intent::Unarchive(u) => steps.push(resolved(m, index, intent, &u.id)?),
            Intent::Mv(mv) => {
                ids::validate_project(&mv.project)
                    .map_err(|e| invalid(index, op, e.to_string()))?;
                steps.push(resolved(m, index, intent, &mv.id)?);
            }
            Intent::Purge(p) => steps.push(resolved(m, index, intent, &p.id)?),
        }
    }
    Ok(steps)
}

/// A step for an intent that addresses one existing task.
fn resolved<'a>(
    m: &Mutation<'_>,
    index: usize,
    intent: &'a Intent,
    input: &str,
) -> Result<Step<'a>, StoreError> {
    let id = resolve(m, index, intent.name(), input)?;
    let rev = Some(m.load(&id)?.rev());
    Ok(Step {
        index,
        intent,
        target: Some(id),
        rev,
    })
}

fn resolve(m: &Mutation<'_>, index: usize, op: &str, input: &str) -> Result<String, StoreError> {
    m.store()
        .resolve(input)
        .map_err(|e| StoreError::InvalidInput(format!("intent {index} ({op}) is invalid: {e}")))
}

/// `id -> blockers` for every record: the base for batch validation.
fn blocked_graph(
    m: &Mutation<'_>,
) -> Result<std::collections::HashMap<String, Vec<String>>, StoreError> {
    Ok(m.store()
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

/// `-` is how both the CLI and a JSON intent say "clear this".
fn clearable(value: String) -> Option<String> {
    (value != "-").then_some(value)
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Run one validated intent through the shared operation layer.
fn execute(m: &Mutation<'_>, intent: &Intent) -> Result<Option<TaskView>, StoreError> {
    let view = match intent {
        Intent::Add(add) => {
            let parent = add
                .parent
                .as_ref()
                .map(|p| m.store().resolve(p))
                .transpose()?;
            ops::create(
                m,
                CreateOptions {
                    title: add.title.clone(),
                    description: add.desc.clone(),
                    priority: add.priority,
                    project: add.project.clone(),
                    labels: (!add.labels.is_empty()).then(|| add.labels.clone()),
                    parent,
                },
            )?
        }
        Intent::Checkpoint(t) => {
            let id = m.store().resolve(&t.id)?;
            let text = t.text.clone().filter(|_| !t.clear);
            ops::set_checkpoint(m, &id, text, t.if_rev.as_deref())?
        }
        Intent::Status(s) => {
            let id = m.store().resolve(&s.id)?;
            let status =
                Status::parse(&s.status).map_err(|e| StoreError::InvalidInput(e.to_string()))?;
            ops::set_status(m, &id, status)?
        }
        Intent::Log(l) => {
            let id = m.store().resolve(&l.id)?;
            ops::add_log(m, &id, &l.msg)?
        }
        Intent::Edit(e) => {
            let id = m.store().resolve(&e.id)?;
            // The same edit the CLI builds, applied by the same function.
            let edit = Edit {
                title: e.title.clone(),
                description: e.desc.as_ref().map(|d| clearable(d.clone())),
                priority: e.priority,
                parent: e.parent.as_ref().map(|p| clearable(p.clone())),
                labels: e.labels.clone(),
                remove_labels: e.remove_labels.clone(),
                if_rev: e.if_rev.clone(),
            };
            ops::apply_edit(m, &id, &edit)?
        }
        Intent::Block(e) => {
            let id = m.store().resolve(&e.id)?;
            let blocker = m.store().resolve(&e.blocker)?;
            ops::add_blocker(m, &id, &blocker)?
        }
        Intent::Unblock(e) => {
            let id = m.store().resolve(&e.id)?;
            let blocker = m.store().resolve(&e.blocker)?;
            ops::remove_blocker(m, &id, &blocker)?.0
        }
        Intent::Link(l) => list_intent(m, l, ListField::Links)?,
        Intent::Unlink(l) => list_intent(m, l, ListField::Links)?,
        Intent::Accept(l) => list_intent(m, l, ListField::Acceptance)?,
        Intent::Evidence(l) => list_intent(m, l, ListField::Evidence)?,
        Intent::Archive(a) => {
            let id = m.store().resolve(&a.id)?;
            ops::archive(m, &id, a.if_rev.as_deref())?
        }
        Intent::Unarchive(u) => {
            let id = m.store().resolve(&u.id)?;
            ops::unarchive(m, &id)?
        }
        Intent::Mv(mv) => {
            let id = m.store().resolve(&mv.id)?;
            ops::set_project(m, &id, &mv.project)?
        }
        Intent::Purge(p) => {
            let id = m.store().resolve(&p.id)?;
            ops::purge(m, &id, p.scrub, p.if_rev.as_deref())?;
            return Ok(None);
        }
    };
    Ok(Some(view))
}

fn list_intent(
    m: &Mutation<'_>,
    intent: &ListIntent,
    field: ListField,
) -> Result<TaskView, StoreError> {
    let id = m.store().resolve(&intent.id)?;
    let edit = if intent.clear {
        ListEdit::Clear
    } else if !intent.remove.is_empty() {
        ListEdit::Remove(&intent.remove)
    } else {
        ListEdit::Add(&intent.values)
    };
    ops::edit_list(m, &id, field, edit, None)
}

/// Parse a request body.
pub fn parse_request(input: &str) -> Result<ApplyRequest, StoreError> {
    serde_json::from_str(input).map_err(|err| StoreError::Parse {
        what: "intent".into(),
        err: err.to_string(),
    })
}

/// The batch, as a command runs it.
pub fn run(ctx: &AppCtx, dry_run: bool) -> miette::Result<()> {
    use std::io::Read as _;
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .into_diagnostic()?;
    if input.trim().is_empty() {
        return Err(crate::output::invalid(
            "no intents on stdin; pipe a JSON body like \
             {\"intents\": [{\"op\": \"log\", \"id\": \"a7b3\", \"msg\": \"...\"}]}",
        ));
    }
    let request = parse_request(&input)?;
    ctx.require_store()?;
    let m = Mutation::locked(&ctx.store)?;
    let outcome = apply(&m, &request, dry_run)?;
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
            Intent::Block(edge) => assert_eq!(edge.blocker, "b7c4"),
            other => panic!("expected block, got {}", other.name()),
        }

        // A misspelled field must fail rather than silently do nothing.
        let err =
            parse_request(r#"{"intents":[{"op":"log","id":"a7b3","message":"hi"}]}"#).unwrap_err();
        assert!(err.to_string().contains("message"), "{err}");

        // An unknown op is a bad request, not a silent no-op.
        assert!(parse_request(r#"{"intents":[{"op":"teleport","id":"x"}]}"#).is_err());

        // Fields that no longer exist are rejected too, so a stale caller finds
        // out rather than having the key ignored.
        assert!(parse_request(r#"{"intents":[{"op":"add","title":"t","due":"+7d"}]}"#).is_err());
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
