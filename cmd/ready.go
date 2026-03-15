package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type ReadyCmd struct{}

func (c *ReadyCmd) Run(cli *CLI) error {
	tasks, err := task.ListTasks(task.ListOptions{})
	if err != nil {
		return err
	}

	var ready []*task.TaskWithMeta
	for _, t := range tasks {
		if (t.Status == task.StatusOpen || t.Status == task.StatusActive) &&
			!t.BlockedByIncomplete {
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
