package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type CheckCmd struct{}

func (c *CheckCmd) Run(cli *CLI) error {
	issues, err := task.CheckIntegrity()
	if err != nil {
		return err
	}

	if len(issues) == 0 {
		fmt.Println("No integrity issues found.")
	} else {
		for _, issue := range issues {
			fmt.Println(format.Yellow(issue))
		}
	}

	return nil
}
