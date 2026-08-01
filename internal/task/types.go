package task

import (
	"bytes"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"time"
)

type Status string

const (
	StatusDeferred Status = "deferred"
	StatusOpen     Status = "open"
	StatusActive   Status = "active"
	StatusDone     Status = "done"
	StatusClosed   Status = "closed"
)

// ParseStatus parses a status filter, accepting case-insensitive names.
func ParseStatus(input string) (Status, error) {
	switch Status(strings.ToLower(strings.TrimSpace(input))) {
	case "":
		return "", nil
	case StatusDeferred, StatusOpen, StatusActive, StatusDone, StatusClosed:
		return Status(strings.ToLower(strings.TrimSpace(input))), nil
	default:
		return "", fmt.Errorf(
			"invalid status %q: use open, active, deferred, done, or closed",
			input,
		)
	}
}

type Priority int

const (
	PriorityNone   Priority = 0
	PriorityUrgent Priority = 1
	PriorityHigh   Priority = 2
	PriorityMedium Priority = 3
	PriorityLow    Priority = 4
)

var PriorityLabels = map[Priority]string{
	0: "none",
	1: "urgent",
	2: "high",
	3: "medium",
	4: "low",
}

var PriorityFromName = map[string]Priority{
	"none":   0,
	"urgent": 1,
	"high":   2,
	"medium": 3,
	"low":    4,
}

type LogEntry struct {
	Ts  string `json:"ts"`
	Msg string `json:"msg"`
}

// UnmarshalJSON accepts both the current structured log entries and the
// legacy string entries written by older tk versions. Legacy entries use a
// timestamp prefix such as "2026-01-10: message" when one is available.
func (l *LogEntry) UnmarshalJSON(data []byte) error {
	data = bytes.TrimSpace(data)
	if len(data) == 0 {
		return fmt.Errorf("log entry must be an object or string")
	}

	switch data[0] {
	case '{':
		type logEntry LogEntry
		var entry logEntry
		if err := json.Unmarshal(data, &entry); err != nil {
			return err
		}
		*l = LogEntry(entry)
		return nil
	case '"':
		var legacy string
		if err := json.Unmarshal(data, &legacy); err != nil {
			return err
		}
		*l = parseLegacyLog(legacy)
		return nil
	case 'n':
		if bytes.Equal(data, []byte("null")) {
			*l = LogEntry{}
			return nil
		}
	}

	return fmt.Errorf("log entry must be an object or string")
}

var legacyLogTimestampLayouts = [...]string{
	"2006-01-02",
	time.RFC3339Nano,
	time.RFC3339,
}

func isLegacyLogTimestamp(value string) bool {
	for _, layout := range legacyLogTimestampLayouts {
		if _, err := time.Parse(layout, value); err == nil {
			return true
		}
	}
	return false
}

func parseLegacyLog(value string) LogEntry {
	if isLegacyLogTimestamp(value) {
		return LogEntry{Ts: value}
	}

	// Find a colon that follows a complete timestamp. This handles both the
	// old date-only form and full RFC3339 timestamps without mistaking the
	// colons inside an RFC3339 time for the message separator.
	for i := 0; i < len(value); i++ {
		if value[i] != ':' || !isLegacyLogTimestamp(value[:i]) {
			continue
		}
		message := strings.TrimLeft(value[i+1:], " \t")
		return LogEntry{Ts: value[:i], Msg: message}
	}

	return LogEntry{Msg: value}
}

type ExternalLink struct {
	Number   *int    `json:"number,omitempty"`
	Repo     *string `json:"repo,omitempty"`
	SyncedAt *string `json:"synced_at,omitempty"`
}

type External struct {
	GitHub *ExternalLink `json:"github,omitempty"`
	Linear *ExternalLink `json:"linear,omitempty"`
	Jira   *ExternalLink `json:"jira,omitempty"`
}

type Task struct {
	Project     string     `json:"project"`
	Ref         string     `json:"ref"`
	Title       string     `json:"title"`
	Description *string    `json:"description"`
	Status      Status     `json:"status"`
	Priority    Priority   `json:"priority"`
	Labels      []string   `json:"labels"`
	Assignees   []string   `json:"assignees"`
	Parent      *string    `json:"parent"`
	BlockedBy   []string   `json:"blocked_by"`
	Estimate    *int       `json:"estimate"`
	DueDate     *string    `json:"due_date"`
	Logs        []LogEntry `json:"logs"`
	CreatedAt   string     `json:"created_at"`
	UpdatedAt   string     `json:"updated_at"`
	CompletedAt *string    `json:"completed_at"`
	External    External   `json:"external"`
}

type TaskWithMeta struct {
	Task
	ID                  string `json:"id"`
	BlockedByIncomplete bool   `json:"blocked_by_incomplete"`
	IsOverdue           bool   `json:"is_overdue"`
	DaysUntilDue        *int   `json:"days_until_due"`
}

// CleanAfter represents clean_after which can be a number of days or false (disabled).
type CleanAfter struct {
	Enabled bool
	Days    int
}

func (c CleanAfter) MarshalJSON() ([]byte, error) {
	if !c.Enabled {
		return []byte("false"), nil
	}
	return json.Marshal(c.Days)
}

func (c *CleanAfter) UnmarshalJSON(data []byte) error {
	if string(data) == "false" {
		c.Enabled = false
		c.Days = 0
		return nil
	}
	c.Enabled = true
	return json.Unmarshal(data, &c.Days)
}

type ConfigDefaults struct {
	Priority  Priority `json:"priority"`
	Labels    []string `json:"labels"`
	Assignees []string `json:"assignees"`
}

type Config struct {
	Version    int               `json:"version"`
	Project    string            `json:"project"`
	Defaults   ConfigDefaults    `json:"defaults"`
	CleanAfter CleanAfter        `json:"clean_after"`
	Aliases    map[string]string `json:"aliases,omitempty"`
}

var DefaultConfig = Config{
	Version: 1,
	Project: "tk",
	Defaults: ConfigDefaults{
		Priority:  PriorityMedium,
		Labels:    []string{},
		Assignees: []string{},
	},
	CleanAfter: CleanAfter{Enabled: true, Days: 14},
}

// TaskID returns the full task ID from project and ref.
func TaskID(project, ref string) string {
	var b strings.Builder
	b.Grow(len(project) + 1 + len(ref))
	b.WriteString(project)
	b.WriteByte('-')
	b.WriteString(ref)
	return b.String()
}

// ID returns the full task ID.
func (t *Task) ID() string {
	return TaskID(t.Project, t.Ref)
}

// projectPattern validates project names used in task IDs.
var projectPattern = regexp.MustCompile(`^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$`)

// idPattern validates the overall ID format. Uses strings.LastIndex to split
// project/ref so project names may contain hyphens.
var idPattern = regexp.MustCompile(`^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$`)

// ValidateProjectName validates a project name before it becomes part of a
// task filename or ID.
func ValidateProjectName(project string) error {
	if !projectPattern.MatchString(project) {
		return fmt.Errorf(
			"invalid project name %q: use lowercase letters, digits, and internal hyphens",
			project,
		)
	}
	return nil
}

// ParseID parses "project-ref" into its components.
// Project names may contain hyphens (e.g. "my-app-a7b3" → project="my-app", ref="a7b3").
func ParseID(id string) (project, ref string, ok bool) {
	s := strings.ToLower(id)
	if !idPattern.MatchString(s) {
		return "", "", false
	}
	dash := strings.LastIndex(s, "-")
	return s[:dash], s[dash+1:], true
}

const refChars = "abcdefghijklmnopqrstuvwxyz0123456789"

// GenerateRef generates a random 4-char alphanumeric ref without modulo bias.
func GenerateRef() (string, error) {
	const length = 4
	const charsetLen = byte(len(refChars))
	maxValid := charsetLen * (255 / charsetLen) // reject >= maxValid to avoid bias

	result := make([]byte, 0, length)
	buf := make([]byte, length*2)

	for len(result) < length {
		if _, err := rand.Read(buf); err != nil {
			return "", fmt.Errorf("generate ref: %w", err)
		}
		for _, b := range buf {
			if b < maxValid && len(result) < length {
				result = append(result, refChars[b%charsetLen])
			}
		}
	}
	return string(result), nil
}
