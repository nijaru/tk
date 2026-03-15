package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type ShowCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *ShowCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	t, cleanup, err := task.GetTask(id)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Println(format.FormatTaskDetail(t))
		if cleanup != nil {
			fmt.Println(format.Yellow(cleanup.Message(id)))
		}
	}

	return nil
}
