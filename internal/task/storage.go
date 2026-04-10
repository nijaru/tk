package task

import (
	"cmp"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/nijaru/tk/internal/timeutil"
)

// --- Path Utilities ---

func ensureTasksDir() (string, error) {
	root := FindRoot()
	if _, err := os.Stat(root.TasksDir); os.IsNotExist(err) {
		if err := os.MkdirAll(root.TasksDir, 0o755); err != nil {
			return "", fmt.Errorf("create tasks dir: %w", err)
		}
	}
	return root.TasksDir, nil
}

func getTaskPath(tasksDir, id string) string {
	return filepath.Join(tasksDir, fmt.Sprintf("%s.json", id))
}

func getConfigPath(tasksDir string) string {
	return filepath.Join(tasksDir, "config.json")
}

/**
 * Write a file atomically by writing to a temporary file and renaming it.
 */
func atomicWrite(path string, content []byte) error {
	ref, err := GenerateRef()
	if err != nil {
		return err
	}
	tempPath := fmt.Sprintf("%s.tmp.%s", path, ref)

	f, err := os.OpenFile(tempPath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o644)
	if err != nil {
		return fmt.Errorf("open temp file: %w", err)
	}

	_, err = f.Write(content)
	if err != nil {
		f.Close()
		_ = os.Remove(tempPath)
		return fmt.Errorf("write temp file: %w", err)
	}

	if err := f.Sync(); err != nil {
		f.Close()
		_ = os.Remove(tempPath)
		return fmt.Errorf("sync temp file: %w", err)
	}

	if err := f.Close(); err != nil {
		_ = os.Remove(tempPath)
		return fmt.Errorf("close temp file: %w", err)
	}

	if err := os.Rename(tempPath, path); err != nil {
		_ = os.Remove(tempPath)
		return fmt.Errorf("rename to sync: %w", err)
	}

	return nil
}

// --- Sorting ---

var statusOrder = map[Status]int{
	StatusActive:   0,
	StatusOpen:     1,
	StatusDeferred: 2,
	StatusDone:     3,
	StatusClosed:   4,
}

// IsTerminalStatus returns true if the status is done or closed.
func IsTerminalStatus(s Status) bool {
	return s == StatusDone || s == StatusClosed
}

func compareTasks(a, b *Task) int {
	sa := statusOrder[a.Status]
	sb := statusOrder[b.Status]
	if sa != sb {
		return cmp.Compare(sa, sb)
	}

	if !IsTerminalStatus(a.Status) {
		// Overdue hoist
		oa := 1
		if timeutil.IsOverdue(a.DueDate, a.Status == StatusDone) {
			oa = 0
		}
		ob := 1
		if timeutil.IsOverdue(b.DueDate, IsTerminalStatus(b.Status)) {
			ob = 0
		}
		if oa != ob {
			return cmp.Compare(oa, ob)
		}

		// Priority (1-4, then 0/none)
		pa := a.Priority
		if pa == PriorityNone {
			pa = 5
		}
		pb := b.Priority
		if pb == PriorityNone {
			pb = 5
		}
		if pa != pb {
			return cmp.Compare(pa, pb)
		}

		// Due date (soonest first, nulls last)
		da := a.DueDate
		db := b.DueDate
		if da != nil && db != nil {
			if *da != *db {
				return cmp.Compare(*da, *db)
			}
		} else if da != nil {
			return -1
		} else if db != nil {
			return 1
		}

		// Created at (newest first)
		return cmp.Compare(b.CreatedAt, a.CreatedAt)
	}

	// Done tasks: newest completion first
	ca := ""
	if a.CompletedAt != nil {
		ca = *a.CompletedAt
	}
	cb := ""
	if b.CompletedAt != nil {
		cb = *b.CompletedAt
	}
	return cmp.Compare(cb, ca)
}

// --- Config Operations ---

func GetConfig() Config {
	root := FindRoot()
	configPath := getConfigPath(root.TasksDir)

	data, err := os.ReadFile(configPath)
	if err != nil {
		return DefaultConfig
	}

	config := DefaultConfig
	if err := json.Unmarshal(data, &config); err != nil {
		return DefaultConfig
	}

	return config
}

func SaveConfig(config Config) error {
	tasksDir, err := ensureTasksDir()
	if err != nil {
		return err
	}

	data, err := json.MarshalIndent(config, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal config: %w", err)
	}

	return atomicWrite(getConfigPath(tasksDir), data)
}

func UpdateConfig(updates func(*Config)) (Config, error) {
	config := GetConfig()
	updates(&config)
	if err := SaveConfig(config); err != nil {
		return config, err
	}
	return config, nil
}

// --- Task CRUD ---

type CreateTaskOptions struct {
	Title       string
	Description *string
	Priority    Priority
	Project     string
	Labels      []string
	Assignees   []string
	Parent      *string
	Estimate    *int
	DueDate     *string
}

func CreateTask(options CreateTaskOptions) (*TaskWithMeta, error) {
	tasksDir, err := ensureTasksDir()
	if err != nil {
		return nil, err
	}

	config := GetConfig()
	project := options.Project
	if project == "" {
		project = config.Project
	}

	now := time.Now().UTC().Format(time.RFC3339Nano)

	// Generate random ref with collision detection
	const maxRetries = 10
	for attempt := 0; attempt < maxRetries; attempt++ {
		ref, err := GenerateRef()
		if err != nil {
			return nil, err
		}
		id := TaskID(project, ref)

		priority := options.Priority
		if priority == 0 {
			priority = config.Defaults.Priority
		}

		labels := options.Labels
		if labels == nil {
			labels = append([]string(nil), config.Defaults.Labels...)
		}

		assignees := options.Assignees
		if assignees == nil {
			assignees = append([]string(nil), config.Defaults.Assignees...)
		}

		task := Task{
			Project:     project,
			Ref:         ref,
			Title:       options.Title,
			Description: options.Description,
			Status:      StatusOpen,
			Priority:    priority,
			Labels:      labels,
			Assignees:   assignees,
			Parent:      options.Parent,
			BlockedBy:   []string{},
			Estimate:    options.Estimate,
			DueDate:     options.DueDate,
			Logs:        []LogEntry{},
			CreatedAt:   now,
			UpdatedAt:   now,
			CompletedAt: nil,
			External:    External{},
		}

		path := getTaskPath(tasksDir, id)

		err = writeTaskFileExclusive(path, &task)
		if err == nil {
			return EnrichTask(&task, nil), nil
		}
		if !errors.Is(err, os.ErrExist) {
			return nil, err
		}
		// Collision - retry
	}

	return nil, fmt.Errorf("failed to create task after %d attempts", maxRetries)
}

func writeTaskFileExclusive(path string, task *Task) error {
	data, err := json.MarshalIndent(task, "", "  ")
	if err != nil {
		return err
	}

	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if err != nil {
		return err
	}

	if _, err = f.Write(data); err != nil {
		f.Close()
		_ = os.Remove(path)
		return err
	}

	return f.Close()
}

func GetTask(id string) (*TaskWithMeta, *CleanupInfo, error) {
	root := FindRoot()
	if !root.Exists {
		return nil, nil, &ErrTasksNotFound{SearchedFrom: GetWorkingDir()}
	}

	task, err := ReadTaskFile(getTaskPath(root.TasksDir, id))
	if err != nil {
		return nil, nil, err
	}

	cleanup := CleanTaskOrphans(task, id, root.TasksDir)

	// Pre-build statusMap to avoid N+1 reads in EnrichTask.
	statusMap := make(map[string]Status, len(task.BlockedBy))
	for _, blockerID := range task.BlockedBy {
		if b, err := ReadTaskFile(getTaskPath(root.TasksDir, blockerID)); err == nil {
			statusMap[blockerID] = b.Status
		}
	}

	return EnrichTask(task, statusMap), cleanup, nil
}

func ReadTaskFile(path string) (*Task, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	var task Task
	if err := json.Unmarshal(data, &task); err != nil {
		return nil, fmt.Errorf("parse task: %w", err)
	}

	return &task, nil
}

func SaveTask(task *Task) error {
	root := FindRoot()
	if !root.Exists {
		return &ErrTasksNotFound{SearchedFrom: GetWorkingDir()}
	}

	data, err := json.MarshalIndent(task, "", "  ")
	if err != nil {
		return err
	}

	return atomicWrite(getTaskPath(root.TasksDir, task.ID()), data)
}

func EnrichTask(task *Task, statusMap map[string]Status) *TaskWithMeta {
	root := FindRoot()
	blockedByIncomplete := false

	for _, blockerID := range task.BlockedBy {
		status, ok := statusMap[blockerID]
		if !ok {
			t, err := ReadTaskFile(getTaskPath(root.TasksDir, blockerID))
			if err == nil {
				status = t.Status
			}
		}

		if status != "" && status != StatusDone {
			blockedByIncomplete = true
			break
		}
	}

	return &TaskWithMeta{
		Task:                *task,
		ID:                  task.ID(),
		BlockedByIncomplete: blockedByIncomplete,
		IsOverdue:           timeutil.IsOverdue(task.DueDate, IsTerminalStatus(task.Status)),
		DaysUntilDue:        timeutil.DaysUntilDue(task.DueDate, IsTerminalStatus(task.Status)),
	}
}

// --- List Operations ---

type ListOptions struct {
	Search       string
	HideTerminal bool
	Status       Status
	Priority *Priority
	Project  string
	Label    string
	Assignee string
	Parent   *string // nil means no filter, non-nil means filter by parent ID (empty string for roots)
	Roots    bool
	Overdue  bool
	Limit    int
}

func ListTasks(options ListOptions) ([]*TaskWithMeta, error) {
	root := FindRoot()
	if !root.Exists {
		return nil, nil
	}

	allTasks, err := GetAllTasks(root.TasksDir)
	if err != nil {
		return nil, err
	}

	statusMap := make(map[string]Status)
	for _, t := range allTasks {
		statusMap[t.ID()] = t.Status
	}

	var filtered []*Task
	for _, t := range allTasks {
		if options.Status != "" && t.Status != options.Status {
			continue
		}
		if options.HideTerminal && IsTerminalStatus(t.Status) {
			continue
		}
		if options.Search != "" {
			query := strings.ToLower(options.Search)
			titleMatch := strings.Contains(strings.ToLower(t.Title), query)
			descMatch := t.Description != nil && strings.Contains(strings.ToLower(*t.Description), query)
			idMatch := strings.Contains(strings.ToLower(t.ID()), query)
			if !titleMatch && !descMatch && !idMatch {
				continue
			}
		}
		if options.Priority != nil && t.Priority != *options.Priority {
			continue
		}
		if options.Project != "" && t.Project != options.Project {
			continue
		}
		if options.Label != "" {
			found := false
			for _, l := range t.Labels {
				if l == options.Label {
					found = true
					break
				}
			}
			if !found {
				continue
			}
		}
		if options.Assignee != "" {
			found := false
			for _, a := range t.Assignees {
				if a == options.Assignee {
					found = true
					break
				}
			}
			if !found {
				continue
			}
		}
		if options.Roots && t.Parent != nil {
			continue
		}
		if options.Parent != nil {
			if t.Parent == nil {
				if *options.Parent != "" {
					continue
				}
			} else if *t.Parent != *options.Parent {
				continue
			}
		}
		if options.Overdue && !timeutil.IsOverdue(t.DueDate, IsTerminalStatus(t.Status)) {
			continue
		}

		filtered = append(filtered, t)
	}

	slices.SortFunc(filtered, compareTasks)

	if options.Limit > 0 && len(filtered) > options.Limit {
		filtered = filtered[:options.Limit]
	}

	results := make([]*TaskWithMeta, len(filtered))
	for i, t := range filtered {
		results[i] = EnrichTask(t, statusMap)
	}

	return results, nil
}

func GetAllTasks(tasksDir string) ([]*Task, error) {
	entries, err := os.ReadDir(tasksDir)
	if err != nil {
		return nil, err
	}

	var tasks []*Task
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") ||
			entry.Name() == "config.json" {
			continue
		}

		task, err := ReadTaskFile(filepath.Join(tasksDir, entry.Name()))
		if err == nil {
			tasks = append(tasks, task)
		}
	}
	return tasks, nil
}

// --- Task Updates ---

func UpdateTaskStatus(id string, status Status) (*TaskWithMeta, error) {
	root := FindRoot()
	path := getTaskPath(root.TasksDir, id)
	task, err := ReadTaskFile(path)
	if err != nil {
		return nil, err
	}

	now := time.Now().UTC().Format(time.RFC3339Nano)
	task.Status = status
	task.UpdatedAt = now
	if status == StatusDone {
		task.CompletedAt = &now
	} else {
		task.CompletedAt = nil
	}

	if err := SaveTask(task); err != nil {
		return nil, err
	}

	return EnrichTask(task, nil), nil
}

type UpdateTaskOptions struct {
	Title       *string
	Description *string
	ClearDesc   bool
	Priority    *Priority
	Labels      []string
	Assignees   []string
	Parent      *string
	ClearParent bool
	Estimate    *int
	ClearEst    bool
	DueDate     *string
	ClearDue    bool
}

func UpdateTask(id string, updates UpdateTaskOptions) (*TaskWithMeta, error) {
	root := FindRoot()
	path := getTaskPath(root.TasksDir, id)
	task, err := ReadTaskFile(path)
	if err != nil {
		return nil, err
	}

	now := time.Now().UTC().Format(time.RFC3339Nano)
	modified := false

	if updates.Title != nil {
		task.Title = *updates.Title
		modified = true
	}
	if updates.ClearDesc {
		task.Description = nil
		modified = true
	} else if updates.Description != nil {
		task.Description = updates.Description
		modified = true
	}
	if updates.Priority != nil {
		task.Priority = *updates.Priority
		modified = true
	}
	if updates.Labels != nil {
		task.Labels = updates.Labels
		modified = true
	}
	if updates.Assignees != nil {
		task.Assignees = updates.Assignees
		modified = true
	}
	if updates.ClearParent {
		task.Parent = nil
		modified = true
	} else if updates.Parent != nil {
		task.Parent = updates.Parent
		modified = true
	}
	if updates.ClearEst {
		task.Estimate = nil
		modified = true
	} else if updates.Estimate != nil {
		task.Estimate = updates.Estimate
		modified = true
	}
	if updates.ClearDue {
		task.DueDate = nil
		modified = true
	} else if updates.DueDate != nil {
		task.DueDate = updates.DueDate
		modified = true
	}

	if modified {
		task.UpdatedAt = now
		if err := SaveTask(task); err != nil {
			return nil, err
		}
	}

	return EnrichTask(task, nil), nil
}

func DeleteTask(id string) error {
	root := FindRoot()
	path := getTaskPath(root.TasksDir, id)

	if err := os.Remove(path); err != nil {
		return err
	}

	allTasks, err := GetAllTasks(root.TasksDir)
	if err != nil {
		return nil // Non-critical
	}

	for _, t := range allTasks {
		modified := false

		newBlockedBy := make([]string, 0, len(t.BlockedBy))
		for _, b := range t.BlockedBy {
			if b != id {
				newBlockedBy = append(newBlockedBy, b)
			} else {
				modified = true
			}
		}
		t.BlockedBy = newBlockedBy

		if t.Parent != nil && *t.Parent == id {
			t.Parent = nil
			modified = true
		}

		if modified {
			_ = SaveTask(t)
		}
	}

	return nil
}

// --- ID Resolution ---

func ResolveID(input string) (string, error) {
	root := FindRoot()
	if !root.Exists {
		return "", &ErrTasksNotFound{SearchedFrom: GetWorkingDir()}
	}

	// 1. Literal match
	if _, err := os.Stat(getTaskPath(root.TasksDir, input)); err == nil {
		return input, nil
	}

	// 2. Prefix matching
	entries, err := os.ReadDir(root.TasksDir)
	if err != nil {
		return "", err
	}

	var matches []string
	lowerInput := strings.ToLower(input)

	for _, entry := range entries {
		name := entry.Name()
		if !strings.HasSuffix(name, ".json") || name == "config.json" {
			continue
		}
		id := strings.TrimSuffix(name, ".json")

		// Match against full ID or just the part after the project prefix
		if strings.HasPrefix(strings.ToLower(id), lowerInput) {
			matches = append(matches, id)
		} else if dash := strings.LastIndex(id, "-"); dash != -1 {
			ref := id[dash+1:]
			if strings.HasPrefix(strings.ToLower(ref), lowerInput) {
				matches = append(matches, id)
			}
		}
	}

	if len(matches) == 1 {
		return matches[0], nil
	}
	if len(matches) > 1 {
		return "", fmt.Errorf("ambiguous ID %q: matched %s", input, strings.Join(matches, ", "))
	}

	return "", fmt.Errorf("task not found: %s", input)
}

// --- Renames ---

type RenameResult struct {
	Renamed           []string
	ReferencesUpdated int
}

func RenameProject(oldName, newName string) (*RenameResult, error) {
	root := FindRoot()
	if !root.Exists {
		return nil, &ErrTasksNotFound{SearchedFrom: GetWorkingDir()}
	}

	allTasks, err := GetAllTasks(root.TasksDir)
	if err != nil {
		return nil, err
	}

	var toRename []*Task
	for _, t := range allTasks {
		if t.Project == oldName {
			toRename = append(toRename, t)
		}
	}

	if len(toRename) == 0 {
		return nil, fmt.Errorf("no tasks found with project %q", oldName)
	}

	// Check for collisions
	existingIDs := make(map[string]bool)
	for _, t := range allTasks {
		existingIDs[t.ID()] = true
	}

	idMap := make(map[string]string)
	for _, t := range toRename {
		newID := TaskID(newName, t.Ref)
		if existingIDs[newID] {
			return nil, fmt.Errorf("cannot rename: %q already exists", newID)
		}
		idMap[t.ID()] = newID
	}

	res := &RenameResult{Renamed: make([]string, 0, len(toRename))}

	for _, t := range allTasks {
		modified := false

		for i, b := range t.BlockedBy {
			if newID, ok := idMap[b]; ok {
				t.BlockedBy[i] = newID
				res.ReferencesUpdated++
				modified = true
			}
		}

		if t.Parent != nil {
			if newID, ok := idMap[*t.Parent]; ok {
				t.Parent = &newID
				res.ReferencesUpdated++
				modified = true
			}
		}

		if t.Project == oldName {
			oldPath := getTaskPath(root.TasksDir, t.ID())
			t.Project = newName
			res.Renamed = append(res.Renamed, t.ID())

			if err := SaveTask(t); err != nil {
				return nil, err
			}
			if err := os.Remove(oldPath); err != nil && !os.IsNotExist(err) {
				return nil, fmt.Errorf("remove old task file %s: %w", oldPath, err)
			}
		} else if modified {
			if err := SaveTask(t); err != nil {
				return nil, err
			}
		}
	}

	config := GetConfig()
	if config.Project == oldName {
		config.Project = newName
		_ = SaveConfig(config)
	}

	return res, nil
}

type MoveResult struct {
	OldID             string
	NewID             string
	ReferencesUpdated int
}

func MoveTask(id, newProject string) (*MoveResult, error) {
	root := FindRoot()
	if !root.Exists {
		return nil, &ErrTasksNotFound{SearchedFrom: GetWorkingDir()}
	}

	proj, ref, ok := ParseID(id)
	if !ok {
		return nil, fmt.Errorf("invalid task ID: %s", id)
	}

	if proj == newProject {
		return nil, fmt.Errorf("task %s is already in project %q", id, newProject)
	}

	oldPath := getTaskPath(root.TasksDir, id)
	task, err := ReadTaskFile(oldPath)
	if err != nil {
		return nil, fmt.Errorf("task not found: %s", id)
	}

	newID := TaskID(newProject, ref)
	newPath := getTaskPath(root.TasksDir, newID)
	if _, err := os.Stat(newPath); err == nil {
		return nil, fmt.Errorf("cannot move: %q already exists", newID)
	}

	task.Project = newProject
	task.UpdatedAt = time.Now().UTC().Format(time.RFC3339Nano)

	if err := SaveTask(task); err != nil {
		return nil, err
	}
	if err := os.Remove(oldPath); err != nil && !os.IsNotExist(err) {
		return nil, fmt.Errorf("remove old task file: %w", err)
	}

	res := &MoveResult{OldID: id, NewID: newID}

	allTasks, err := GetAllTasks(root.TasksDir)
	if err == nil {
		for _, t := range allTasks {
			modified := false
			for i, b := range t.BlockedBy {
				if b == id {
					t.BlockedBy[i] = newID
					res.ReferencesUpdated++
					modified = true
				}
			}
			if t.Parent != nil && *t.Parent == id {
				t.Parent = &newID
				res.ReferencesUpdated++
				modified = true
			}
			if modified {
				_ = SaveTask(t)
			}
		}
	}

	return res, nil
}

// --- Cycles ---

func WouldCreateBlockCycle(taskId, blockerId string) bool {
	root := FindRoot()
	visited := make(map[string]bool)
	stack := []string{blockerId}

	for len(stack) > 0 {
		current := stack[len(stack)-1]
		stack = stack[:len(stack)-1]

		if current == taskId {
			return true
		}
		if visited[current] {
			continue
		}
		visited[current] = true

		task, err := ReadTaskFile(getTaskPath(root.TasksDir, current))
		if err == nil {
			stack = append(stack, task.BlockedBy...)
		}
	}
	return false
}

func WouldCreateParentCycle(taskId, parentId string) bool {
	root := FindRoot()
	visited := make(map[string]bool)
	current := parentId

	for current != "" {
		if current == taskId {
			return true
		}
		if visited[current] {
			return true
		}
		visited[current] = true

		task, err := ReadTaskFile(getTaskPath(root.TasksDir, current))
		if err != nil || task.Parent == nil {
			break
		}
		current = *task.Parent
	}
	return false
}

func ValidateParent(parentId, currentTaskId string) error {
	if _, _, ok := ParseID(parentId); !ok {
		return fmt.Errorf("invalid parent ID format: %s", parentId)
	}

	if currentTaskId != "" && parentId == currentTaskId {
		return errors.New("task cannot be its own parent")
	}

	root := FindRoot()
	if _, err := os.Stat(getTaskPath(root.TasksDir, parentId)); os.IsNotExist(err) {
		return fmt.Errorf("parent task not found: %s", parentId)
	}

	if currentTaskId != "" && WouldCreateParentCycle(currentTaskId, parentId) {
		return errors.New("would create circular parent relationship")
	}

	return nil
}

// --- Cleanup ---

type CleanupInfo struct {
	OrphanedBlockers []string
	OrphanedParent   string
	IDMismatch       *struct{ Was, Fixed string }
}

func CleanTaskOrphans(task *Task, expectedID, tasksDir string) *CleanupInfo {
	cleanup := &CleanupInfo{}
	modified := false

	actualID := task.ID()
	if actualID != expectedID {
		proj, ref, ok := ParseID(expectedID)
		if ok {
			cleanup.IDMismatch = &struct{ Was, Fixed string }{Was: actualID, Fixed: expectedID}
			task.Project = proj
			task.Ref = ref
			modified = true
		}
	}

	var validBlockers []string
	for _, b := range task.BlockedBy {
		if _, err := os.Stat(getTaskPath(tasksDir, b)); err == nil {
			validBlockers = append(validBlockers, b)
		} else {
			cleanup.OrphanedBlockers = append(cleanup.OrphanedBlockers, b)
			modified = true
		}
	}
	if len(cleanup.OrphanedBlockers) > 0 {
		task.BlockedBy = validBlockers
	}

	if task.Parent != nil {
		if _, err := os.Stat(getTaskPath(tasksDir, *task.Parent)); os.IsNotExist(err) {
			cleanup.OrphanedParent = *task.Parent
			task.Parent = nil
			modified = true
		}
	}

	if modified {
		task.UpdatedAt = time.Now().UTC().Format(time.RFC3339Nano)
		_ = SaveTask(task)
	}

	if len(cleanup.OrphanedBlockers) == 0 && cleanup.OrphanedParent == "" &&
		cleanup.IDMismatch == nil {
		return nil
	}
	return cleanup
}

func (c *CleanupInfo) Message(id string) string {
	var parts []string
	if len(c.OrphanedBlockers) > 0 {
		s := "references"
		if len(c.OrphanedBlockers) == 1 {
			s = "reference"
		}
		parts = append(parts, fmt.Sprintf("%d orphaned %s", len(c.OrphanedBlockers), s))
	}
	if c.OrphanedParent != "" {
		parts = append(parts, "orphaned parent")
	}
	if c.IDMismatch != nil {
		parts = append(parts, fmt.Sprintf("ID mismatch (was %s)", c.IDMismatch.Was))
	}
	return fmt.Sprintf("(cleaned %s from %s)", strings.Join(parts, ", "), id)
}

func CleanTasks(days int) (int, error) {
	root := FindRoot()
	tasks, err := GetAllTasks(root.TasksDir)
	if err != nil {
		return 0, err
	}

	now := time.Now().UTC()
	threshold := time.Duration(days) * 24 * time.Hour

	toDelete := make(map[string]bool)
	for _, t := range tasks {
		if IsTerminalStatus(t.Status) && t.CompletedAt != nil {
			comp, err := time.Parse(time.RFC3339Nano, *t.CompletedAt)
			if err != nil {
				comp, err = time.Parse(time.RFC3339, *t.CompletedAt)
			}
			if err == nil && now.Sub(comp) > threshold {
				toDelete[t.ID()] = true
			}
		}
	}

	for id := range toDelete {
		_ = os.Remove(getTaskPath(root.TasksDir, id))
	}

	// Update references in surviving tasks in one pass.
	for _, t := range tasks {
		if toDelete[t.ID()] {
			continue
		}
		modified := false
		newBlockedBy := make([]string, 0, len(t.BlockedBy))
		for _, b := range t.BlockedBy {
			if toDelete[b] {
				modified = true
			} else {
				newBlockedBy = append(newBlockedBy, b)
			}
		}
		if modified {
			t.BlockedBy = newBlockedBy
		}
		if t.Parent != nil && toDelete[*t.Parent] {
			t.Parent = nil
			modified = true
		}
		if modified {
			_ = SaveTask(t)
		}
	}

	return len(toDelete), nil
}

// --- Mutation helpers for block/unblock/log ---

// AddBlocker appends a blocker to a task and saves it.
func AddBlocker(id, blockerID string) (*Task, error) {
	root := FindRoot()
	t, err := ReadTaskFile(getTaskPath(root.TasksDir, id))
	if err != nil {
		return nil, err
	}
	t.BlockedBy = append(t.BlockedBy, blockerID)
	t.UpdatedAt = time.Now().UTC().Format(time.RFC3339Nano)
	return t, SaveTask(t)
}

// RemoveBlocker removes a blocker from a task and saves it.
// Returns (task, found, error). found is false if the blocker was not present.
func RemoveBlocker(id, blockerID string) (*Task, bool, error) {
	root := FindRoot()
	t, err := ReadTaskFile(getTaskPath(root.TasksDir, id))
	if err != nil {
		return nil, false, err
	}
	newBlockedBy := make([]string, 0, len(t.BlockedBy))
	found := false
	for _, b := range t.BlockedBy {
		if b == blockerID {
			found = true
		} else {
			newBlockedBy = append(newBlockedBy, b)
		}
	}
	if !found {
		return t, false, nil
	}
	t.BlockedBy = newBlockedBy
	t.UpdatedAt = time.Now().UTC().Format(time.RFC3339Nano)
	return t, true, SaveTask(t)
}

// AddLog appends a log entry to a task and saves it.
func AddLog(id, msg string) (*Task, error) {
	root := FindRoot()
	t, err := ReadTaskFile(getTaskPath(root.TasksDir, id))
	if err != nil {
		return nil, err
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	t.Logs = append(t.Logs, LogEntry{Ts: now, Msg: msg})
	t.UpdatedAt = now
	return t, SaveTask(t)
}

func CheckIntegrity() ([]string, error) {
	root := FindRoot()
	tasks, err := GetAllTasks(root.TasksDir)
	if err != nil {
		return nil, err
	}

	var issues []string
	idMap := make(map[string]bool)
	for _, t := range tasks {
		idMap[t.ID()] = true
	}

	for _, t := range tasks {
		id := t.ID()
		for _, b := range t.BlockedBy {
			if !idMap[b] {
				issues = append(issues, fmt.Sprintf("Task %s is blocked by missing task %s", id, b))
			}
		}
		if t.Parent != nil && !idMap[*t.Parent] {
			issues = append(issues, fmt.Sprintf("Task %s has missing parent %s", id, *t.Parent))
		}
	}

	return issues, nil
}
