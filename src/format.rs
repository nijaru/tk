//! Output formatting: task tables, detail views, config display, JSON.
//!
//! The table shows the 4-character alias — the handle people type — while the
//! detail view shows both the alias and the full ULID. JSON always carries both.

use std::io::IsTerminal as _;

use owo_colors::OwoColorize;

use crate::model::{Config, Priority, Status, TaskState, TaskView};
use crate::timeutil;

/// Color when stdout is a TTY and `NO_COLOR` is unset.
pub fn use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Truncate to `max` characters (Unicode-safe), appending `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    if max <= 1 {
        return "…".to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn paint(color: bool, text: &str, style: Style) -> String {
    if !color {
        return text.to_owned();
    }
    match style {
        Style::Plain => text.to_owned(),
        Style::Dim => text.dimmed().to_string(),
        Style::Red => text.red().to_string(),
        Style::RedBold => text.red().bold().to_string(),
        Style::Yellow => text.yellow().to_string(),
        Style::Blue => text.blue().to_string(),
        Style::Cyan => text.cyan().to_string(),
    }
}

#[derive(Clone, Copy)]
enum Style {
    Plain,
    Dim,
    Red,
    RedBold,
    Yellow,
    Blue,
    Cyan,
}

fn status_style(s: Status) -> Style {
    match s {
        Status::Open => Style::Blue,
        Status::Active => Style::Cyan,
        Status::Done => Style::Dim,
        Status::Deferred => Style::Plain,
        Status::Closed => Style::Plain,
    }
}

fn priority_style(p: Priority) -> Style {
    match p {
        Priority::Urgent => Style::RedBold,
        Priority::High => Style::Red,
        Priority::Medium => Style::Yellow,
        Priority::Low => Style::Blue,
        Priority::None => Style::Dim,
    }
}

/// Width of the handle column: the longest alias in the result set.
fn alias_width(tasks: &[TaskView]) -> usize {
    tasks
        .iter()
        .map(|t| t.task.alias.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 12)
}

fn format_task_row_w(t: &TaskView, color: bool, w: usize) -> String {
    let prio = format!("{:<4}", t.task.priority.short());
    let mut status_text = t.task.status.to_string();
    if t.task.status.is_terminal()
        && let Some(c) = &t.task.completed_at
    {
        status_text = format!("{} {}", t.task.status, timeutil::format_relative(c));
    }
    let status = format!("{status_text:<12}");
    let title = truncate(&t.task.title, 50);

    if color {
        let sc = if t.is_overdue {
            Style::RedBold
        } else if t
            .days_until_due
            .is_some_and(|d| d <= timeutil::DUE_SOON_THRESHOLD)
        {
            Style::Yellow
        } else {
            status_style(t.task.status)
        };
        let tc = if t.task.status == Status::Done {
            Style::Dim
        } else {
            Style::Plain
        };
        return format!(
            "{:<w$} | {} | {} | {}",
            t.task.alias,
            paint(color, &prio, priority_style(t.task.priority)),
            paint(color, &status, sc),
            paint(color, &title, tc),
        );
    }

    let mut markers = String::new();
    if t.task.is_archived() {
        markers += " [archived]";
    }
    if t.is_overdue {
        markers += " [OVERDUE]";
    } else if let Some(d) = t.days_until_due
        && d <= timeutil::DUE_SOON_THRESHOLD
    {
        if d == 0 {
            markers += " [due today]";
        } else {
            markers += &format!(" [due {d}d]");
        }
    }
    format!(
        "{:<w$} | {prio} | {status} | {title}{markers}",
        t.task.alias,
        w = w
    )
}

pub fn format_task_list(tasks: &[TaskView], empty_hint: &str, color: bool) -> String {
    if tasks.is_empty() {
        if empty_hint.is_empty() {
            return "No tasks found. Run 'tk add \"title\"' to create one.".to_owned();
        }
        return empty_hint.to_owned();
    }
    let w = alias_width(tasks);
    let header = format!("{:<w$} | PRIO | STATUS       | TITLE", "REF");
    let divider = "-".repeat(header.chars().count());
    let mut rows = vec![header, divider];
    rows.extend(tasks.iter().map(|t| format_task_row_w(t, color, w)));
    rows.join("\n")
}

pub fn format_task_detail(t: &TaskView, color: bool) -> String {
    let task: &TaskState = &t.task;
    let mut lines = Vec::new();
    lines.push(format!("ID:          {}", task.id));
    lines.push(format!("Ref:         {}", task.alias));
    if !task.legacy_aliases.is_empty() {
        lines.push(format!("Also known:  {}", task.legacy_aliases.join(", ")));
    }
    lines.push(format!("Project:     {}", task.project));
    if !task.title.is_empty() {
        lines.push(format!("Title:       {}", task.title));
    }
    let sc = if t.is_overdue {
        Style::RedBold
    } else {
        status_style(task.status)
    };
    lines.push(format!(
        "Status:      {}",
        paint(color, &task.status.to_string(), sc)
    ));
    lines.push(format!(
        "Priority:    {}",
        paint(color, task.priority.name(), priority_style(task.priority))
    ));
    if let Some(d) = &task.description {
        lines.push(format!("Description: {d}"));
    }
    if !task.labels.is_empty() {
        lines.push(format!("Labels:      {}", task.labels.join(", ")));
    }
    if !task.assignees.is_empty() {
        lines.push(format!("Assignees:   {}", task.assignees.join(", ")));
    }
    if let Some(a) = &task.assignee {
        lines.push(format!("Assignee:    {a}"));
    }
    if let Some(p) = &t.parent_ref {
        lines.push(format!("Parent:      {p}"));
    }
    if let Some(e) = task.estimate {
        lines.push(format!("Estimate:    {e}"));
    }
    if let Some(d) = &task.due_date {
        let mut due = d.clone();
        if t.is_overdue {
            due += &paint(color, " [OVERDUE]", Style::RedBold);
        } else if let Some(n) = t.days_until_due
            && n <= timeutil::DUE_SOON_THRESHOLD
        {
            if n == 0 {
                due += &paint(color, " [due today]", Style::Yellow);
            } else {
                due += &paint(color, &format!(" [due {n}d]"), Style::Yellow);
            }
        }
        lines.push(format!("Due:         {due}"));
    }
    lines.push(format!(
        "Created:     {}",
        timeutil::format_date(&task.created_at)
    ));
    lines.push(format!(
        "Updated:     {}",
        timeutil::format_date(&task.updated_at)
    ));
    if let Some(c) = &task.completed_at {
        lines.push(format!("Completed:   {}", timeutil::format_date(c)));
    }
    if let Some(a) = &task.archived_at {
        lines.push(format!("Archived:    {}", timeutil::format_date(a)));
    }
    lines.push(format!("Revision:    {}", t.rev));
    if !t.unresolved_blockers.is_empty() {
        lines.push(format!(
            "Unresolved:  {} (task missing from the store)",
            t.unresolved_blockers
                .iter()
                .map(|id| id.chars().take(8).collect::<String>())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !task.blocked_by.is_empty() {
        let state = if t.blocked_by_incomplete {
            " (blocked)"
        } else {
            " (resolved)"
        };
        lines.push(format!("Blockers:    {}{state}", t.blocker_refs.join(", ")));
    }
    if !t.task.related.is_empty() {
        lines.push(format!("Related:     {}", t.related_refs.join(", ")));
    }
    if let Some(c) = &task.checkpoint {
        lines.push(String::new());
        lines.push(format!("Checkpoint:  {c}"));
    }
    if !task.links.is_empty() {
        lines.push(format!("Links:       {}", task.links.join(", ")));
    }
    if !task.acceptance.is_empty() {
        lines.push(String::new());
        lines.push("Acceptance:".to_owned());
        for a in &task.acceptance {
            lines.push(format!("  - {a}"));
        }
    }
    if !task.evidence.is_empty() {
        lines.push(String::new());
        lines.push("Evidence:".to_owned());
        for e in &task.evidence {
            lines.push(format!("  - {e}"));
        }
    }
    if !task.logs.is_empty() {
        lines.push(String::new());
        lines.push("Log:".to_owned());
        for log in &task.logs {
            lines.push(format!(
                "  [{}] {}",
                timeutil::format_date(&log.ts),
                log.msg
            ));
        }
    }
    lines.join("\n")
}

/// Single-line warning (yellow when color is on).
pub fn warning(text: &str, color: bool) -> String {
    paint(color, text, Style::Yellow)
}

pub fn format_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned())
}

pub fn format_config(config: &Config) -> String {
    let mut lines = vec![
        format!("Format:      {}", config.format),
        format!("Project:     {}", config.project),
    ];
    if config.clean_after.enabled {
        lines.push(format!("Clean After: {} days", config.clean_after.days));
    } else {
        lines.push("Clean After: disabled".to_owned());
    }
    if !config.defaults.labels.is_empty() {
        lines.push(format!(
            "Def Labels:  {}",
            config.defaults.labels.join(", ")
        ));
    }
    if !config.defaults.assignees.is_empty() {
        lines.push(format!(
            "Def Assigns: {}",
            config.defaults.assignees.join(", ")
        ));
    }
    lines.push(format!("Def Prio:    {}", config.defaults.priority.name()));
    if let Some(aliases) = &config.aliases
        && !aliases.is_empty()
    {
        lines.push(String::new());
        lines.push("Aliases:".to_owned());
        for (k, v) in aliases {
            lines.push(format!("  {k:<10} -> {v}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LogEntry, TaskState};

    fn sample() -> TaskView {
        TaskView {
            task: TaskState {
                id: "01j8x0m5r7000000000000000a".into(),
                alias: "a7b3".into(),
                legacy_aliases: Vec::new(),
                project: "demo".into(),
                title: "Implement auth".into(),
                description: None,
                status: Status::Open,
                priority: Priority::Urgent,
                labels: vec![],
                assignees: vec![],
                assignee: None,
                attempt: 0,
                parent: None,
                blocked_by: vec![],
                related: vec![],
                estimate: None,
                due_date: Some("2000-01-01".into()),
                logs: vec![LogEntry {
                    ts: "2026-01-10T12:00:00.000000000Z".into(),
                    msg: "note".into(),
                }],
                checkpoint: None,
                links: Vec::new(),
                acceptance: Vec::new(),
                evidence: Vec::new(),
                created_at: "2026-01-10T12:00:00.000000000Z".into(),
                updated_at: "2026-01-10T12:00:00.000000000Z".into(),
                completed_at: None,
                archived_at: None,
            },
            rev: "1-aaaa:1:deadbeef".into(),
            blocked_by_incomplete: false,
            unresolved_blockers: Vec::new(),
            blocker_refs: Vec::new(),
            related_refs: Vec::new(),
            parent_ref: None,
            is_overdue: true,
            days_until_due: None,
        }
    }

    #[test]
    fn truncate_is_unicode_safe() {
        assert_eq!(truncate("héllo→world", 6), "héllo…");
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 1), "…");
    }

    #[test]
    fn the_table_uses_the_alias_not_the_ulid() {
        let t = sample();
        let table = format_task_list(std::slice::from_ref(&t), "", false);
        assert!(table.contains("a7b3"), "{table}");
        assert!(
            !table.contains("01j8x0m5r7000000000000000a"),
            "the 26-character ID must not widen every row: {table}"
        );
        assert!(table.contains("[OVERDUE]"), "{table}");
    }

    #[test]
    fn detail_shows_both_handles_and_renders_timestamps() {
        let t = sample();
        let detail = format_task_detail(&t, false);
        assert!(detail.contains("01j8x0m5r7000000000000000a"), "{detail}");
        assert!(detail.contains("Ref:         a7b3"), "{detail}");
        assert!(detail.contains("[OVERDUE]"), "{detail}");
        // Nano timestamps must render, not pass through raw.
        assert!(
            !detail.contains("2026-01-10T12:00:00.000000000Z"),
            "{detail}"
        );
    }

    #[test]
    fn references_render_as_aliases_not_ulids() {
        let mut t = sample();
        t.task.blocked_by = vec!["01m25qbfpr5ekbr9zxh0xc93kx".into()];
        t.blocker_refs = vec!["vp80".into()];
        t.blocked_by_incomplete = true;
        t.parent_ref = Some("jvv2".into());
        let detail = format_task_detail(&t, false);
        assert!(detail.contains("Blockers:    vp80 (blocked)"), "{detail}");
        assert!(detail.contains("Parent:      jvv2"), "{detail}");
        assert!(
            !detail.contains("Blockers:    01m25qbf"),
            "the blocker line must not lead with a ULID: {detail}"
        );
    }
}
