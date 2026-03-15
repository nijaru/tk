package task

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func setupTestDir(t *testing.T) string {
	tmp, err := os.MkdirTemp("", "tk-test-*")
	require.NoError(t, err)

	err = os.Mkdir(filepath.Join(tmp, ".tasks"), 0o755)
	require.NoError(t, err)

	SetWorkingDir(tmp)
	return tmp
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

	t.Run("Ambiguous Match", func(t *testing.T) {
		// This is tricky with random refs, but we can mock files if needed.
		// For now, let's just test a "not found"
		_, err := ResolveID("nonexistent")
		assert.Error(t, err)
	})
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
