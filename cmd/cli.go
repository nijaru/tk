package cmd

import (
	"github.com/alecthomas/kong"
	"github.com/nijaru/tk-go/internal/task"
)

// CLI is the root Kong command struct.
type CLI struct {
	// Global flags
	JSON bool   `short:"j" help:"Output as JSON"`
	Dir  string `short:"C" help:"Run in directory" default:""`

	// Commands
	Init        InitCmd        `cmd:"" help:"Initialize .tasks/ in current directory"`
	Add         AddCmd         `cmd:"" help:"Create a task"`
	Ls          LsCmd          `cmd:"" help:"List tasks"                              name:"ls" aliases:"list"`
	Ready       ReadyCmd       `cmd:"" help:"List active/open unblocked tasks"`
	Show        ShowCmd        `cmd:"" help:"Show task details"`
	Start       StartCmd       `cmd:"" help:"Start working on a task (open → active)"`
	Done        DoneCmd        `cmd:"" help:"Complete a task"`
	Reopen      ReopenCmd      `cmd:"" help:"Reopen a completed task"`
	Edit        EditCmd        `cmd:"" help:"Edit a task"`
	Log         LogCmd         `cmd:"" help:"Add a log entry to a task"`
	Block       BlockCmd       `cmd:"" help:"Add a blocker dependency"`
	Unblock     UnblockCmd     `cmd:"" help:"Remove a blocker dependency"`
	Rm          RmCmd          `cmd:"" help:"Delete a task"                           name:"rm" aliases:"remove"`
	Clean       CleanCmd       `cmd:"" help:"Remove old completed tasks"`
	Check       CheckCmd       `cmd:"" help:"Check task integrity"`
	Config      ConfigCmd      `cmd:"" help:"Show or set configuration"`
	Completions CompletionsCmd `cmd:"" help:"Output shell completions"`
	Mv          MvCmd          `cmd:"" help:"Move a task to a different project"      name:"mv"`
}

// Run sets the working directory if -C was provided.
func (c *CLI) AfterApply() error {
	if c.Dir != "" {
		return task.SetWorkingDir(c.Dir)
	}
	return nil
}

func Run(args []string) int {
	cli := &CLI{}
	ctx := kong.Parse(cli,
		kong.Name("tk"),
		kong.Description("Minimal task tracker."),
		kong.UsageOnError(),
		kong.ConfigureHelp(kong.HelpOptions{Compact: true}),
	)

	err := ctx.Run(cli)
	if err != nil {
		ctx.Errorf("%v", err)
		return 1
	}
	return 0
}
