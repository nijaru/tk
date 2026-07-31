package cmd

import (
	"fmt"

	"github.com/nijaru/tk/internal/format"
	"github.com/nijaru/tk/internal/task"
)

type ConfigCmd struct {
	Show     ConfigShowCmd       `cmd:"" help:"Show configuration"              default:"1"`
	Project  ConfigProjectCmd    `cmd:"" help:"Get or set the default project"`
	Alias    ConfigAliasCmd      `cmd:"" help:"Manage directory aliases for -C"`
	Defaults ConfigDefaultsCmd   `cmd:"" help:"Show or set default values"`
	Clean    ConfigCleanAfterCmd `cmd:"" help:"Configure auto-cleanup"                      name:"clean-after"`
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
	Show   ConfigProjectShowCmd   `cmd:"" help:"Show default project"             default:"1"`
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
	if err := task.ValidateProjectName(c.Name); err != nil {
		return err
	}

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
	Show      ConfigDefaultsShowCmd      `cmd:"" help:"Show default values"   default:"1"`
	Priority  ConfigDefaultsPriorityCmd  `cmd:"" help:"Set default priority"`
	Labels    ConfigDefaultsLabelsCmd    `cmd:"" help:"Set default labels"`
	Assignees ConfigDefaultsAssigneesCmd `cmd:"" help:"Set default assignees"`
}

type ConfigDefaultsShowCmd struct{}

func (c *ConfigDefaultsShowCmd) Run(cli *CLI) error {
	config := task.GetConfig()
	fmt.Println("Defaults:")
	fmt.Printf("  Priority:  %d\n", config.Defaults.Priority)
	fmt.Printf("  Labels:    %v\n", config.Defaults.Labels)
	fmt.Printf("  Assignees: %v\n", config.Defaults.Assignees)
	return nil
}

type ConfigDefaultsPriorityCmd struct {
	Level int `arg:"" help:"Default priority level (0-4)"`
}

func (c *ConfigDefaultsPriorityCmd) Run(cli *CLI) error {
	if c.Level < 0 || c.Level > 4 {
		return fmt.Errorf("priority must be 0-4")
	}
	_, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.Defaults.Priority = task.Priority(c.Level)
	})
	return err
}

type ConfigDefaultsLabelsCmd struct {
	Labels []string `arg:"" help:"Default labels (CSV)" sep:","`
}

func (c *ConfigDefaultsLabelsCmd) Run(cli *CLI) error {
	_, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.Defaults.Labels = c.Labels
	})
	return err
}

type ConfigDefaultsAssigneesCmd struct {
	Assignees []string `arg:"" help:"Default assignees (CSV)" sep:","`
}

func (c *ConfigDefaultsAssigneesCmd) Run(cli *CLI) error {
	_, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.Defaults.Assignees = c.Assignees
	})
	return err
}

type ConfigCleanAfterCmd struct {
	Show    ConfigCleanAfterShowCmd    `cmd:"" help:"Show clean-after config" default:"1"`
	Enable  ConfigCleanAfterEnableCmd  `cmd:"" help:"Enable auto-clean"`
	Disable ConfigCleanAfterDisableCmd `cmd:"" help:"Disable auto-clean"`
	Days    ConfigCleanAfterDaysCmd    `cmd:"" help:"Set clean-after days"`
}

type ConfigCleanAfterShowCmd struct{}

func (c *ConfigCleanAfterShowCmd) Run(cli *CLI) error {
	config := task.GetConfig()
	status := "disabled"
	if config.CleanAfter.Enabled {
		status = "enabled"
	}
	fmt.Printf("Clean After: %s (%d days)\n", status, config.CleanAfter.Days)
	return nil
}

type ConfigCleanAfterEnableCmd struct{}

func (c *ConfigCleanAfterEnableCmd) Run(cli *CLI) error {
	_, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.CleanAfter.Enabled = true
	})
	return err
}

type ConfigCleanAfterDisableCmd struct{}

func (c *ConfigCleanAfterDisableCmd) Run(cli *CLI) error {
	_, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.CleanAfter.Enabled = false
	})
	return err
}

type ConfigCleanAfterDaysCmd struct {
	Days int `arg:"" help:"Days after which to clean completed tasks"`
}

func (c *ConfigCleanAfterDaysCmd) Run(cli *CLI) error {
	if c.Days < 0 {
		return fmt.Errorf("days must be >= 0")
	}
	_, err := task.UpdateConfig(func(cfg *task.Config) {
		cfg.CleanAfter.Days = c.Days
	})
	return err
}
