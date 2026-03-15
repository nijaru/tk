package cmd

import (
	"github.com/alecthomas/kong"
)

// Core command implementations are in individual files in this package.
// Subcommands and complex command structs are also moved to their respective files.

type CompletionsCmd struct{}

func (c *CompletionsCmd) Run(ctx *kong.Context) error {
	ctx.PrintUsage(false)
	return nil
}
