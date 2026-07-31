package cmd

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/nijaru/tk/internal/task"
)

type InitCmd struct {
	Project string `short:"P" help:"Project name (default: directory name)"`
}

func (c *InitCmd) Run(cli *CLI) error {
	root := task.FindRoot()
	if root.Exists {
		return fmt.Errorf(".tasks directory already exists at %s", root.TasksDir)
	}

	projectName := c.Project
	if projectName == "" {
		projectName = filepath.Base(root.Root)
		if projectName == "." || projectName == "/" {
			projectName = "tk"
		}
	}
	if err := task.ValidateProjectName(projectName); err != nil {
		return err
	}

	if err := os.MkdirAll(root.TasksDir, 0o755); err != nil {
		return fmt.Errorf("create .tasks directory: %w", err)
	}

	config := task.DefaultConfig
	config.Project = projectName

	if err := task.SaveConfig(config); err != nil {
		return err
	}

	fmt.Printf("Initialized empty tk project in %s\n", root.TasksDir)
	return nil
}
