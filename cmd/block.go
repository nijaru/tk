package cmd

import (
	"fmt"
	"path/filepath"
	"time"

	"github.com/nijaru/tk-go/internal/format"
	"github.com/nijaru/tk-go/internal/task"
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

	blockerId, err := task.ResolveID(c.Blocker)
	if err != nil {
		return err
	}

	if id == blockerId {
		return fmt.Errorf("task cannot block itself")
	}

	root := task.FindRoot()
	t, err := task.ReadTaskFile(filepath.Join(root.TasksDir, id+".json"))
	if err != nil {
		return err
	}

	// Check if already blocked
	for _, b := range t.BlockedBy {
		if b == blockerId {
			fmt.Printf("Task %s is already blocked by %s\n", id, blockerId)
			return nil
		}
	}

	// Cycle detection
	if task.WouldCreateBlockCycle(id, blockerId) {
		return fmt.Errorf("would create circular dependency")
	}

	t.BlockedBy = append(t.BlockedBy, blockerId)
	t.UpdatedAt = time.Now().UTC().Format(time.RFC3339)

	if err := task.SaveTask(t); err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Blocked %s by %s\n", id, blockerId)
	}

	return nil
}
