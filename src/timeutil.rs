//! Timestamps: one place that knows how tk writes and renders them.
//!
//! Dropped with due dates: the relative-date parser (`+7d`), calendar-month
//! arithmetic, and the overdue/countdown logic. Across 169 real tasks no due
//! date was ever set, and that arithmetic — DST, month lengths, calendar days
//! versus 24-hour spans — was the most intricate thing in the crate. Reinstating
//! it means re-adding an event op, a state field, and that date math.

use chrono::{DateTime, Local, Utc};

/// RFC3339Nano in UTC, the stamp every event carries.
pub fn now_rfc3339_nano() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn parse_timestamp(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Short relative age like `2d`, `5h`, `3m`, `now` (or `?` when unparseable).
pub fn format_relative(ts: &str) -> String {
    let Some(t) = parse_timestamp(ts) else {
        return "?".to_owned();
    };
    let diff = Utc::now().signed_duration_since(t);
    if diff.num_days() >= 1 {
        format!("{}d", diff.num_days())
    } else if diff.num_hours() >= 1 {
        format!("{}h", diff.num_hours())
    } else if diff.num_minutes() >= 1 {
        format!("{}m", diff.num_minutes())
    } else {
        "now".to_owned()
    }
}

/// Local `YYYY-MM-DD HH:MM` for detail views; passes through when unparseable.
pub fn format_date(ts: &str) -> String {
    match parse_timestamp(ts) {
        Some(t) => t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string(),
        None => ts.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nano_timestamps_format() {
        // The Go writer emitted RFC3339Nano but only parsed RFC3339, so these
        // rendered as "?" — reading must accept the fractional seconds it wrote.
        let ts = "2026-01-10T12:34:56.123456789Z";
        assert_ne!(format_relative(ts), "?");
        assert_ne!(format_date(ts), ts);
        assert_eq!(format_relative("not-a-date"), "?");
        assert_eq!(format_date("not-a-date"), "not-a-date");
    }

    #[test]
    fn written_stamps_read_back() {
        let now = now_rfc3339_nano();
        assert_ne!(format_date(&now), now);
        assert!(now.ends_with('Z'), "{now}");
        // Nanosecond precision, so two writes in the same second differ.
        let later = now_rfc3339_nano();
        assert_ne!(now, later);
    }
}
