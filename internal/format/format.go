package format

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/fatih/color"
	"github.com/nijaru/tk-go/internal/priority"
	"github.com/nijaru/tk-go/internal/task"
	"github.com/nijaru/tk-go/internal/timeutil"
)

// Colors
var (
	Green  = color.New(color.FgGreen).SprintFunc()
	Red    = color.New(color.FgRed).SprintFunc()
	Yellow = color.New(color.FgYellow).SprintFunc()
	Dim    = color.New(color.Faint).SprintFunc()
	Bold   = color.New(color.Bold).SprintFunc()

	StatusColors = map[task.Status]*color.Color{
		task.StatusOpen:   color.New(color.FgBlue),
		task.StatusActive: color.New(color.FgCyan),
		task.StatusDone:   color.New(color.Faint),
	}

	PriorityColors = map[task.Priority]*color.Color{
		task.PriorityUrgent: color.New(color.FgRed, color.Bold),
		task.PriorityHigh:   color.New(color.FgRed),
		task.PriorityMedium: color.New(color.FgYellow),
		task.PriorityLow:    color.New(color.FgBlue),
		task.PriorityNone:   color.New(color.Faint),
	}

	OverdueColor = color.New(color.FgRed, color.Bold)
	DueSoonColor = color.New(color.FgYellow)
)

func ShouldUseColor() bool {
	if os.Getenv("NO_COLOR") != "" {
		return false
	}
	// fatih/color handles TTY detection automatically, but we can override if needed
	return !color.NoColor
}

func formatId(id string, maxLen int) string {
	dash := strings.LastIndex(id, "-")
	if dash == -1 {
		return truncate(id, maxLen)
	}
	project := id[:dash]
	ref := id[dash+1:]
	maxProjectLen := maxLen - len(ref) - 1
	if len(project) > maxProjectLen {
		project = truncate(project, maxProjectLen)
	}
	return fmt.Sprintf("%s-%s", project, ref)
}

func truncate(text string, maxLen int) string {
	if len(text) <= maxLen {
		return text
	}
	if maxLen <= 1 {
		return "…"
	}
	return text[:maxLen-1] + "…"
}

func FormatTaskRow(t *task.TaskWithMeta) string {
	id := fmt.Sprintf("%-11s", formatId(t.ID, 11))
	prio := fmt.Sprintf("%-4s", priority.Format(t.Priority))
	
	isOverdue := t.IsOverdue
	dueSoon := !isOverdue && t.DaysUntilDue != nil && *t.DaysUntilDue <= timeutil.DueSoonThreshold

	statusText := string(t.Status)
	if t.Status == task.StatusDone && t.CompletedAt != nil {
		statusText = fmt.Sprintf("done %s", timeutil.FormatRelative(*t.CompletedAt))
	}
	status := fmt.Sprintf("%-12s", statusText)
	title := truncate(t.Title, 50)

	if ShouldUseColor() {
		pc := PriorityColors[t.Priority]
		sc := StatusColors[t.Status]
		if isOverdue {
			sc = OverdueColor
		} else if dueSoon {
			sc = DueSoonColor
		}
		
		tc := color.New()
		if t.Status == task.StatusDone {
			tc = color.New(color.Faint)
		}

		return fmt.Sprintf("%s | %s | %s | %s",
			id,
			pc.Sprint(prio),
			sc.Sprint(status),
			tc.Sprint(title),
		)
	}

	markers := ""
	if isOverdue {
		markers += " [OVERDUE]"
	} else if dueSoon {
		if *t.DaysUntilDue == 0 {
			markers += " [due today]"
		} else {
			markers += fmt.Sprintf(" [due %dd]", *t.DaysUntilDue)
		}
	}

	return fmt.Sprintf("%s | %s | %s | %s%s", id, prio, status, title, markers)
}

func FormatTaskList(tasks []*task.TaskWithMeta, emptyHint string) string {
	if len(tasks) == 0 {
		if emptyHint == "" {
			return "No tasks found. Run 'tk add \"title\"' to create one."
		}
		return emptyHint
	}

	header := "ID          | PRIO | STATUS       | TITLE"
	divider := strings.Repeat("-", 65)
	
	var rows []string
	rows = append(rows, header, divider)
	for _, t := range tasks {
		rows = append(rows, FormatTaskRow(t))
	}

	return strings.Join(rows, "\n")
}

func FormatTaskDetail(t *task.TaskWithMeta) string {
	var lines lineList

	sc := color.New()
	if c, ok := StatusColors[t.Status]; ok {
		sc = c
	}
	if t.IsOverdue {
		sc = OverdueColor
	}

	pc := color.New()
	if c, ok := PriorityColors[t.Priority]; ok {
		pc = c
	}

	lines = append(lines, fmt.Sprintf("ID:          %s", t.ID))
	lines.pushIfNonEmpty("Title:       ", t.Title)
	lines = append(lines, fmt.Sprintf("Status:      %s", sc.Sprint(string(t.Status))))
	lines = append(lines, fmt.Sprintf("Priority:    %s", pc.Sprint(priority.FormatName(t.Priority))))

	if t.Description != nil {
		lines = append(lines, fmt.Sprintf("Description: %s", *t.Description))
	}

	if len(t.Labels) > 0 {
		lines = append(lines, fmt.Sprintf("Labels:      %s", strings.Join(t.Labels, ", ")))
	}

	if len(t.Assignees) > 0 {
		lines = append(lines, fmt.Sprintf("Assignees:   %s", strings.Join(t.Assignees, ", ")))
	}

	if t.Parent != nil {
		lines = append(lines, fmt.Sprintf("Parent:      %s", *t.Parent))
	}

	if t.Estimate != nil {
		lines = append(lines, fmt.Sprintf("Estimate:    %d", *t.Estimate))
	}

	if t.DueDate != nil {
		dueStr := *t.DueDate
		if t.IsOverdue {
			dueStr += OverdueColor.Sprint(" [OVERDUE]")
		} else if t.DaysUntilDue != nil && *t.DaysUntilDue <= timeutil.DueSoonThreshold {
			if *t.DaysUntilDue == 0 {
				dueStr += DueSoonColor.Sprint(" [due today]")
			} else {
				dueStr += DueSoonColor.Sprint(fmt.Sprintf(" [due %dd]", *t.DaysUntilDue))
			}
		}
		lines = append(lines, fmt.Sprintf("Due:         %s", dueStr))
	}

	lines = append(lines, fmt.Sprintf("Created:     %s", timeutil.FormatDate(t.CreatedAt)))
	lines = append(lines, fmt.Sprintf("Updated:     %s", timeutil.FormatDate(t.UpdatedAt)))

	if t.CompletedAt != nil {
		lines = append(lines, fmt.Sprintf("Completed:   %s", timeutil.FormatDate(*t.CompletedAt)))
	}

	if len(t.BlockedBy) > 0 {
		status := " (resolved)"
		if t.BlockedByIncomplete {
			status = " (blocked)"
		}
		lines = append(lines, fmt.Sprintf("Blockers:    %s%s", strings.Join(t.BlockedBy, ", "), status))
	}

	if len(t.Logs) > 0 {
		lines = append(lines, "", "Log:")
		for _, log := range t.Logs {
			lines = append(lines, fmt.Sprintf("  [%s] %s", timeutil.FormatDate(log.Ts), log.Msg))
		}
	}

	return strings.Join(lines, "\n")
}

// Helper for detail formatting
type lineList []string

func (l *lineList) pushIfNonEmpty(prefix, value string) {
	if value != "" {
		*l = append(*l, prefix+value)
	}
}

func FormatJson(data interface{}) string {
	b, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		return "{}"
	}
	return string(b)
}

func FormatConfig(config task.Config) string {
	var lines []string
	lines = append(lines, fmt.Sprintf("Version:     %d", config.Version))
	lines = append(lines, fmt.Sprintf("Project:     %s", config.Project))
	
	if config.CleanAfter.Enabled {
		lines = append(lines, fmt.Sprintf("Clean After: %d days", config.CleanAfter.Days))
	} else {
		lines = append(lines, "Clean After: disabled")
	}

	if len(config.Defaults.Labels) > 0 {
		lines = append(lines, fmt.Sprintf("Def Labels:  %s", strings.Join(config.Defaults.Labels, ", ")))
	}
	if len(config.Defaults.Assignees) > 0 {
		lines = append(lines, fmt.Sprintf("Def Assigns: %s", strings.Join(config.Defaults.Assignees, ", ")))
	}
	lines = append(lines, fmt.Sprintf("Def Prio:    %s", priority.FormatName(config.Defaults.Priority)))

	if len(config.Aliases) > 0 {
		lines = append(lines, "", "Aliases:")
		for k, v := range config.Aliases {
			lines = append(lines, fmt.Sprintf("  %-10s -> %s", k, v))
		}
	}

	return strings.Join(lines, "\n")
}
