package timeutil

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

const DueSoonThreshold = 7

// IsOverdue returns true if the due date has passed and the task is not done.
func IsOverdue(dueDate *string, done bool) bool {
	if dueDate == nil || done {
		return false
	}
	due, err := parseDate(*dueDate)
	if err != nil {
		return false
	}
	today := today()
	return due.Before(today)
}

// DaysUntilDue returns the number of days until due (0 = today), or nil if
// no due date, done, or already overdue.
func DaysUntilDue(dueDate *string, done bool) *int {
	if dueDate == nil || done {
		return nil
	}
	due, err := parseDate(*dueDate)
	if err != nil {
		return nil
	}
	t := today()
	diff := due.Sub(t)
	if diff < 0 {
		return nil
	}
	days := int(diff.Hours() / 24)
	return &days
}

// FormatRelative formats a timestamp as a short relative string like "2d", "5h".
func FormatRelative(ts string) string {
	t, err := time.Parse(time.RFC3339, ts)
	if err != nil {
		return "?"
	}
	diff := time.Since(t)
	switch {
	case diff >= 24*time.Hour:
		return fmt.Sprintf("%dd", int(diff.Hours()/24))
	case diff >= time.Hour:
		return fmt.Sprintf("%dh", int(diff.Hours()))
	case diff >= time.Minute:
		return fmt.Sprintf("%dm", int(diff.Minutes()))
	default:
		return "now"
	}
}

// FormatDate formats a timestamp as a localized date string.
func FormatDate(ts string) string {
	t, err := time.Parse(time.RFC3339, ts)
	if err != nil {
		return ts
	}
	return t.Local().Format("2006-01-02 15:04")
}

// FormatLocalDate formats a date as YYYY-MM-DD.
func FormatLocalDate(t time.Time) string {
	return t.Format("2006-01-02")
}

// ParseDueDate parses YYYY-MM-DD or relative formats like +7d, +2w, +1m.
// Returns "" and nil error for "-" (clear signal).
func ParseDueDate(input string) (string, error) {
	if input == "" || input == "-" {
		return "", nil
	}

	if strings.HasPrefix(input, "+") {
		return parseRelativeDate(input)
	}

	// Validate YYYY-MM-DD
	t, err := time.Parse("2006-01-02", input)
	if err != nil {
		return "", fmt.Errorf("invalid date %q: use YYYY-MM-DD or relative like +7d", input)
	}
	// Re-format to normalize (reject things like 2026-02-31)
	if t.Format("2006-01-02") != input {
		return "", fmt.Errorf("invalid date %q", input)
	}
	return input, nil
}

func parseRelativeDate(input string) (string, error) {
	s := input[1:]
	if len(s) < 2 {
		return "", fmt.Errorf("invalid relative date %q: use +Nd/+Nw/+Nm/+Nh", input)
	}

	unit := s[len(s)-1]
	n, err := strconv.Atoi(s[:len(s)-1])
	if err != nil || n < 0 {
		return "", fmt.Errorf("invalid relative date %q: use +Nd/+Nw/+Nm/+Nh", input)
	}

	now := time.Now()
	switch unit {
	case 'h':
		now = now.Add(time.Duration(n) * time.Hour)
	case 'd':
		now = now.AddDate(0, 0, n)
	case 'w':
		now = now.AddDate(0, 0, n*7)
	case 'm':
		now = now.AddDate(0, n, 0)
	default:
		return "", fmt.Errorf("invalid relative date unit %q: use d/w/m/h", string(unit))
	}

	return FormatLocalDate(now), nil
}

func today() time.Time {
	y, m, d := time.Now().Date()
	return time.Date(y, m, d, 0, 0, 0, 0, time.Local)
}

func parseDate(s string) (time.Time, error) {
	return time.ParseInLocation("2006-01-02", s, time.Local)
}
