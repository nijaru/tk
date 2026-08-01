package task

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func setupTestDir(t *testing.T) string {
	tmp, err := os.MkdirTemp("", "tk-test-*")
	require.NoError(t, err)

	err = os.Mkdir(filepath.Join(tmp, ".tasks"), 0o755)
	require.NoError(t, err)

	SetWorkingDir(tmp)
	t.Cleanup(func() { workingDir = "" })
	return tmp
}

func TestCreateTaskHonorsExplicitNonePriority(t *testing.T) {
	setupTestDir(t)

	none := PriorityNone
	created, err := CreateTask(CreateTaskOptions{
		Title:    "No priority",
		Project:  "proj",
		Priority: &none,
	})
	require.NoError(t, err)
	assert.Equal(t, PriorityNone, created.Priority)

	_, err = os.Stat(filepath.Join(GetTasksDir(), "config.json"))
	assert.NoError(t, err, "creating the first task should initialize config.json")
}

func TestCreateTaskRejectsInvalidInput(t *testing.T) {
	setupTestDir(t)

	_, err := CreateTask(CreateTaskOptions{Title: "   ", Project: "proj"})
	assert.EqualError(t, err, "task title cannot be empty")

	_, err = CreateTask(CreateTaskOptions{Title: "Task", Project: "Bad Name"})
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "invalid project name")
}

func TestProjectNameValidation(t *testing.T) {
	for _, name := range []string{"tk", "tk-go", "project2", "my-project-2"} {
		assert.NoError(t, ValidateProjectName(name), name)
	}
	for _, name := range []string{"", "Bad", "bad_name", "bad name", "-bad", "bad-"} {
		assert.Error(t, ValidateProjectName(name), name)
	}
}

func TestParseStatus(t *testing.T) {
	status, err := ParseStatus("DONE")
	assert.NoError(t, err)
	assert.Equal(t, StatusDone, status)

	_, err = ParseStatus("bogus")
	assert.Error(t, err)
}

func TestIDResolution(t *testing.T) {
	tmp := setupTestDir(t)
	defer os.RemoveAll(tmp)

	// Create some tasks
	opts := CreateTaskOptions{Title: "Task 1", Project: "proj"}
	t1, err := CreateTask(opts)
	require.NoError(t, err)

	t.Run("Literal Match", func(t *testing.T) {
		id, err := ResolveID(t1.ID)
		assert.NoError(t, err)
		assert.Equal(t, t1.ID, id)
	})

	t.Run("Prefix Match (ref only)", func(t *testing.T) {
		// Refs are random 4-char. We can use the first 2 chars.
		id, err := ResolveID(t1.Ref[:2])
		assert.NoError(t, err)
		assert.Equal(t, t1.ID, id)
	})

	t.Run("Prefix Match (full ID)", func(t *testing.T) {
		id, err := ResolveID(t1.ID[:len(t1.ID)-1])
		assert.NoError(t, err)
		assert.Equal(t, t1.ID, id)
	})

	t.Run("Not found and reserved config", func(t *testing.T) {
		_, err := ResolveID("nonexistent")
		assert.Error(t, err)
		_, err = ResolveID("config")
		assert.Error(t, err)
	})
}

func TestAbsoluteDirectoryAlias(t *testing.T) {
	root := setupTestDir(t)
	external := t.TempDir()
	require.NoError(t, os.Mkdir(filepath.Join(external, ".tasks"), 0o755))

	config := GetConfig()
	config.Aliases = map[string]string{"external": external}
	require.NoError(t, SaveConfig(config))

	t.Chdir(root)
	require.NoError(t, SetWorkingDir("external"))
	assert.Equal(t, filepath.Join(external, ".tasks"), GetTasksDir())
}

func TestCorruptTaskFilesAreVisible(t *testing.T) {
	root := setupTestDir(t)
	badPath := filepath.Join(root, ".tasks", "proj-bad1.json")
	require.NoError(t, os.WriteFile(badPath, []byte("not json"), 0o644))

	_, err := GetAllTasks(filepath.Join(root, ".tasks"))
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "proj-bad1.json")

	issues, err := CheckIntegrity()
	require.NoError(t, err)
	require.Len(t, issues, 1)
	assert.Contains(t, issues[0], "Task file proj-bad1.json is invalid")
}

func TestReadTaskFileSupportsLegacyStringLogs(t *testing.T) {
	root := setupTestDir(t)
	defer os.RemoveAll(root)

	legacy := []byte(`{
  "project": "tk",
  "ref": "ixkz",
  "title": "Legacy task",
  "description": null,
  "status": "done",
  "priority": 3,
  "labels": [],
  "assignees": [],
  "parent": null,
  "blocked_by": [],
  "estimate": null,
  "due_date": null,
  "logs": [
    "2026-01-10: Researched embedding models",
    "A note without a timestamp"
  ],
  "created_at": "2026-01-10T09:04:32.061Z",
  "updated_at": "2026-01-10T19:25:00.000Z",
  "completed_at": "2026-01-10T19:25:00.000Z",
  "external": {}
}`)
	path := filepath.Join(root, ".tasks", "tk-ixkz.json")
	require.NoError(t, os.WriteFile(path, legacy, 0o644))

	parsed, err := ReadTaskFile(path)
	require.NoError(t, err)
	require.Len(t, parsed.Logs, 2)
	assert.Equal(t, "2026-01-10", parsed.Logs[0].Ts)
	assert.Equal(t, "Researched embedding models", parsed.Logs[0].Msg)
	assert.Empty(t, parsed.Logs[1].Ts)
	assert.Equal(t, "A note without a timestamp", parsed.Logs[1].Msg)

	// Loading the complete store, which is what list/ready use, must also
	// succeed when a legacy task is present.
	all, err := GetAllTasks(filepath.Join(root, ".tasks"))
	require.NoError(t, err)
	require.Len(t, all, 1)

	// A later write migrates the entries to the current structured schema.
	require.NoError(t, SaveTask(parsed))
	stored, err := os.ReadFile(path)
	require.NoError(t, err)
	var migrated struct {
		Logs []LogEntry `json:"logs"`
	}
	require.NoError(t, json.Unmarshal(stored, &migrated))
	require.Len(t, migrated.Logs, 2)
	assert.Equal(t, parsed.Logs, migrated.Logs)
}

func TestLegacyInvalidProjectTaskCanBeMovedAndUsedAsParent(t *testing.T) {
	root := setupTestDir(t)
	legacy := Task{
		Project:  "Bad Name!",
		Ref:      "abcd",
		Title:    "Legacy",
		Status:   StatusOpen,
		Priority: PriorityMedium,
		Labels:   []string{}, Assignees: []string{}, BlockedBy: []string{}, Logs: []LogEntry{},
		CreatedAt: "2026-01-01T00:00:00Z", UpdatedAt: "2026-01-01T00:00:00Z",
	}
	data, err := json.MarshalIndent(legacy, "", "  ")
	require.NoError(t, err)
	require.NoError(
		t,
		os.WriteFile(filepath.Join(root, ".tasks", "Bad Name!-abcd.json"), data, 0o644),
	)

	child, err := CreateTask(CreateTaskOptions{Title: "Child", Project: "proj"})
	require.NoError(t, err)
	assert.NoError(t, ValidateParent("Bad Name!-abcd", child.ID))

	result, err := MoveTask("Bad Name!-abcd", "proj")
	require.NoError(t, err)
	assert.Equal(t, "proj-abcd", result.NewID)
}

func TestClosedBlockerDoesNotBlockTask(t *testing.T) {
	root := setupTestDir(t)
	defer os.RemoveAll(root)

	blocker, err := CreateTask(CreateTaskOptions{Title: "Blocker", Project: "proj"})
	require.NoError(t, err)
	blocked, err := CreateTask(CreateTaskOptions{Title: "Blocked", Project: "proj"})
	require.NoError(t, err)

	_, err = UpdateTaskStatus(blocker.ID, StatusClosed)
	require.NoError(t, err)
	blockedTask, err := ReadTaskFile(filepath.Join(root, ".tasks", blocked.ID+".json"))
	require.NoError(t, err)
	blockedTask.BlockedBy = []string{blocker.ID}
	require.NoError(t, SaveTask(blockedTask))

	withMeta, _, err := GetTask(blocked.ID)
	require.NoError(t, err)
	assert.False(t, withMeta.BlockedByIncomplete)
}

func TestRepeatedStatusUpdateIsIdempotent(t *testing.T) {
	setupTestDir(t)
	created, err := CreateTask(CreateTaskOptions{Title: "Task", Project: "proj"})
	require.NoError(t, err)

	completed, err := UpdateTaskStatus(created.ID, StatusDone)
	require.NoError(t, err)
	recordedUpdated := completed.UpdatedAt
	recordedCompleted := completed.CompletedAt
	time.Sleep(2 * time.Millisecond)

	repeated, err := UpdateTaskStatus(created.ID, StatusDone)
	require.NoError(t, err)
	assert.Equal(t, recordedUpdated, repeated.UpdatedAt)
	assert.Equal(t, recordedCompleted, repeated.CompletedAt)
}

func TestCycleDetection(t *testing.T) {
	tmp := setupTestDir(t)
	defer os.RemoveAll(tmp)

	t1, _ := CreateTask(CreateTaskOptions{Title: "A", Project: "p"})
	t2, _ := CreateTask(CreateTaskOptions{Title: "B", Project: "p"})
	t3, _ := CreateTask(CreateTaskOptions{Title: "C", Project: "p"})

	t.Run("Block Cycle", func(t *testing.T) {
		// A blocks B, B blocks C, C blocks A (invalid)
		// We use WouldCreateBlockCycle(taskId, blockerId)
		// taskId is the one THAT WILL BE BLOCKED BY blockerId.

		// Set up A -> B -> C
		taskB, _, _ := GetTask(t2.ID)
		taskB.BlockedBy = append(taskB.BlockedBy, t1.ID)
		SaveTask(&taskB.Task)

		taskC, _, _ := GetTask(t3.ID)
		taskC.BlockedBy = append(taskC.BlockedBy, t2.ID)
		SaveTask(&taskC.Task)

		// Test: Does C blocking A create a cycle?
		// taskId=t1.ID (A), blockerId=t3.ID (C)
		assert.True(t, WouldCreateBlockCycle(t1.ID, t3.ID), "C blocking A should create cycle")
	})

	t.Run("Parent Cycle", func(t *testing.T) {
		// A is parent of B, B is parent of C, C is parent of A (invalid)

		taskB, _, _ := GetTask(t2.ID)
		taskB.Parent = &t1.ID
		SaveTask(&taskB.Task)

		taskC, _, _ := GetTask(t3.ID)
		taskC.Parent = &t2.ID
		SaveTask(&taskC.Task)

		assert.True(t, WouldCreateParentCycle(t1.ID, t3.ID), "C as parent of A should create cycle")
	})
}

func TestMoveAndRename(t *testing.T) {
	tmp := setupTestDir(t)
	defer os.RemoveAll(tmp)

	t1, _ := CreateTask(CreateTaskOptions{Title: "T1", Project: "p1"})
	t2, _ := CreateTask(CreateTaskOptions{Title: "T2", Project: "p1"})

	// t2 blocked by t1
	task2, _, _ := GetTask(t2.ID)
	task2.BlockedBy = []string{t1.ID}
	SaveTask(&task2.Task)

	t.Run("Rename Project", func(t *testing.T) {
		res, err := RenameProject("p1", "p2")
		require.NoError(t, err)
		assert.Equal(t, 2, len(res.Renamed))
		assert.Equal(t, 1, res.ReferencesUpdated)

		// Verify t1 is now p2-xxxx
		newID1 := TaskID("p2", t1.Ref)
		_, _, err = GetTask(newID1)
		assert.NoError(t, err)

		// Verify t2's blocker is updated
		newID2 := TaskID("p2", t2.Ref)
		task2New, _, _ := GetTask(newID2)
		assert.Equal(t, []string{newID1}, task2New.BlockedBy)
	})

	t.Run("Move Task", func(t *testing.T) {
		// Move t1 (now in p2) to p3
		oldID := TaskID("p2", t1.Ref)
		res, err := MoveTask(oldID, "p3")
		require.NoError(t, err)
		assert.Equal(t, oldID, res.OldID)
		assert.Equal(t, TaskID("p3", t1.Ref), res.NewID)
		assert.Equal(t, 1, res.ReferencesUpdated)

		// Check that t2 (in p2) now blocks on p3-ref
		t2ID := TaskID("p2", t2.Ref)
		task2, _, _ := GetTask(t2ID)
		assert.Equal(t, []string{res.NewID}, task2.BlockedBy)
	})
}
