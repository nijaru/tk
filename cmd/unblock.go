package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type UnblockCmd struct {
	ID      string `arg:"" help:"Task ID or ref"`
	Blocker string `arg:"" help:"Blocking task ID or ref to remove"`
}

func (c *UnblockCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	blockerID, err := task.ResolveID(c.Blocker)
	if err != nil {
		return err
	}

	t, found, err := task.RemoveBlocker(id, blockerID)
	if err != nil {
		return err
	}

	if !found {
		fmt.Printf("Task %s is not blocked by %s\n", id, blockerID)
		return nil
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Unblocked %s from %s\n", id, blockerID)
	}

	return nil
}
