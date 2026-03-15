package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/task"
)

type CleanCmd struct {
	OlderThan int  `help:"Remove tasks completed more than N days ago" default:"0"`
	Force     bool `help:"Force clean even if disabled in config"`
}

func (c *CleanCmd) Run(cli *CLI) error {
	config := task.GetConfig()
	
	days := c.OlderThan
	if days == 0 {
		if config.CleanAfter.Enabled {
			days = config.CleanAfter.Days
		} else if !c.Force {
			fmt.Println("Auto-clean is disabled in config. Use --older-than N or --force to clean manually.")
			return nil
		}
	}

	// Implementation of clean in storage.go
	// Since I haven't implemented CleanTasks in storage.go yet, I'll add it.
	deleted, err := task.CleanTasks(days)
	if err != nil {
		return err
	}

	fmt.Printf("Cleaned %d tasks completed more than %d days ago.\n", deleted, days)
	return nil
}
