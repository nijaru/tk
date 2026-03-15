package priority

import (
	"testing"

	"github.com/nijaru/tk/internal/task"
	"github.com/stretchr/testify/assert"
)

func TestParse(t *testing.T) {
	tests := []struct {
		input    string
		expected task.Priority
		err      bool
	}{
		{"0", task.PriorityNone, false},
		{"1", task.PriorityUrgent, false},
		{"4", task.PriorityLow, false},
		{"p1", task.PriorityUrgent, false},
		{"p4", task.PriorityLow, false},
		{"urgent", task.PriorityUrgent, false},
		{"low", task.PriorityLow, false},
		{"NONE", task.PriorityNone, false},
		{"5", 0, true},
		{"p5", 0, true},
		{"invalid", 0, true},
	}

	for _, tt := range tests {
		got, err := Parse(tt.input)
		if tt.err {
			assert.Error(t, err)
		} else {
			assert.NoError(t, err)
			assert.Equal(t, tt.expected, got)
		}
	}
}

func TestFormat(t *testing.T) {
	assert.Equal(t, "p1", Format(task.PriorityUrgent))
	assert.Equal(t, "p0", Format(task.PriorityNone))
}

func TestFormatName(t *testing.T) {
	assert.Equal(t, "urgent", FormatName(task.PriorityUrgent))
	assert.Equal(t, "none", FormatName(task.PriorityNone))
	assert.Equal(t, "unknown", FormatName(task.Priority(10)))
}
