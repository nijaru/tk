package cmd

import (
	"fmt"
	"strings"

	"github.com/nijaru/tk-go/internal/format"
	"github.com/nijaru/tk-go/internal/priority"
	"github.com/nijaru/tk-go/internal/task"
	"github.com/nijaru/tk-go/internal/timeutil"
)

type EditCmd struct {
	ID        string   `arg:"" help:"Task ID or ref"`
	Title     string   `       help:"New title"                                      short:"t"`
	Priority  string   `       help:"New priority"                                   short:"p"`
	Labels    []string `       help:"Labels (+add, -remove, or replace)"             short:"l" sep:","`
	Assignees []string `       help:"Assignees"                                      short:"A" sep:","`
	Due       string   `       help:"Due date (YYYY-MM-DD, relative, or - to clear)"`
	Parent    string   `       help:"Parent task (or - to clear)"`
	Desc      string   `       help:"Description"                                    short:"d"`
	Estimate  *int     `       help:"Estimate (or 0 to clear)"`
}

func (c *EditCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	t, _, err := task.GetTask(id)
	if err != nil {
		return err
	}

	updates := task.UpdateTaskOptions{}

	if c.Title != "" {
		updates.Title = &c.Title
	}

	if c.Priority != "" {
		p, err := priority.Parse(c.Priority)
		if err != nil {
			return err
		}
		updates.Priority = &p
	}

	if len(c.Labels) > 0 {
		current := make(map[string]bool)
		for _, l := range t.Labels {
			current[l] = true
		}

		replaced := false
		for _, l := range c.Labels {
			if strings.HasPrefix(l, "+") {
				current[l[1:]] = true
			} else if strings.HasPrefix(l, "-") {
				delete(current, l[1:])
			} else {
				// If no prefix, we assume we want to replace the whole list
				// But original tk logic might be different. 
				// The prompt says replacement if no prefix.
				if !replaced {
					current = make(map[string]bool)
					replaced = true
				}
				current[l] = true
			}
		}

		newList := make([]string, 0, len(current))
		for l := range current {
			newList = append(newList, l)
		}
		updates.Labels = newList
	}

	if len(c.Assignees) > 0 {
		// Similar logic for assignees
		current := make(map[string]bool)
		for _, a := range t.Assignees {
			current[a] = true
		}

		replaced := false
		for _, a := range c.Assignees {
			if strings.HasPrefix(a, "+") {
				current[a[1:]] = true
			} else if strings.HasPrefix(a, "-") {
				delete(current, a[1:])
			} else {
				if !replaced {
					current = make(map[string]bool)
					replaced = true
				}
				current[a] = true
			}
		}

		newList := make([]string, 0, len(current))
		for a := range current {
			newList = append(newList, a)
		}
		updates.Assignees = newList
	}

	if c.Due != "" {
		if c.Due == "-" {
			updates.ClearDue = true
		} else {
			d, err := timeutil.ParseDueDate(c.Due)
			if err != nil {
				return err
			}
			updates.DueDate = &d
		}
	}

	if c.Parent != "" {
		if c.Parent == "-" {
			updates.ClearParent = true
		} else {
			pID, err := task.ResolveID(c.Parent)
			if err != nil {
				return err
			}
			if err := task.ValidateParent(pID, id); err != nil {
				return err
			}
			updates.Parent = &pID
		}
	}

	if c.Desc != "" {
		if c.Desc == "-" {
			updates.ClearDesc = true
		} else {
			updates.Description = &c.Desc
		}
	}

	if c.Estimate != nil {
		if *c.Estimate == 0 {
			updates.ClearEst = true
		} else {
			updates.Estimate = c.Estimate
		}
	}

	updated, err := task.UpdateTask(id, updates)
	if err != nil {
		return err
	}

	if cli.JSON {
		fmt.Println(format.FormatJson(updated))
	} else {
		fmt.Printf("Updated %s: %s\n", updated.ID, updated.Title)
	}

	return nil
}
