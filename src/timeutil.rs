//! Dates: due-date parsing, overdue logic, and human-readable timestamps.
//!
//! Design notes vs the Go version:
//! - All timestamp *parsing* accepts RFC 3339 with or without fractional
//!   seconds (the Go `FormatRelative`/`FormatDate` only tried `RFC3339` while
//!   writes were `RFC3339Nano`, so done-row ages rendered as `?`).
//! - Day arithmetic uses calendar dates, not `hours / 24`, so DST can't skew
//!   "days until due" by one.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use thiserror::Error;

pub const DUE_SOON_THRESHOLD: i64 = 7;
const DATE_FMT: &str = "%Y-%m-%d";

#[derive(Debug, Error)]
pub enum DateError {
    #[error("invalid date {0:?}: use YYYY-MM-DD or relative like +7d")]
    BadDate(String),
    #[error("invalid relative date {0:?}: use +Nh/+Nd/+Nw/+Nm")]
    BadRelative(String),
}

/// Parse a due-date flag. Returns `Ok(None)` for `""`/`"-"` (clear signal).
pub fn parse_due_date(input: &str) -> Result<Option<String>, DateError> {
    if input.is_empty() || input == "-" {
        return Ok(None);
    }
    if let Some(rest) = input.strip_prefix('+') {
        return parse_relative_date(rest).map(Some);
    }
    let d = NaiveDate::parse_from_str(input, DATE_FMT)
        .map_err(|_| DateError::BadDate(input.to_owned()))?;
    // Re-format to normalize (rejects e.g. 2026-02-31, which chrono already
    // rejects, and any non-canonical spelling).
    let back = d.format(DATE_FMT).to_string();
    if back != input {
        return Err(DateError::BadDate(input.to_owned()));
    }
    Ok(Some(back))
}

fn parse_relative_date(s: &str) -> Result<String, DateError> {
    let err = || DateError::BadRelative(format!("+{s}"));
    if s.len() < 2 {
        return Err(err());
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: i64 = num.parse().map_err(|_| err())?;
    if n < 0 {
        return Err(err());
    }
    let today = Local::now().date_naive();
    let d = match unit {
        "d" => today
            .checked_add_days(chrono::Days::new(n as u64))
            .ok_or_else(err)?,
        "w" => today
            .checked_add_days(chrono::Days::new((n as u64) * 7))
            .ok_or_else(err)?,
        "m" => add_months(today, n).ok_or_else(err)?,
        "h" => {
            // Hours don't land on a calendar day; convert via datetime.
            let dt = Local::now() + chrono::Duration::hours(n);
            return Ok(dt.format(DATE_FMT).to_string());
        }
        _ => return Err(err()),
    };
    Ok(d.format(DATE_FMT).to_string())
}

/// Add `n` calendar months, clamping to the last day of the target month
/// (e.g. Jan 31 + 1mo -> Feb 28/29), matching Go's `AddDate` behavior.
fn add_months(d: NaiveDate, n: i64) -> Option<NaiveDate> {
    let total = d.month0() as i64 + n;
    let year = d.year() + total.div_euclid(12) as i32;
    let month = (total.rem_euclid(12) + 1) as u32;
    let last = last_day_of_month(year, month);
    NaiveDate::from_ymd_opt(year, month, d.day().min(last))
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (y, m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
        .day0()
        + 1
}

fn today_local() -> NaiveDate {
    Local::now().date_naive()
}

fn parse_due(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, DATE_FMT).ok()
}

/// True when the due date is before today and the task isn't terminal.
pub fn is_overdue(due_date: Option<&str>, done: bool) -> bool {
    if done {
        return false;
    }
    let Some(due) = due_date.and_then(parse_due) else {
        return false;
    };
    due < today_local()
}

/// Days until due (`Some(0)` = today). `None` when no date, done, or overdue.
pub fn days_until_due(due_date: Option<&str>, done: bool) -> Option<i64> {
    if done {
        return None;
    }
    let due = due_date.and_then(parse_due)?;
    let diff = (due - today_local()).num_days();
    (diff >= 0).then_some(diff)
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

pub fn now_rfc3339_nano() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_dates() {
        assert_eq!(
            parse_due_date("2026-01-15").unwrap(),
            Some("2026-01-15".into())
        );
        assert_eq!(parse_due_date("-").unwrap(), None);
        assert_eq!(parse_due_date("").unwrap(), None);
        assert!(parse_due_date("2026-02-31").is_err());
        assert!(parse_due_date("tomorrow").is_err());
    }

    #[test]
    fn relative_dates_land_on_expected_days() {
        let today = today_local();
        let plus7 = parse_due_date("+7d").unwrap().unwrap();
        let expect = today.checked_add_days(chrono::Days::new(7)).unwrap();
        assert_eq!(plus7, expect.format(DATE_FMT).to_string());

        let plus2w = parse_due_date("+2w").unwrap().unwrap();
        let expect = today.checked_add_days(chrono::Days::new(14)).unwrap();
        assert_eq!(plus2w, expect.format(DATE_FMT).to_string());

        assert!(parse_due_date("+x").is_err());
        assert!(parse_due_date("+5y").is_err());
        assert!(parse_due_date("+-1d").is_err());
    }

    #[test]
    fn overdue_and_countdown_use_calendar_days() {
        assert!(is_overdue(Some("2000-01-01"), false));
        assert!(!is_overdue(Some("2000-01-01"), true));
        assert!(!is_overdue(None, false));
        assert!(!is_overdue(Some("2999-01-01"), false));
        assert_eq!(days_until_due(Some("2000-01-01"), false), None);
        assert_eq!(days_until_due(None, false), None);
        let today = today_local().format(DATE_FMT).to_string();
        assert_eq!(days_until_due(Some(&today), false), Some(0));
    }

    #[test]
    fn nano_timestamps_format() {
        // Regression test: Go wrote RFC3339Nano but only parsed RFC3339,
        // so these rendered as "?" — must work here.
        let ts = "2026-01-10T12:34:56.123456789Z";
        assert_ne!(format_relative(ts), "?");
        assert_ne!(format_date(ts), ts);
        assert_eq!(format_relative("not-a-date"), "?");
        assert_eq!(format_date("not-a-date"), "not-a-date");
    }
}
