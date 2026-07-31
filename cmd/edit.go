package cmd

import (
	"fmt"
	"sort"
	"strings"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/priority"
	"github.com/nijaru/tk/internal/task"
	"github.com/nijaru/tk/internal/timeutil"
)

type EditCmd struct {
	ID              string   `arg:"" help:"Task ID or ref"`
	Title           string   `       help:"New title"                                                     short:"t"`
	Priority        string   `       help:"New priority"                                                  short:"p"`
	Labels          []string `       help:"Labels (+add, or replace; use --remove-label to remove)"       short:"l" sep:","`
	RemoveLabels    []string `       help:"Labels to remove (CSV)"                                                  sep:"," name:"remove-label"`
	Assignees       []string `       help:"Assignees (+add, or replace; use --remove-assignee to remove)" short:"A" sep:","`
	RemoveAssignees []string `       help:"Assignees to remove (CSV)"                                               sep:"," name:"remove-assignee"`
	Due             string   `       help:"Due date (YYYY-MM-DD, relative, or - to clear)"`
	Parent          string   `       help:"Parent task (or - to clear)"`
	Desc            string   `       help:"Description"                                                   short:"d"`
	Estimate        *int     `       help:"Estimate (or 0 to clear)"`
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

	if len(c.Labels) > 0 || len(c.RemoveLabels) > 0 {
		updates.Labels = applySliceUpdates(t.Labels, appendRemovals(c.Labels, c.RemoveLabels))
	}

	if len(c.Assignees) > 0 || len(c.RemoveAssignees) > 0 {
		updates.Assignees = applySliceUpdates(
			t.Assignees,
			appendRemovals(c.Assignees, c.RemoveAssignees),
		)
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

func appendRemovals(updates, removals []string) []string {
	result := make([]string, 0, len(updates)+len(removals))
	result = append(result, updates...)
	for _, removal := range removals {
		result = append(result, "-"+removal)
	}
	return result
}

func applySliceUpdates(current []string, updates []string) []string {
	set := make(map[string]bool)
	for _, s := range current {
		set[s] = true
	}

	replaced := false
	for _, s := range updates {
		if strings.HasPrefix(s, "+") {
			set[s[1:]] = true
		} else if strings.HasPrefix(s, "-") {
			delete(set, s[1:])
		} else {
			if !replaced {
				set = make(map[string]bool)
				replaced = true
			}
			set[s] = true
		}
	}

	res := make([]string, 0, len(set))
	for s := range set {
		res = append(res, s)
	}
	sort.Strings(res)
	return res
}
