package cmd

import (
	"fmt"

	"github.com/nijaru/tk-go/internal/format"
	"github.com/nijaru/tk-go/internal/task"
)

type StartCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *StartCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	t, err := task.UpdateTaskStatus(id, task.StatusActive)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Started %s: %s\n", t.ID, t.Title)
	}

	return nil
}
