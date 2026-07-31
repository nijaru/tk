package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/task"
)

type CleanCmd struct {
	OlderThan *int `help:"Remove tasks completed more than N days ago"`
	Force     bool `help:"Force clean even if disabled in config"`
}

func (c *CleanCmd) Run(cli *CLI) error {
	config := task.GetConfig()

	var days int
	switch {
	case c.OlderThan != nil:
		if *c.OlderThan < 0 {
			return fmt.Errorf("--older-than must be non-negative")
		}
		days = *c.OlderThan
	case config.CleanAfter.Enabled:
		days = config.CleanAfter.Days
	case c.Force:
		days = config.CleanAfter.Days
		if days <= 0 {
			days = task.DefaultConfig.CleanAfter.Days
		}
	default:
		fmt.Println(
			"Auto-clean is disabled. Use --older-than N or enable with 'tk config clean-after enable'.",
		)
		return nil
	}

	deleted, err := task.CleanTasks(days)
	if err != nil {
		return err
	}

	fmt.Printf("Cleaned %d tasks completed more than %d days ago.\n", deleted, days)
	return nil
}
