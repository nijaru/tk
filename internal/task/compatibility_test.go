package task

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestLegacyCancelledTaskAndUnknownFieldsSurviveSave(t *testing.T) {
	root := setupTestDir(t)
	defer os.RemoveAll(root)

	path := filepath.Join(root, ".tasks", "tk-ixkz.json")
	legacy := []byte(`{
  "project": "tk",
  "ref": "ixkz",
  "title": "Legacy task",
  "description": null,
  "status": "cancelled",
  "priority": 3,
  "labels": null,
  "assignees": null,
  "parent": null,
  "blocked_by": [],
  "estimate": null,
  "due_date": null,
  "logs": [
    "2026-01-10: legacy note",
    {"ts": "2026-01-10T10:00:00Z", "msg": "structured note", "future_log": {"keep": true}}
  ],
  "created_at": "2026-01-10T09:04:32.061Z",
  "updated_at": "2026-01-10T19:25:00.000Z",
  "completed_at": "2026-01-10T19:25:00.000Z",
  "external": {
    "github": {"number": 4, "future_link": {"keep": true}},
    "future_external": {"keep": true}
  },
  "future_field": {"keep": true}
}`)
	require.NoError(t, os.WriteFile(path, legacy, 0o644))

	parsed, err := ReadTaskFile(path)
	require.NoError(t, err)
	assert.Equal(t, StatusClosed, parsed.Status)
	require.Len(t, parsed.Logs, 2)
	assert.Equal(t, LogEntry{Ts: "2026-01-10", Msg: "legacy note"}, parsed.Logs[0])

	updated, err := UpdateTaskStatus(parsed.ID(), StatusClosed)
	require.NoError(t, err)
	assert.Equal(t, StatusClosed, updated.Status)

	stored, err := os.ReadFile(path)
	require.NoError(t, err)
	var fields map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(stored, &fields))
	var status Status
	require.NoError(t, json.Unmarshal(fields["status"], &status))
	assert.Equal(t, StatusClosed, status)
	assert.JSONEq(t, `{"keep":true}`, string(fields["future_field"]))

	var logs []json.RawMessage
	require.NoError(t, json.Unmarshal(fields["logs"], &logs))
	var structuredLog map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(logs[1], &structuredLog))
	assert.JSONEq(t, `{"keep":true}`, string(structuredLog["future_log"]))

	var external map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(fields["external"], &external))
	assert.JSONEq(t, `{"keep":true}`, string(external["future_external"]))
	var github map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(external["github"], &github))
	assert.JSONEq(t, `{"keep":true}`, string(github["future_link"]))
}

func TestConfigUpdatePreservesUnknownFields(t *testing.T) {
	root := setupTestDir(t)
	defer os.RemoveAll(root)

	path := filepath.Join(root, ".tasks", "config.json")
	legacy := []byte(`{
  "version": 1,
  "project": "legacy",
  "defaults": {
    "priority": 3,
    "labels": [],
    "assignees": [],
    "future_default": {"keep": true}
  },
  "clean_after": 14,
  "aliases": {},
  "auto_project": true,
  "counters": {"next": 7}
}`)
	require.NoError(t, os.WriteFile(path, legacy, 0o644))

	updated, err := UpdateConfig(func(config *Config) {
		config.Project = "current"
	})
	require.NoError(t, err)
	assert.Equal(t, "current", updated.Project)

	stored, err := os.ReadFile(path)
	require.NoError(t, err)
	var fields map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(stored, &fields))
	assert.JSONEq(t, `true`, string(fields["auto_project"]))
	assert.JSONEq(t, `{"next":7}`, string(fields["counters"]))

	var defaults map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(fields["defaults"], &defaults))
	assert.JSONEq(t, `{"keep":true}`, string(defaults["future_default"]))
}

func TestMalformedConfigIsNotReplacedByDefaults(t *testing.T) {
	root := setupTestDir(t)
	defer os.RemoveAll(root)

	path := filepath.Join(root, ".tasks", "config.json")
	before := []byte(`{"version":1,"project":"preserve","defaults":{"priority":"3"}}`)
	require.NoError(t, os.WriteFile(path, before, 0o644))

	_, err := UpdateConfig(func(config *Config) {
		config.Project = "should-not-write"
	})
	require.Error(t, err)
	assert.Contains(t, err.Error(), "parse config")

	after, err := os.ReadFile(path)
	require.NoError(t, err)
	assert.Equal(t, before, after)

	_, err = CreateTask(CreateTaskOptions{Title: "blocked", Project: "proj"})
	require.Error(t, err)
	afterCreate, err := os.ReadFile(path)
	require.NoError(t, err)
	assert.Equal(t, before, afterCreate)
}
