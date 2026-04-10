package cmd

import (
	"fmt"
	"strings"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/priority"
	"github.com/nijaru/tk/internal/task"
	"github.com/nijaru/tk/internal/timeutil"
)

type AddCmd struct {
	Title     []string `arg:"" help:"Task title"`
	Priority  string   `       help:"Priority (0-4, p0-p4, or none/urgent/high/medium/low)" short:"p"`
	Project   string   `       help:"Project prefix"                                        short:"P"`
	Desc      string   `       help:"Description"                                           short:"d"`
	Labels    []string `       help:"Labels (CSV)"                                          short:"l" sep:","`
	Assignees []string `       help:"Assignees (CSV, @me for git user)"                     short:"A" sep:","`
	Parent    string   `       help:"Parent task ID"`
	Estimate  *int     `       help:"Estimate (user-defined units)"`
	Due       string   `       help:"Due date (YYYY-MM-DD or relative +Nh/+Nd/+Nw/+Nm)"`
}

func (c *AddCmd) Run(cli *CLI) error {
	prio := task.PriorityNone
	if c.Priority != "" {
		p, err := priority.Parse(c.Priority)
		if err != nil {
			return err
		}
		prio = p
	}

	var dueDate *string
	if c.Due != "" {
		d, err := timeutil.ParseDueDate(c.Due)
		if err != nil {
			return err
		}
		if d != "" {
			dueDate = &d
		}
	}

	var parent *string
	if c.Parent != "" {
		id, err := task.ResolveID(c.Parent)
		if err != nil {
			return err
		}
		parent = &id
	}

	opts := task.CreateTaskOptions{
		Title:     strings.Join(c.Title, " "),
		Priority:  prio,
		Project:   c.Project,
		Labels:    c.Labels,
		Assignees: c.Assignees,
		Parent:    parent,
		Estimate:  c.Estimate,
		DueDate:   dueDate,
	}

	if c.Desc != "" {
		opts.Description = &c.Desc
	}

	t, err := task.CreateTask(opts)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(t))
	} else {
		fmt.Printf("Created task %s: %s\n", t.ID, t.Title)
	}

	return nil
}
