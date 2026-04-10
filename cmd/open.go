package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type OpenCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *OpenCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	t, err := task.UpdateTaskStatus(id, task.StatusOpen)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Set %s to open: %s\n", t.ID, t.Title)
	}

	return nil
}
