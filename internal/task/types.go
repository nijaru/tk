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

	unknownFields map[string]json.RawMessage
}

var logEntryJSONFields = map[string]struct{}{
	"ts": {}, "msg": {},
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
		entry := logEntry(*l)
		if err := json.Unmarshal(data, &entry); err != nil {
			return err
		}
		unknown, err := captureUnknownJSONFields(data, logEntryJSONFields)
		if err != nil {
			return err
		}
		entry.unknownFields = unknown
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

func (l LogEntry) MarshalJSON() ([]byte, error) {
	type logEntry LogEntry
	data, err := json.Marshal(logEntry(l))
	if err != nil {
		return nil, err
	}
	return mergeUnknownJSONFields(data, l.unknownFields)
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

// captureUnknownJSONFields retains fields introduced by a newer writer so a
// read-modify-write cycle does not erase data this version does not understand.
func captureUnknownJSONFields(
	data []byte,
	known map[string]struct{},
) (map[string]json.RawMessage, error) {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	for key := range known {
		delete(fields, key)
	}
	if len(fields) == 0 {
		return nil, nil
	}
	return fields, nil
}

func mergeUnknownJSONFields(data []byte, unknown map[string]json.RawMessage) ([]byte, error) {
	if len(unknown) == 0 {
		return data, nil
	}

	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	for key, value := range unknown {
		if _, exists := fields[key]; !exists {
			fields[key] = value
		}
	}
	return json.MarshalIndent(fields, "", "  ")
}

type ExternalLink struct {
	Number   *int    `json:"number,omitempty"`
	Repo     *string `json:"repo,omitempty"`
	SyncedAt *string `json:"synced_at,omitempty"`

	unknownFields map[string]json.RawMessage
}

var externalLinkJSONFields = map[string]struct{}{
	"number": {}, "repo": {}, "synced_at": {},
}

func (l *ExternalLink) UnmarshalJSON(data []byte) error {
	type externalLink ExternalLink
	decoded := externalLink(*l)
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	unknown, err := captureUnknownJSONFields(data, externalLinkJSONFields)
	if err != nil {
		return err
	}
	decoded.unknownFields = unknown
	*l = ExternalLink(decoded)
	return nil
}

func (l ExternalLink) MarshalJSON() ([]byte, error) {
	type externalLink ExternalLink
	data, err := json.Marshal(externalLink(l))
	if err != nil {
		return nil, err
	}
	return mergeUnknownJSONFields(data, l.unknownFields)
}

type External struct {
	GitHub *ExternalLink `json:"github,omitempty"`
	Linear *ExternalLink `json:"linear,omitempty"`
	Jira   *ExternalLink `json:"jira,omitempty"`

	unknownFields map[string]json.RawMessage
}

var externalJSONFields = map[string]struct{}{
	"github": {}, "linear": {}, "jira": {},
}

func (e *External) UnmarshalJSON(data []byte) error {
	type external External
	decoded := external(*e)
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	unknown, err := captureUnknownJSONFields(data, externalJSONFields)
	if err != nil {
		return err
	}
	decoded.unknownFields = unknown
	*e = External(decoded)
	return nil
}

func (e External) MarshalJSON() ([]byte, error) {
	type external External
	data, err := json.Marshal(external(e))
	if err != nil {
		return nil, err
	}
	return mergeUnknownJSONFields(data, e.unknownFields)
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

	unknownFields        map[string]json.RawMessage
	statusNeedsMigration bool
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
	data = bytes.TrimSpace(data)
	if bytes.Equal(data, []byte("false")) {
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

	unknownFields map[string]json.RawMessage
}

type Config struct {
	Version    int               `json:"version"`
	Project    string            `json:"project"`
	Defaults   ConfigDefaults    `json:"defaults"`
	CleanAfter CleanAfter        `json:"clean_after"`
	Aliases    map[string]string `json:"aliases,omitempty"`

	unknownFields map[string]json.RawMessage
}

var taskJSONFields = map[string]struct{}{
	"project": {}, "ref": {}, "title": {}, "description": {}, "status": {},
	"priority": {}, "labels": {}, "assignees": {}, "parent": {}, "blocked_by": {},
	"estimate": {}, "due_date": {}, "logs": {}, "created_at": {}, "updated_at": {},
	"completed_at": {}, "external": {},
}

var configDefaultsJSONFields = map[string]struct{}{
	"priority": {}, "labels": {}, "assignees": {},
}

var configJSONFields = map[string]struct{}{
	"version": {}, "project": {}, "defaults": {}, "clean_after": {}, "aliases": {},
}

func (d *ConfigDefaults) UnmarshalJSON(data []byte) error {
	type configDefaults ConfigDefaults
	decoded := configDefaults(*d)
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	*d = ConfigDefaults(decoded)

	unknown, err := captureUnknownJSONFields(data, configDefaultsJSONFields)
	if err != nil {
		return err
	}
	d.unknownFields = unknown
	return nil
}

func (d ConfigDefaults) MarshalJSON() ([]byte, error) {
	type configDefaults ConfigDefaults
	data, err := json.Marshal(configDefaults(d))
	if err != nil {
		return nil, err
	}
	return mergeUnknownJSONFields(data, d.unknownFields)
}

func (c *Config) UnmarshalJSON(data []byte) error {
	type config Config
	decoded := config(*c)
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	*c = Config(decoded)

	unknown, err := captureUnknownJSONFields(data, configJSONFields)
	if err != nil {
		return err
	}
	c.unknownFields = unknown
	return nil
}

func (c Config) MarshalJSON() ([]byte, error) {
	type config Config
	data, err := json.Marshal(config(c))
	if err != nil {
		return nil, err
	}
	return mergeUnknownJSONFields(data, c.unknownFields)
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

func normalizeTaskStatus(status Status) Status {
	normalized := strings.ToLower(strings.TrimSpace(string(status)))
	switch normalized {
	case string(StatusDeferred),
		string(StatusOpen),
		string(StatusActive),
		string(StatusDone),
		string(StatusClosed):
		return Status(normalized)
	case "cancelled", "canceled":
		// Older tk versions used cancelled/canceled for the terminal state now
		// named closed. Keep completed tasks terminal after loading them.
		return StatusClosed
	default:
		return status
	}
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
