//! Output formatting: the table, the detail view, configuration, JSON.
//!
//! Human output leads with the ref, because that is what people type back. JSON
//! carries the whole document, and every command emits it through one envelope
//! (see [`crate::output`]) so an agent never has to parse prose.

use std::io::IsTerminal as _;

use owo_colors::OwoColorize;

use crate::model::{Config, EntryView, State};
use crate::store::Ctx;
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

#[derive(Clone, Copy)]
enum Style {
    Dim,
    Yellow,
    Blue,
    Cyan,
}

fn paint(color: bool, text: &str, style: Style) -> String {
    if !color {
        return text.to_owned();
    }
    match style {
        Style::Dim => text.dimmed().to_string(),
        Style::Yellow => text.yellow().to_string(),
        Style::Blue => text.blue().to_string(),
        Style::Cyan => text.cyan().to_string(),
    }
}

fn state_style(state: State) -> Style {
    match state {
        State::Open => Style::Cyan,
        State::Done => Style::Dim,
        State::Dropped => Style::Dim,
    }
}

/// One row of the list: ref, state, labels, title.
fn render_row(view: &EntryView, color: bool) -> String {
    let entry = &view.entry;
    let labels = if entry.labels.is_empty() {
        String::new()
    } else {
        truncate(&entry.labels.join(","), 24)
    };
    let title = truncate(&entry.title, 60);
    format!(
        "{} | {} | {} | {}{}",
        paint(color, &entry.r#ref, Style::Blue),
        paint(
            color,
            &format!("{:<7}", entry.state),
            state_style(entry.state)
        ),
        paint(color, &format!("{labels:<24}"), Style::Dim),
        title,
        blocked_marker(view, color)
    )
    .trim_end()
    .to_owned()
}

/// ` [blocked]` for an entry waiting on something, and a distinct mark when what
/// it waits on is not in the store.
fn blocked_marker(view: &EntryView, color: bool) -> String {
    if !view.is_waiting() {
        return String::new();
    }
    let text = if view.unresolved_blockers.is_empty() {
        " [blocked]"
    } else {
        " [blocked?]"
    };
    paint(color, text, Style::Yellow)
}

/// A list of entries, or `empty_hint` when there are none.
pub fn render_list(views: &[EntryView], empty_hint: &str, color: bool) -> String {
    if views.is_empty() {
        return empty_hint.to_owned();
    }
    let mut lines: Vec<String> = views.iter().map(|v| render_row(v, color)).collect();
    let blocked = views.iter().filter(|v| v.is_waiting()).count();
    let mut summary = format!(
        "{} entr{}",
        views.len(),
        if views.len() == 1 { "y" } else { "ies" }
    );
    if blocked > 0 {
        summary.push_str(&format!(", {blocked} blocked"));
    }
    lines.push(String::new());
    lines.push(paint(color, &summary, Style::Dim));
    lines.join("\n")
}

/// The whole document, rendered for a person: fields, criteria, then the log.
pub fn render_detail(view: &EntryView, color: bool) -> String {
    let entry = &view.entry;
    let mut lines = vec![format!(
        "{}  {}",
        paint(color, &entry.r#ref, Style::Blue),
        entry.title
    )];

    let mut field = |label: &str, value: String| {
        if value.is_empty() {
            return;
        }
        // A value can contain a newline (a pasted status, a multi-line log
        // message); align its continuation lines instead of letting them run
        // back to column zero.
        let mut parts = value.split('\n');
        let first = parts.next().unwrap_or_default();
        lines.push(format!(
            "      {} {}",
            paint(color, &format!("{label:<8}"), Style::Dim),
            first
        ));
        for part in parts {
            lines.push(format!("      {} {}", " ".repeat(8), part));
        }
    };

    field(
        "state",
        paint(color, entry.state.as_str(), state_style(entry.state)).to_string(),
    );
    field("labels", entry.labels.join(", "));
    field(
        "created",
        format!(
            "{} ({})",
            timeutil::format_date(&entry.created),
            timeutil::format_relative(&entry.created)
        ),
    );
    field(
        "updated",
        format!(
            "{} ({})",
            timeutil::format_date(&entry.updated),
            timeutil::format_relative(&entry.updated)
        ),
    );
    if let Some(done) = &entry.done {
        field("done", timeutil::format_date(done));
    }
    if !entry.blocked_by.is_empty() {
        let mut blockers = entry.blocked_by.join(", ");
        if !view.unresolved_blockers.is_empty() {
            blockers.push_str(&format!(
                "  (not in this store: {})",
                view.unresolved_blockers.join(", ")
            ));
        }
        field(
            "blocked",
            paint(color, &blockers, Style::Yellow).to_string(),
        );
    }
    if let Some(status) = &entry.status {
        field("status", status.clone());
    }
    for (index, item) in entry.acceptance.iter().enumerate() {
        let label = if index == 0 { "accept" } else { "" };
        field(label, format!("{}. {}", index + 1, item));
    }
    field("file", paint(color, &view.file, Style::Dim).to_string());
    field("rev", paint(color, &view.rev, Style::Dim).to_string());

    if !entry.log.is_empty() {
        lines.push(String::new());
        lines.push(paint(color, "      log", Style::Dim).to_string());
        for line in &entry.log {
            let stamp = paint(color, &timeutil::format_date(&line.ts), Style::Dim);
            let mut parts = line.msg.split('\n');
            lines.push(format!(
                "        {stamp}  {}",
                parts.next().unwrap_or_default()
            ));
            for part in parts {
                lines.push(format!("                  {part}"));
            }
        }
    }
    lines.join("\n")
}

/// One line confirming a change: ref, state, title.
pub fn render_summary(view: &EntryView) -> String {
    format!(
        "{}  {}  {}",
        view.entry.r#ref,
        view.entry.state,
        truncate(&view.entry.title, 60)
    )
}

/// What a store holds, and where it is.
pub fn render_config(ctx: &Ctx, config: &Config, entries: usize) -> String {
    let mut lines = vec![
        format!("Store:   {}", ctx.tasks_dir.display()),
        format!("Found:   {}", ctx.source.name()),
        format!("Format:  {}", config.format),
        format!("Entries: {entries}"),
    ];
    match &config.aliases {
        Some(aliases) if !aliases.is_empty() => {
            lines.push(String::new());
            lines.push("Aliases:".to_owned());
            for (name, path) in aliases {
                lines.push(format!("  {name:<10} -> {path}"));
            }
        }
        _ => {}
    }
    lines.join("\n")
}

/// A warning line, for issues that do not stop a command.
pub fn warning(text: &str, color: bool) -> String {
    paint(color, &format!("warning: {text}"), Style::Yellow)
}

pub fn format_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entry, LogEntry};

    fn view(title: &str) -> EntryView {
        let mut entry = Entry::new("a7b3".into(), title.into(), "2026-01-10T12:00:00Z".into());
        entry.labels = vec!["backend".into(), "api".into()];
        entry.status = Some("Halfway".into());
        entry.acceptance = vec!["parity test passes".into()];
        entry.log = vec![LogEntry {
            ts: "2026-01-10T09:00:00Z".into(),
            msg: "Started with the JWT approach.".into(),
        }];
        EntryView {
            entry,
            rev: "0123456789abcdef".into(),
            unresolved_blockers: Vec::new(),
            blocking: Vec::new(),
            file: "a7b3-rewrite-the-auth-layer.json".into(),
        }
    }

    #[test]
    fn a_row_leads_with_the_ref_and_has_no_trailing_space() {
        let row = render_row(&view("Rewrite the auth layer"), false);
        assert!(row.starts_with("a7b3 | open"), "{row}");
        assert!(row.contains("backend,api"), "{row}");
        assert!(row.contains("Rewrite the auth layer"), "{row}");
        assert_eq!(row, row.trim_end());
    }

    #[test]
    fn a_blocked_row_says_so_and_a_missing_blocker_says_more() {
        let mut blocked = view("Alpha");
        blocked.entry.blocked_by = vec!["b7c4".into()];
        blocked.blocking = vec!["b7c4".into()];
        assert!(render_row(&blocked, false).contains("[blocked]"));
        blocked.unresolved_blockers = vec!["b7c4".into()];
        assert!(render_row(&blocked, false).contains("[blocked?]"));
        blocked.blocking.clear();
        assert!(
            !render_row(&blocked, false).contains("blocked"),
            "a finished blocker is not a marker"
        );
        // A done entry is not waiting, even while it names an open blocker.
        let mut done = blocked.clone();
        done.blocking = vec!["b7c4".into()];
        done.entry.state = State::Done;
        assert!(!render_row(&done, false).contains("blocked"));
    }

    #[test]
    fn an_empty_list_says_what_to_do_instead() {
        assert_eq!(render_list(&[], "no entries", false), "no entries");
    }

    #[test]
    fn the_summary_counts_entries_and_blocked_ones() {
        let mut blocked = view("Beta");
        blocked.entry.r#ref = "b7c4".into();
        blocked.entry.blocked_by = vec!["zzzz".into()];
        blocked.blocking = vec!["zzzz".into()];
        let out = render_list(&[view("Alpha"), blocked], "none", false);
        assert!(out.contains("2 entries, 1 blocked"), "{out}");
        let one = render_list(&[view("Alpha")], "none", false);
        assert!(one.contains("1 entry"), "{one}");
    }

    #[test]
    fn detail_shows_every_field_that_is_set_and_skips_what_is_not() {
        let out = render_detail(&view("Rewrite the auth layer"), false);
        for expected in [
            "Rewrite the auth layer",
            "state",
            "open",
            "labels",
            "backend, api",
            "status",
            "Halfway",
            "accept",
            "1. parity test passes",
            "log",
            "Started with the JWT approach.",
            "a7b3-rewrite-the-auth-layer.json",
        ] {
            assert!(out.contains(expected), "{expected} missing from:\n{out}");
        }
        assert!(
            !out.contains("done"),
            "an open entry has no done time:\n{out}"
        );
        assert!(!out.contains("blocked"), "nothing is blocking it:\n{out}");
    }

    #[test]
    fn detail_lists_acceptance_in_order_and_marks_missing_blockers() {
        let mut v = view("Alpha");
        v.entry.acceptance = vec!["first".into(), "second".into()];
        v.entry.blocked_by = vec!["b7c4".into()];
        v.unresolved_blockers = vec!["b7c4".into()];
        v.blocking = vec!["b7c4".into()];
        let out = render_detail(&v, false);
        assert!(out.contains("1. first"), "{out}");
        assert!(out.contains("2. second"), "{out}");
        assert!(out.contains("not in this store: b7c4"), "{out}");
    }

    #[test]
    fn a_multi_line_value_stays_aligned() {
        let mut v = view("Alpha");
        v.entry.status = Some("first line\nsecond line".into());
        v.entry.log = vec![LogEntry {
            ts: "2026-01-10T09:00:00Z".into(),
            msg: "one\ntwo\nthree".into(),
        }];
        let out = render_detail(&v, false);
        for line in out.lines() {
            assert!(
                line.is_empty() || line.starts_with(' ') || line.starts_with("a7b3"),
                "every line is indented or the heading: {line:?}"
            );
        }
        assert!(out.contains("first line"), "{out}");
        assert!(out.contains("second line"), "{out}");
        assert!(!out.contains("\nsecond"), "the newline is not printed");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("日本語テキスト", 3), "日本…");
        assert_eq!(truncate("anything", 1), "…");
    }
}
