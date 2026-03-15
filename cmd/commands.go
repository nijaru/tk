package cmd

import "fmt"

// Core command implementations are in individual files in this package.
// Subcommands and complex command structs are also moved to their respective files.

type CompletionsCmd struct {
	Shell string `arg:"" help:"Shell type (bash, zsh, fish)" enum:"bash,zsh,fish"`
}

func (c *CompletionsCmd) Run(cli *CLI) error {
	return fmt.Errorf("completions not implemented yet")
}
