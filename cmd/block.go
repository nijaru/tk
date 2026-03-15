package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type BlockCmd struct {
	ID      string `arg:"" help:"Task ID or ref to block"`
	Blocker string `arg:"" help:"Blocking task ID or ref"`
}

func (c *BlockCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	blockerID, err := task.ResolveID(c.Blocker)
	if err != nil {
		return err
	}

	if id == blockerID {
		return fmt.Errorf("task cannot block itself")
	}

	existing, _, err := task.GetTask(id)
	if err != nil {
		return err
	}

	for _, b := range existing.BlockedBy {
		if b == blockerID {
			fmt.Printf("Task %s is already blocked by %s\n", id, blockerID)
			return nil
		}
	}

	if task.WouldCreateBlockCycle(id, blockerID) {
		return fmt.Errorf("would create circular dependency")
	}

	t, err := task.AddBlocker(id, blockerID)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Blocked %s by %s\n", id, blockerID)
	}

	return nil
}
