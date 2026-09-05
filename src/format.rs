//! Output formatting: task tables, detail views, config display, JSON.
//!
//! Fixes vs the Go version:
//! - Truncation counts Unicode scalar values, not bytes.
//! - The ID column sizes to the content instead of a fixed 11 chars.
//! - Timestamp rendering goes through [`crate::timeutil`], which understands
//!   the fractional-second timestamps the store actually writes.

use std::io::IsTerminal as _;

use owo_colors::OwoColorize;

use crate::model::{Config, Priority, Status, TaskView};
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

fn id_width(tasks: &[TaskView]) -> usize {
    tasks
        .iter()
        .map(|t| t.id.chars().count())
        .max()
        .unwrap_or(2)
        .clamp(11, 32)
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
            t.id,
            paint(color, &prio, priority_style(t.task.priority)),
            paint(color, &status, sc),
            paint(color, &title, tc),
        );
    }

    let mut markers = String::new();
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
    format!("{:<w$} | {prio} | {status} | {title}{markers}", t.id, w = w)
}

pub fn format_task_list(tasks: &[TaskView], empty_hint: &str, color: bool) -> String {
    if tasks.is_empty() {
        if empty_hint.is_empty() {
            return "No tasks found. Run 'tk add \"title\"' to create one.".to_owned();
        }
        return empty_hint.to_owned();
    }
    let w = id_width(tasks);
    let header = format!("{:<w$} | PRIO | STATUS       | TITLE", "ID");
    let divider = "-".repeat(header.chars().count());
    let mut rows = vec![header, divider];
    rows.extend(tasks.iter().map(|t| format_task_row_w(t, color, w)));
    rows.join("\n")
}

pub fn format_task_detail(t: &TaskView, color: bool) -> String {
    let mut lines = Vec::new();
    lines.push(format!("ID:          {}", t.id));
    if !t.task.title.is_empty() {
        lines.push(format!("Title:       {}", t.task.title));
    }
    let sc = if t.is_overdue {
        Style::RedBold
    } else {
        status_style(t.task.status)
    };
    lines.push(format!(
        "Status:      {}",
        paint(color, &t.task.status.to_string(), sc)
    ));
    lines.push(format!(
        "Priority:    {}",
        paint(
            color,
            t.task.priority.name(),
            priority_style(t.task.priority)
        )
    ));
    if let Some(d) = &t.task.description {
        lines.push(format!("Description: {d}"));
    }
    if !t.task.labels.is_empty() {
        lines.push(format!("Labels:      {}", t.task.labels.join(", ")));
    }
    if !t.task.assignees.is_empty() {
        lines.push(format!("Assignees:   {}", t.task.assignees.join(", ")));
    }
    if let Some(p) = &t.task.parent {
        lines.push(format!("Parent:      {p}"));
    }
    if let Some(e) = t.task.estimate {
        lines.push(format!("Estimate:    {e}"));
    }
    if let Some(d) = &t.task.due_date {
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
        timeutil::format_date(&t.task.created_at)
    ));
    lines.push(format!(
        "Updated:     {}",
        timeutil::format_date(&t.task.updated_at)
    ));
    if let Some(c) = &t.task.completed_at {
        lines.push(format!("Completed:   {}", timeutil::format_date(c)));
    }
    if !t.task.blocked_by.is_empty() {
        let state = if t.blocked_by_incomplete {
            " (blocked)"
        } else {
            " (resolved)"
        };
        lines.push(format!(
            "Blockers:    {}{state}",
            t.task.blocked_by.join(", ")
        ));
    }
    if !t.task.logs.is_empty() {
        lines.push(String::new());
        lines.push("Log:".to_owned());
        for log in &t.task.logs {
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
        format!("Version:     {}", config.version),
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
    use crate::model::{LogEntry, Task};

    fn sample() -> TaskView {
        crate::store::enrich(
            &crate::store::Ctx {
                cwd: "/tmp".into(),
                root: "/tmp".into(),
                tasks_dir: "/tmp".into(),
                exists: false,
            },
            &Task {
                project: "tk".into(),
                r#ref: "a7b3".into(),
                title: "Implement auth".into(),
                description: None,
                status: Status::Open,
                priority: Priority::Urgent,
                labels: vec![],
                assignees: vec![],
                parent: None,
                blocked_by: vec![],
                estimate: None,
                due_date: Some("2000-01-01".into()),
                logs: vec![LogEntry {
                    ts: "2026-01-10T12:00:00.000000000Z".into(),
                    msg: "note".into(),
                }],
                created_at: "2026-01-10T12:00:00.000000000Z".into(),
                updated_at: "2026-01-10T12:00:00.000000000Z".into(),
                completed_at: None,
            },
            &std::collections::HashMap::new(),
        )
    }

    #[test]
    fn truncate_is_unicode_safe() {
        assert_eq!(truncate("héllo→world", 6), "héllo…");
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 1), "…");
    }

    #[test]
    fn plain_markers_and_detail() {
        let t = sample();
        assert!(t.is_overdue);
        let row = format_task_row_w(&t, false, 11);
        assert!(row.contains("[OVERDUE]"), "{row}");
        let detail = format_task_detail(&t, false);
        assert!(detail.contains("ID:          tk-a7b3"), "{detail}");
        assert!(detail.contains("[OVERDUE]"), "{detail}");
        // Nano timestamps must render, not pass through raw.
        assert!(
            !detail.contains("2026-01-10T12:00:00.000000000Z"),
            "{detail}"
        );
    }
}
