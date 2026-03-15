package task

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

const (
	tasksDirName = ".tasks"
	gitDirName   = ".git"
)

// workingDir is the global override set by the -C flag.
var workingDir string

// SetWorkingDir sets the global working directory override.
// If the provided dir matches an alias in the project config, it resolves to the alias path.
func SetWorkingDir(dir string) error {
	// First, try absolute path normally
	abs, err := filepath.Abs(dir)
	if err != nil {
		return fmt.Errorf("resolve working dir: %w", err)
	}

	// We can't easily check aliases without finding the root first.
	// But FindRoot depends on workingDir.
	// We'll set the provisional working dir, then check for aliases.
	workingDir = abs

	root := FindRoot()
	if root.Exists {
		configPath := filepath.Join(root.TasksDir, "config.json")
		data, err := os.ReadFile(configPath)
		if err == nil {
			var config struct {
				Aliases map[string]string `json:"aliases"`
			}
			if err := json.Unmarshal(data, &config); err == nil {
				if aliasPath, ok := config.Aliases[dir]; ok {
					// Alias path is relative to project root
					workingDir = filepath.Join(root.Root, aliasPath)
				}
			}
		}
	}

	return nil
}

// GetWorkingDir returns the effective working directory.
func GetWorkingDir() string {
	if workingDir != "" {
		return workingDir
	}
	wd, err := os.Getwd()
	if err != nil {
		return "."
	}
	return wd
}

type RootInfo struct {
	Root     string
	TasksDir string
	Exists   bool
}

// FindRoot walks up from the working directory looking for .tasks/ or .git/.
func FindRoot() RootInfo {
	current := GetWorkingDir()

	for {
		tasksPath := filepath.Join(current, tasksDirName)
		if _, err := os.Stat(tasksPath); err == nil {
			return RootInfo{Root: current, TasksDir: tasksPath, Exists: true}
		}

		gitPath := filepath.Join(current, gitDirName)
		if _, err := os.Stat(gitPath); err == nil {
			return RootInfo{
				Root:     current,
				TasksDir: filepath.Join(current, tasksDirName),
				Exists:   false,
			}
		}

		parent := filepath.Dir(current)
		if parent == current {
			break
		}
		current = parent
	}

	wd := GetWorkingDir()
	return RootInfo{
		Root:     wd,
		TasksDir: filepath.Join(wd, tasksDirName),
		Exists:   false,
	}
}

// GetTasksDir returns the tasks directory path.
func GetTasksDir() string {
	return FindRoot().TasksDir
}

// ErrTasksNotFound is returned when the .tasks/ directory doesn't exist.
type ErrTasksNotFound struct {
	SearchedFrom string
}

func (e *ErrTasksNotFound) Error() string {
	return fmt.Sprintf(
		"no .tasks/ directory found. Run 'tk add' to create one, or initialize with 'tk init'.\nSearched from: %s",
		e.SearchedFrom,
	)
}
