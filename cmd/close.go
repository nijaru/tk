package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type CloseCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *CloseCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	t, err := task.UpdateTaskStatus(id, task.StatusClosed)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Closed %s: %s\n", t.ID, t.Title)
	}

	return nil
}
