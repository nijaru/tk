package cmd

import (
	"fmt"
)

type CompletionsCmd struct {
	Shell string `arg:"" optional:"" help:"Shell type: bash, zsh, fish" default:"fish"`
}

func (c *CompletionsCmd) Run(cli *CLI) error {
	switch c.Shell {
	case "fish":
		fmt.Print(fishCompletion)
	case "bash":
		fmt.Print(bashCompletion)
	case "zsh":
		fmt.Print(zshCompletion)
	default:
		return fmt.Errorf("unsupported shell %q: use bash, zsh, or fish", c.Shell)
	}
	return nil
}

const fishCompletion = `# tk shell completions for fish
# Install: tk completions fish > ~/.config/fish/completions/tk.fish

set -l commands init add list ready show start open done edit log block unblock remove clean check config mv completions

complete -c tk -f
complete -c tk -n "not __fish_seen_subcommand_from $commands" -s h -l help    -d 'Show help'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -s V -l version -d 'Print version'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -s j -l json    -d 'Output as JSON'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -s C -l dir  -r -d 'Run in directory'

complete -c tk -n "not __fish_seen_subcommand_from $commands" -a init        -d 'Initialize .tasks/ in current directory'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a add         -d 'Create a task'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a list        -d 'List tasks'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a ready       -d 'List active/open unblocked tasks'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a show        -d 'Show task details'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a start       -d 'Start working on a task'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a open        -d 'Reset a task status to open'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a done        -d 'Complete a task'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a edit        -d 'Edit a task'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a log         -d 'Add a log entry to a task'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a block       -d 'Add a blocker dependency'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a unblock     -d 'Remove a blocker dependency'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a remove      -d 'Delete a task'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a clean       -d 'Remove old completed tasks'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a check       -d 'Check task integrity'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a config      -d 'Show or set configuration'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a mv          -d 'Move a task to a different project'
complete -c tk -n "not __fish_seen_subcommand_from $commands" -a completions -d 'Output shell completions'
`

const bashCompletion = `# tk shell completions for bash
# Install: tk completions bash >> ~/.bashrc  (or source directly)

_tk() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"
    local commands="init add list ready show start open done edit log block unblock remove clean check config mv completions"

    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$commands" -- "$cur"))
        return
    fi

    case "$prev" in
        completions) COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur")) ;;
        -C|--dir)    COMPREPLY=($(compgen -d -- "$cur")) ;;
    esac
}

complete -F _tk tk
`

const zshCompletion = `#compdef tk
# tk shell completions for zsh
# Install: tk completions zsh > "${fpath[1]}/_tk"

_tk() {
    local state

    _arguments \
        '(-h --help)'{-h,--help}'[Show help]' \
        '(-V --version)'{-V,--version}'[Print version]' \
        '(-j --json)'{-j,--json}'[Output as JSON]' \
        '(-C --dir)'{-C,--dir}'[Run in directory]:directory:_directories' \
        '1: :->command' \
        '*:: :->args'

    case $state in
        command)
            local -a commands=(
                'init:Initialize .tasks/ in current directory'
                'add:Create a task'
                'list:List tasks'
                'ready:List active/open unblocked tasks'
                'show:Show task details'
                'start:Start working on a task'
                'open:Reset a task status to open'
                'defer:Defer a task'
                'done:Complete a task'
                'close:Close/cancel a task'
                'edit:Edit a task'
                'log:Add a log entry to a task'
                'block:Add a blocker dependency'
                'unblock:Remove a blocker dependency'
                'remove:Delete a task'
                'clean:Remove old completed tasks'
                'check:Check task integrity'
                'config:Show or set configuration'
                'mv:Move a task to a different project'
                'completions:Output shell completions'
            )
            _describe 'command' commands ;;
        args)
            case $words[1] in
                completions) _values 'shell' bash zsh fish ;;
                -C|--dir)    _directories ;;
            esac ;;
    esac
}

_tk
`