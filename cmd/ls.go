package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/priority"
	"github.com/nijaru/tk/internal/task"
)

type LsCmd struct {
	All      bool   `short:"a" help:"Show all (no limit)"`
	Status   string `short:"s" help:"Filter by status (open/active/done)"`
	Priority string `short:"p" help:"Filter by priority"`
	Project  string `short:"P" help:"Filter by project"`
	Label    string `short:"l" help:"Filter by label"`
	Assignee string `          help:"Filter by assignee"`
	Parent   string `          help:"Filter by parent task"`
	Roots    bool   `          help:"Top-level tasks only"`
	Overdue  bool   `          help:"Overdue tasks only"`
	Limit    int    `short:"n" help:"Limit results"                       default:"20"`
}

func (c *LsCmd) Run(cli *CLI) error {
	opts := task.ListOptions{
		Status:   task.Status(c.Status),
		Project:  c.Project,
		Label:    c.Label,
		Assignee: c.Assignee,
		Roots:    c.Roots,
		Overdue:  c.Overdue,
		Limit:    c.Limit,
	}

	if c.All {
		opts.Limit = 0
	}

	if c.Priority != "" {
		p, err := priority.Parse(c.Priority)
		if err != nil {
			return err
		}
		opts.Priority = &p
	}

	if c.Parent != "" {
		id, err := task.ResolveID(c.Parent)
		if err != nil {
			return err
		}
		opts.Parent = &id
	}

	tasks, err := task.ListTasks(opts)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(tasks))
	} else {
		fmt.Println(format.FormatTaskList(tasks, ""))
	}

	return nil
}
