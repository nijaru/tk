package cmd

import (
	"fmt"

	"github.com/nijaru/tk-go/internal/format"
	"github.com/nijaru/tk-go/internal/task"
)

type ConfigCmd struct {
	Show     ConfigShowCmd     `cmd:"" help:"Show configuration" default:"1"`
	Project  ConfigProjectCmd  `cmd:"" help:"Get or set the default project"`
	Alias    ConfigAliasCmd    `cmd:"" help:"Manage directory aliases for -C"`
	Defaults ConfigDefaultsCmd `cmd:"" help:"Show or set default values"`
}

type ConfigShowCmd struct{}

func (c *ConfigShowCmd) Run(cli *CLI) error {
	config := task.GetConfig()
	if cli.JSON {
		fmt.Println(format.FormatJson(config))
	} else {
		fmt.Println(format.FormatConfig(config))
	}
	return nil
}

type ConfigProjectCmd struct {
	Show   ConfigProjectShowCmd   `cmd:"" help:"Show default project" default:"1"`
	Set    ConfigProjectSetCmd    `cmd:"" help:"Set default project"`
	Rename ConfigProjectRenameCmd `cmd:"" help:"Rename project and all its tasks"`
}

type ConfigProjectShowCmd struct{}

func (c *ConfigProjectShowCmd) Run(cli *CLI) error {
	config := task.GetConfig()
	fmt.Printf("Default project: %s\n", config.Project)
	return nil
}

type ConfigProjectSetCmd struct {
	Name string `arg:"" help:"Project name to set"`
}

func (c *ConfigProjectSetCmd) Run(cli *CLI) error {
	config, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.Project = c.Name
	})
	if err != nil {
		return err
	}
	fmt.Printf("Default project set to %q\n", config.Project)
	return nil
}

type ConfigProjectRenameCmd struct {
	Old string `arg:"" help:"Old project name"`
	New string `arg:"" help:"New project name"`
}

func (c *ConfigProjectRenameCmd) Run(cli *CLI) error {
	res, err := task.RenameProject(c.Old, c.New)
	if err != nil {
		return err
	}

	fmt.Printf("Renamed project %q -> %q\n", c.Old, c.New)
	fmt.Printf("  Renamed %d tasks\n", len(res.Renamed))
	fmt.Printf("  Updated %d references\n", res.ReferencesUpdated)
	return nil
}

type ConfigAliasCmd struct {
	Name string `arg:"" optional:"" help:"Alias name"`
	Path string `arg:"" optional:"" help:"Directory path"`
	Rm   bool   `                   help:"Remove the alias" short:"r"`
}

func (c *ConfigAliasCmd) Run(cli *CLI) error {
	if c.Name != "" {
		if c.Rm {
			config, err := task.UpdateConfig(func(cfg *task.Config) {
				if cfg.Aliases == nil {
					return
				}
				delete(cfg.Aliases, c.Name)
			})
			if err != nil {
				return err
			}
			fmt.Printf("Removed alias %q\n", c.Name)
			_ = config // check aliases
			return nil
		}

		if c.Path != "" {
			config, err := task.UpdateConfig(func(cfg *task.Config) {
				if cfg.Aliases == nil {
					cfg.Aliases = make(map[string]string)
				}
				cfg.Aliases[c.Name] = c.Path
			})
			if err != nil {
				return err
			}
			fmt.Printf("Alias %q -> %q set\n", c.Name, config.Aliases[c.Name])
			return nil
		}
	}

	config := task.GetConfig()
	if len(config.Aliases) == 0 {
		fmt.Println("No aliases configured.")
		return nil
	}

	fmt.Println("Aliases:")
	for k, v := range config.Aliases {
		fmt.Printf("  %-10s -> %s\n", k, v)
	}
	return nil
}

type ConfigDefaultsCmd struct {
	// Not adding flags yet, keep it simple show-only or add a few
}

func (c *ConfigDefaultsCmd) Run(cli *CLI) error {
	config := task.GetConfig()
	fmt.Println("Defaults:")
	fmt.Printf("  Priority:  %d\n", config.Defaults.Priority)
	if len(config.Defaults.Labels) > 0 {
		fmt.Printf("  Labels:    %v\n", config.Defaults.Labels)
	}
	if len(config.Defaults.Assignees) > 0 {
		fmt.Printf("  Assignees: %v\n", config.Defaults.Assignees)
	}
	return nil
}
