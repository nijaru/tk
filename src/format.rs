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
        let tc = if t.task.status == Status::Done {
            Style::Dim
        } else {
            Style::Plain
        };
        return format!(
            "{:<w$} | {} | {} | {}",
            t.task.alias,
            paint(color, &prio, priority_style(t.task.priority)),
            paint(color, &status, status_style(t.task.status)),
            paint(color, &title, tc),
        );
    }

    let mut markers = String::new();
    if t.task.is_archived() {
        markers += " [archived]";
    }
    if t.blocked_by_incomplete {
        markers += " [blocked]";
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
    lines.push(format!(
        "Status:      {}",
        paint(color, &task.status.to_string(), status_style(task.status))
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
    if let Some(p) = &t.parent_ref {
        lines.push(format!("Parent:      {p}"));
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
    lines.push(format!("Def Prio:    {}", config.defaults.priority.name()));
    if let Some(aliases) = &config.aliases
        && !aliases.is_empty()
    {
        lines.push(String::new());
        lines.push("Aliases:".to_owned());
        for (name, path) in aliases {
            lines.push(format!("  {name:<10} -> {path}"));
        }
    }
    lines.push(String::new());
    lines.push(
        "Change a setting with: tk config set <project|priority|labels|clean-after> <value>"
            .to_owned(),
    );
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
                parent: None,
                blocked_by: vec![],
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
            parent_ref: None,
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
    }

    #[test]
    fn a_blocked_task_is_marked() {
        let mut t = sample();
        t.blocked_by_incomplete = true;
        t.task.blocked_by = vec!["01m25qbfpr5ekbr9zxh0xc93kx".into()];
        t.blocker_refs = vec!["vp80".into()];
        let table = format_task_list(std::slice::from_ref(&t), "", false);
        assert!(table.contains("[blocked]"), "{table}");

        let detail = format_task_detail(&t, false);
        assert!(detail.contains("Blockers:    vp80 (blocked)"), "{detail}");
        assert!(
            !detail.contains("Blockers:    01m25qbf"),
            "the blocker line must not lead with a ULID: {detail}"
        );
    }

    #[test]
    fn detail_shows_both_handles_and_renders_timestamps() {
        let t = sample();
        let detail = format_task_detail(&t, false);
        assert!(detail.contains("01j8x0m5r7000000000000000a"), "{detail}");
        assert!(detail.contains("Ref:         a7b3"), "{detail}");
        // Nano timestamps must render, not pass through raw.
        assert!(
            !detail.contains("2026-01-10T12:00:00.000000000Z"),
            "{detail}"
        );
    }
}
