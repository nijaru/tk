package cmd

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"github.com/nijaru/tk/internal/task"
)

type RmCmd struct {
	ID    string `arg:"" help:"Task ID or ref"`
	Force bool   `       help:"Skip confirmation" short:"f"`
}

func (c *RmCmd) Run(cli *CLI) error {
	id, err := task.ResolveID(c.ID)
	if err != nil {
		return err
	}

	if !c.Force {
		t, _, err := task.GetTask(id)
		if err != nil {
			return err
		}

		fmt.Printf("Delete %s %q? [y/N] ", t.ID, t.Title)
		reader := bufio.NewReader(os.Stdin)
		response, err := reader.ReadString('\n')
		if err != nil {
			return err
		}
		response = strings.ToLower(strings.TrimSpace(response))
		if response != "y" && response != "yes" {
			fmt.Println("Aborted.")
			return nil
		}
	}

	if err := task.DeleteTask(id); err != nil {
		return err
	}

	fmt.Printf("Deleted task %s\n", id)
	return nil
}
