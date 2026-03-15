package timeutil

import (
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
)

func TestParseDueDate(t *testing.T) {
	tests := []struct {
		input    string
		expected string
		err      bool
	}{
		{"2026-03-20", "2026-03-20", false},
		{"-", "", false},
		{"", "", false},
		{"2026-02-31", "", true}, // Invalid date
		{"invalid", "", true},
		{"+1d", FormatLocalDate(time.Now().AddDate(0, 0, 1)), false},
		{"+1w", FormatLocalDate(time.Now().AddDate(0, 0, 7)), false},
		{"+1m", FormatLocalDate(time.Now().AddDate(0, 1, 0)), false},
	}

	for _, tt := range tests {
		got, err := ParseDueDate(tt.input)
		if tt.err {
			assert.Error(t, err)
		} else {
			assert.NoError(t, err)
			assert.Equal(t, tt.expected, got)
		}
	}
}

func TestIsOverdue(t *testing.T) {
	past := "2000-01-01"
	future := "2100-01-01"
	
	assert.True(t, IsOverdue(&past, false))
	assert.False(t, IsOverdue(&past, true)) // Completed
	assert.False(t, IsOverdue(&future, false))
	assert.False(t, IsOverdue(nil, false))
}

func TestDaysUntilDue(t *testing.T) {
	future := time.Now().AddDate(0, 0, 5)
	futureStr := FormatLocalDate(future)
	
	days := DaysUntilDue(&futureStr, false)
	assert.NotNil(t, days)
	assert.Equal(t, 5, *days)
	
	past := "2000-01-01"
	assert.Nil(t, DaysUntilDue(&past, false))
	assert.Nil(t, DaysUntilDue(nil, false))
}

func TestFormatRelative(t *testing.T) {
	now := time.Now().Format(time.RFC3339)
	assert.Equal(t, "now", FormatRelative(now))
	
	h2 := time.Now().Add(-2 * time.Hour).Format(time.RFC3339)
	assert.Equal(t, "2h", FormatRelative(h2))
	
	d3 := time.Now().AddDate(0, 0, -3).Format(time.RFC3339)
	assert.Equal(t, "3d", FormatRelative(d3))
}
