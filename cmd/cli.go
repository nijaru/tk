package cmd

import (
	"github.com/alecthomas/kong"
	"github.com/nijaru/tk/internal/task"
)

// CLI is the root Kong command struct.
type CLI struct {
	// Global flags
	Version kong.VersionFlag `short:"V" name:"version" help:"Print version and exit"`
	JSON    bool             `short:"j"                help:"Output as JSON"`
	Dir     string           `short:"C"                help:"Run in directory"       default:""`

	// Commands
	Init        InitCmd        `cmd:"" help:"Initialize .tasks/ in current directory"`
	Add         AddCmd         `cmd:"" help:"Create a task"`
	List        LsCmd          `cmd:"" help:"List tasks"                                 aliases:"ls"`
	Ready       ReadyCmd       `cmd:"" help:"List active/open unblocked tasks"           aliases:"rdy"`
	Show        ShowCmd        `cmd:"" help:"Show task details"`
	Start       StartCmd       `cmd:"" help:"Start working on a task (open → active)"`
	Done        DoneCmd        `cmd:"" help:"Complete a task"`
	Reopen      ReopenCmd      `cmd:"" help:"Reopen a completed task"`
	Edit        EditCmd        `cmd:"" help:"Edit a task"`
	Log         LogCmd         `cmd:"" help:"Add a log entry to a task"`
	Block       BlockCmd       `cmd:"" help:"Add a blocker dependency"`
	Unblock     UnblockCmd     `cmd:"" help:"Remove a blocker dependency"`
	Remove      RmCmd          `cmd:"" help:"Delete a task"                              aliases:"rm"`
	Clean       CleanCmd       `cmd:"" help:"Remove old completed tasks"`
	Check       CheckCmd       `cmd:"" help:"Check task integrity"`
	Config      ConfigCmd      `cmd:"" help:"Show or set configuration"`
	Completions CompletionsCmd `cmd:"" help:"Output shell completions (bash, zsh, fish)"`
	Mv          MvCmd          `cmd:"" help:"Move a task to a different project"`
}

// AfterApply sets the working directory if -C was provided.
func (c *CLI) AfterApply() error {
	if c.Dir != "" {
		return task.SetWorkingDir(c.Dir)
	}
	return nil
}

func Run(args []string, version string) int {
	if len(args) == 0 {
		args = []string{"--help"}
	}
	cli := &CLI{}
	parser, err := kong.New(cli,
		kong.Name("tk"),
		kong.Description("Minimal task tracker."),
		kong.UsageOnError(),
		kong.ConfigureHelp(kong.HelpOptions{Compact: true, NoExpandSubcommands: true}),
		kong.Vars{"version": version},
	)
	if err != nil {
		panic(err)
	}

	ctx, err := parser.Parse(args)
	if err != nil {
		parser.Errorf("%v", err)
		return 1
	}

	if err := ctx.Run(cli); err != nil {
		parser.Errorf("%v", err)
		return 1
	}
	return 0
}
