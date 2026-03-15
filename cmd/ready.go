package cmd

import (
	"fmt"

	"github.com/nijaru/tk-go/internal/format"
	"github.com/nijaru/tk-go/internal/task"
)

type ReadyCmd struct{}

func (c *ReadyCmd) Run(cli *CLI) error {
	// ListTasks with no filters but internal logic for ready tasks
	// Ready tasks are active/open and NOT blocked by incomplete tasks.
	// We can add a Ready filter to ListOptions or use a helper.
	// Let's use ListTasks and filter in-place or add to internal/task.
	
	// TypeScript had a dedicated listReadyTasks in storage.ts.
	// I'll add it to storage.go if not already there, or just use ListTasks if I can filter easily.
	
	// Actually listReadyTasks was in the TS source I ported. Let's see if I added it.
	
	tasks, err := task.ListTasks(task.ListOptions{}) // Default list
	if err != nil {
		return err
	}
	
	var ready []*task.TaskWithMeta
	for _, t := range tasks {
		if (t.Status == task.StatusOpen || t.Status == task.StatusActive) && !t.BlockedByIncomplete {
			ready = append(ready, t)
		}
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(ready))
	} else {
		fmt.Println(format.FormatTaskList(ready, "No ready tasks found. Good job!"))
	}

	return nil
}
