package cmd

import (
	"fmt"
	"path/filepath"
	"time"

	"github.com/nijaru/tk-go/internal/format"
	"github.com/nijaru/tk-go/internal/task"
)

type LogCmd struct {
	ID  string `arg:"" help:"Task ID or ref"`
	Msg string `arg:"" help:"Log message"`
}

func (c *LogCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	root := task.FindRoot()
	t, err := task.ReadTaskFile(filepath.Join(root.TasksDir, id+".json"))
	if err != nil {
		return err
	}

	t.Logs = append(t.Logs, task.LogEntry{
		Ts:  time.Now().UTC().Format(time.RFC3339),
		Msg: c.Msg,
	})
	t.UpdatedAt = time.Now().UTC().Format(time.RFC3339)

	if err := task.SaveTask(t); err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Logged to %s: %s\n", t.ID(), c.Msg)
	}

	return nil
}
