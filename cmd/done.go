package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type DoneCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *DoneCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	t, err := task.UpdateTaskStatus(id, task.StatusDone)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Completed %s: %s\n", t.ID, t.Title)
	}

	return nil
}
