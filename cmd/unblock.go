package cmd

import (
	"fmt"
	"path/filepath"
	"time"

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

	blockerId, err := task.ResolveID(c.Blocker)
	if err != nil {
		return err
	}

	root := task.FindRoot()
	t, err := task.ReadTaskFile(filepath.Join(root.TasksDir, id+".json"))
	if err != nil {
		return err
	}

	found := false
	newBlockedBy := make([]string, 0, len(t.BlockedBy))
	for _, b := range t.BlockedBy {
		if b == blockerId {
			found = true
			continue
		}
		newBlockedBy = append(newBlockedBy, b)
	}

	if !found {
		fmt.Printf("Task %s is not blocked by %s\n", id, blockerId)
		return nil
	}

	t.BlockedBy = newBlockedBy
	t.UpdatedAt = time.Now().UTC().Format(time.RFC3339)

	if err := task.SaveTask(t); err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Unblocked %s from %s\n", id, blockerId)
	}

	return nil
}
