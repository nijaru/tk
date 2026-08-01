package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type MvCmd struct {
	Source  string `arg:"" help:"Task ID or Project name"`
	Project string `arg:"" help:"Target project name"`
}

func (c *MvCmd) Run(cli *CLI) error {
	root := task.FindRoot()
	if !root.Exists {
		return &task.ErrTasksNotFound{SearchedFrom: task.GetWorkingDir()}
	}

	// Check if Source is an exact project name
	allTasks, err := task.GetAllTasks(root.TasksDir)
	if err != nil {
		return err
	}

	projectExists := false
	for _, t := range allTasks {
		if t.Project == c.Source {
			projectExists = true
			break
		}
	}

	if projectExists {
		res, err := task.RenameProject(c.Source, c.Project)
		if err != nil {
			return err
		}
		if cli.JSON {
			fmt.Println(format.FormatJson(res))
		} else {
			fmt.Printf(
				"Renamed project %s -> %s (updated %d references, %d tasks)\n",
				c.Source,
				c.Project,
				res.ReferencesUpdated,
				len(res.Renamed),
			)
		}
		return nil
	}

	// Not a project, try as a task ID
	id, err := task.ResolveID(c.Source)
	if err != nil {
		return err
	}

	res, err := task.MoveTask(id, c.Project)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(res))
	} else {
		fmt.Printf(
			"Moved %s -> %s (updated %d references)\n",
			res.OldID,
			res.NewID,
			res.ReferencesUpdated,
		)
	}

	return nil
}
