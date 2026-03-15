package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type MvCmd struct {
	ID      string `arg:"" help:"Task ID or ref"`
	Project string `arg:"" help:"Target project name"`
}

func (c *MvCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
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
		fmt.Printf("Moved %s -> %s (updated %d references)\n", res.OldID, res.NewID, res.ReferencesUpdated)
	}

	return nil
}
