# tk

Minimal task tracker. Simple, fast, git-friendly.

- Plain JSON files in `.tasks/`
- No daemons, no merge conflicts, no viruses
- Single binary, no runtime dependency

## Install

### Homebrew

```bash
brew install nijaru/tap/tk
```

### Cargo

```bash
cargo install --git https://github.com/nijaru/tk
```

### npm

```bash
npm install -g @nijaru/tk
```

### Build from source

```bash
git clone https://github.com/nijaru/tk
cd tk
cargo build --release
```

## Quick Start

```bash
$ cd myapp
$ tk init                               # project auto-derived from directory
Initialized empty tk project in /path/to/myapp/.tasks

$ tk add Implement auth -p 1
Created task myapp-a7b3: Implement auth

$ tk add Write tests -p 2
Created task myapp-x9k2: Write tests

$ tk block x9k2 a7b3                    # tests blocked by auth
Blocked myapp-x9k2 by myapp-a7b3

$ tk ready                              # what can I work on?
ID          | PRIO | STATUS       | TITLE
-----------------------------------------------------------------
myapp-a7b3  | p1   | open         | Implement auth

$ tk start a7b3                         # or tk active a7b3
Started myapp-a7b3: Implement auth

$ tk log a7b3 Using JWT approach
Logged to myapp-a7b3: Using JWT approach

$ tk done a7b3
Completed myapp-a7b3: Implement auth

$ tk ready                              # tests now unblocked
ID          | PRIO | STATUS       | TITLE
-----------------------------------------------------------------
myapp-x9k2  | p2   | open         | Write tests
```

## Commands

| Command                     | Description                                |
| --------------------------- | ------------------------------------------ |
| `tk init`                   | Initialize (project name from directory)   |
| `tk add <title>`            | Create task                                |
| `tk list` / `tk ls`         | List tasks (hides done/closed by default)  |
| `tk ls <query>`             | Search tasks by title or description       |
| `tk ready` / `tk rdy`       | List active/open + unblocked tasks         |
| `tk show <id>`              | Show task details                          |
| `tk start <id>` / `active`  | Start working (open → active)              |
| `tk open <id>`              | Reset task status to open                  |
| `tk defer <id>`             | Defer task                                 |
| `tk done <id>`              | Complete task                              |
| `tk close <id>`             | Close/cancel task                          |
| `tk edit <id>`              | Edit task                                  |
| `tk log <id> <msg>`         | Add log entry                              |
| `tk block <id> <blocker>`   | Add dependency (id blocked by blocker)     |
| `tk unblock <id> <blocker>` | Remove dependency                          |
| `tk remove` / `tk rm <id>`  | Delete task (prompts for confirmation)     |
| `tk mv <id> <project>`      | Move task to a different project           |
| `tk clean`                  | Remove old terminal tasks (default: 14d)   |
| `tk check`                  | Check task integrity                       |
| `tk config`                 | Show/set configuration                     |

## Add Options

```bash
tk add Title -p 2                  # Priority (0-4)
tk add Title -P api                # Project prefix
tk add Title -d "Description"      # Description
tk add Title -l bug,urgent         # Labels (CSV)
tk add Title -A nick,alice         # Assignees (CSV)
tk add Title --parent a7b3         # Parent task
tk add Title --estimate 3          # Estimate (user-defined units)
tk add Title --due 2026-01-15      # Due date (YYYY-MM-DD)
tk add Title --due +7d             # Relative due date (+Nh/+Nd/+Nw/+Nm)
```

## List Options

```bash
tk list                        # List active/open/deferred tasks (limit 20)
tk list database               # Search tasks for 'database'
tk list -a                     # Show all (including done/closed)
tk list -s done                # Filter by status
tk list -p 1                   # Filter by priority
tk list -P api                 # Filter by project
tk list -l bug                 # Filter by label
tk list --assignee nick        # Filter by assignee
tk list --parent a7b3          # Filter by parent
tk list --roots                # Top-level tasks only
tk list --overdue              # Overdue tasks only
tk list -n 10                  # Limit results
```

## Edit Options

```bash
tk edit a7b3 -t "New title"    # Update title
tk edit a7b3 -p 1              # Update priority
tk edit a7b3 -l +urgent        # Add label
tk edit a7b3 --remove-label bug # Remove label
tk edit a7b3 --remove-assignee nick # Remove assignee
tk edit a7b3 --due -           # Clear due date
tk edit a7b3 --parent -        # Clear parent
```

## Config

```bash
tk config                                  # Show all config
tk config project                          # Show default project
tk config project set api                  # Set default project (lowercase, digits, hyphens)
tk config project rename old new           # Rename project and all its tasks
tk config alias                            # List aliases
tk config alias web src/web                # Add alias
tk config alias web --rm                   # Remove alias
```

## Shell Completions

Completions are generated from the CLI spec with the
[usage CLI](https://usage.jdx.dev) (`tk __usage_spec__` prints the spec).

```bash
# Fish (add --install to write the file where your shell looks for it)
usage generate completion fish tk --usage-cmd "tk __usage_spec__"

# Bash / Zsh (same pattern, any supported shell)
usage generate completion bash tk --usage-cmd "tk __usage_spec__"
usage generate completion zsh tk --usage-cmd "tk __usage_spec__"
```

A manpage renders the same way:

```bash
tk __usage_spec__ | usage generate manpage -f -
```

## Priority

| Value | Name   | Description      |
| ----- | ------ | ---------------- |
| 0     | none   | No priority set  |
| 1     | urgent | Drop everything  |
| 2     | high   | Important        |
| 3     | medium | Normal (default) |
| 4     | low    | Nice to have     |

## Global Options

- `-C <dir>` — Run in different directory
- `-j, --json` — Output as JSON
- `-V, --version` — Show version
- `-h, --help` — Show help

## Environment

- `NO_COLOR` — Disable colored output

## Storage

Plain JSON files in `.tasks/` — one file per task, one config file.

## License

[MIT](LICENSE)
