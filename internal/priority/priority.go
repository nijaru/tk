package priority

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/nijaru/tk-go/internal/task"
)

// Parse parses priority from number (0-4), prefixed (p0-p4), or named form.
func Parse(input string) (task.Priority, error) {
	s := strings.ToLower(strings.TrimSpace(input))

	if p, ok := task.PriorityFromName[s]; ok {
		return p, nil
	}

	if strings.HasPrefix(s, "p") {
		n, err := strconv.Atoi(s[1:])
		if err == nil && n >= 0 && n <= 4 {
			return task.Priority(n), nil
		}
	}

	n, err := strconv.Atoi(s)
	if err == nil && n >= 0 && n <= 4 {
		return task.Priority(n), nil
	}

	return 0, fmt.Errorf(
		"invalid priority %q: use 0-4, p0-p4, or none/urgent/high/medium/low",
		input,
	)
}

// Format returns the short label like "p1".
func Format(p task.Priority) string {
	return fmt.Sprintf("p%d", p)
}

// FormatName returns the name like "urgent".
func FormatName(p task.Priority) string {
	if name, ok := task.PriorityLabels[p]; ok {
		return name
	}
	return "unknown"
}
