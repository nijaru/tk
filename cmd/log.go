package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
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

	t, err := task.AddLog(id, c.Msg)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Logged to %s: %s\n", t.ID(), c.Msg)
	}

	return nil
}
