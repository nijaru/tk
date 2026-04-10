package cmd

import (
	"fmt"
	"strings"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/priority"
	"github.com/nijaru/tk/internal/task"
)

type LsCmd struct {
	Search   []string `arg:"" optional:"" help:"Search title/description"`
	All      bool     `                   help:"Show all (including done, no limit)" short:"a"`
	Status   string   `                   help:"Filter by status (open/active/done)" short:"s"`
	Priority string   `                   help:"Filter by priority"                  short:"p"`
	Project  string   `                   help:"Filter by project"                   short:"P"`
	Label    string   `                   help:"Filter by label"                     short:"l"`
	Assignee string   `                   help:"Filter by assignee"`
	Parent   string   `                   help:"Filter by parent task"`
	Roots    bool     `                   help:"Top-level tasks only"`
	Overdue  bool     `                   help:"Overdue tasks only"`
	Limit    int      `                   help:"Limit results"                       short:"n" default:"20"`
}

func (c *LsCmd) Run(cli *CLI) error {
	searchQuery := strings.Join(c.Search, " ")
	hideTerminal := !c.All && !task.IsTerminalStatus(task.Status(c.Status))

	opts := task.ListOptions{
		Search:       searchQuery,
		HideTerminal: hideTerminal,
		Status:       task.Status(c.Status),
		Project:      c.Project,
		Label:        c.Label,
		Assignee:     c.Assignee,
		Roots:        c.Roots,
		Overdue:      c.Overdue,
		Limit:        c.Limit,
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
