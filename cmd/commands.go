package cmd

import "fmt"

// Stub command structs — implementation in subsequent tasks.

// Core implementations in separate files.


type StartCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *StartCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type DoneCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *DoneCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type ReopenCmd struct {
	ID string `arg:"" help:"Task ID or ref"`
}

func (c *ReopenCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

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
	return fmt.Errorf("not implemented")
}

type LogCmd struct {
	ID  string `arg:"" help:"Task ID or ref"`
	Msg string `arg:"" help:"Log message"`
}

func (c *LogCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type BlockCmd struct {
	ID      string `arg:"" help:"Task ID or ref to block"`
	Blocker string `arg:"" help:"Blocking task ID or ref"`
}

func (c *BlockCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type UnblockCmd struct {
	ID      string `arg:"" help:"Task ID or ref"`
	Blocker string `arg:"" help:"Blocking task ID or ref to remove"`
}

func (c *UnblockCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type RmCmd struct {
	ID    string `arg:"" help:"Task ID or ref"`
	Force bool   `       help:"Skip confirmation" short:"f"`
}

func (c *RmCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type CleanCmd struct {
	OlderThan int  `help:"Remove tasks completed more than N days ago" default:"0"`
	Force     bool `help:"Force clean even if disabled in config"`
}

func (c *CleanCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type CheckCmd struct{}

func (c *CheckCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type ConfigCmd struct {
	Project  ConfigProjectCmd  `cmd:"" help:"Get or set the default project"`
	Alias    ConfigAliasCmd    `cmd:"" help:"Manage directory aliases for -C"`
	Defaults ConfigDefaultsCmd `cmd:"" help:"Show or set default values"`
}

// ConfigCmd with no subcommand shows all config.
func (c *ConfigCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type ConfigProjectCmd struct {
	Name   string `arg:"" optional:"" help:"Project name to set"`
	Rename string `                   help:"Rename: tk config project rename <old> <new>"`
}

func (c *ConfigProjectCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type ConfigProjectRenameCmd struct {
	Old string `arg:"" help:"Old project name"`
	New string `arg:"" help:"New project name"`
}

func (c *ConfigProjectRenameCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type ConfigAliasCmd struct {
	Name string `arg:"" optional:"" help:"Alias name"`
	Path string `arg:"" optional:"" help:"Directory path"`
	Rm   bool   `                   help:"Remove the alias"`
}

func (c *ConfigAliasCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type ConfigDefaultsCmd struct{}

func (c *ConfigDefaultsCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type CompletionsCmd struct {
	Shell string `arg:"" help:"Shell type (bash, zsh, fish)" enum:"bash,zsh,fish"`
}

func (c *CompletionsCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}

type MvCmd struct {
	ID      string `arg:"" help:"Task ID or ref"`
	Project string `arg:"" help:"Target project name"`
}

func (c *MvCmd) Run(cli *CLI) error {
	return fmt.Errorf("not implemented")
}
