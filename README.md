# tk

Minimal task tracker. Simple, fast, git-friendly.

- Plain JSON files in `.tasks/`
- One advisory lock per mutation, so concurrent writers do not lose updates
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
| `tk checkpoint <id> [TEXT]` | Replace the current checkpoint (`ck`; no text shows it) |
| `tk link <id> <REF…>`       | Add links to research, decisions, or source |
| `tk unlink <id> <REF…>`     | Remove links                               |
| `tk accept <id> [TEXT…]`    | Add or show acceptance criteria            |
| `tk evidence <id> [TEXT…]`  | Add or show completion evidence            |
| `tk log <id> <msg>`         | Add log entry                              |
| `tk block <id> <blocker>`   | Add dependency (id blocked by blocker)     |
| `tk unblock <id> <blocker>` | Remove dependency                          |
| `tk archive <id>`           | Retire a done/closed task without deleting it |
| `tk unarchive <id>`         | Return an archived task to active views    |
| `tk remove` / `tk rm <id>`  | Delete task (prompts for confirmation)     |
| `tk repair <id>`            | Fix recorded inconsistencies in a task file |
| `tk mv <id> <project>`      | Move task to a different project           |
| `tk clean`                  | Archive old terminal tasks (default: 14d; `--purge` deletes) |
| `tk check`                  | Check task integrity (non-zero on findings) |
| `tk path`                   | Print the resolved task store location     |
| `tk lock -- CMD`            | Run a command while holding the store lock |
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
tk list --archived             # Archived tasks only (implies terminal)
```

## Checkpoint, Links, Acceptance, Evidence

```bash
tk checkpoint a7b3 "Parity passes; blocked on docs. Next: write the guide."
tk checkpoint a7b3                       # print the current checkpoint
tk checkpoint a7b3 --clear
tk link a7b3 agent-context/projects/x/research/y.md src/store.rs:120
tk accept a7b3 "parity test passes" "docs updated"
tk evidence a7b3 "cargo test --all-targets"
```

The checkpoint is one replaceable summary of where the work stands — current
result, blocker, next action, verification. It never replaces the log, which
stays append-only history. `checkpoint` and `archive` accept `--if-rev`, so a
replacement written from a stale read is refused instead of silently
overwriting a newer one.

## Archives and Stable References

`tk clean` archives old terminal tasks by default: the record stays, references
to it stay resolvable, and it drops out of `list`/`ready`. `tk list --archived`
shows the archived set and marks it `[archived]`; `-a` includes it.
`tk clean --purge` keeps the old destructive behavior and scrubs references.

`tk mv` and `tk config project rename` still change IDs, but the previous ID is
recorded in the task's `previous_ids`, so `tk show <old-id>` and blockers or
parents written before the move keep resolving. `tk check` reports a
`previous_ids` entry that collides with a live task ID.

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
- `--tasks-dir <dir>` — Use this task store directory exactly (no discovery; must exist)
- `-j, --json` — Output as JSON
- `-V, --version` — Show version
- `-h, --help` — Show help

## Store Selection

Without an override, tk walks up from the working directory to the nearest
`.tasks/` or `.git`. `-C <dir>` runs that search from another directory.
`--tasks-dir <dir>` (or `TK_TASKS_DIR`) names the store exactly: no walking, and
a missing store is an error rather than a new one. `tk init` is the deliberate
exception — it creates the store, including an explicitly designated one.

`tk init` errors when the directory already holds a store. `tk add` bootstraps a
store for a plain directory or normal checkout, but refuses inside a linked
worktree or submodule (where `.git` is a file) so it cannot grow a second task
queue in a worker checkout; use `tk init` or point `--tasks-dir` at the shared
store. `tk path --json` reports the selected store, how it was selected, and
whether it exists.

## Concurrency and Integrity

Every mutation (add, edit, log, block, status change, move, remove, clean)
runs inside one advisory lock on `<store>/.lock`, covering resolve → read →
validate → edit → persist. Concurrent `tk` writers serialize instead of
overwriting each other. Read-only commands take no lock and never repair:
`tk show` reports an unresolvable blocker or parent, and only
`tk repair <id> [--drop-missing]` changes the record. A missing prerequisite
counts as blocking, so it never makes a dependent look ready. `tk check` exits
non-zero when it finds anything, and prints `{"ok":false,"issues":[...]}` with
`--json`.

`list --json` and `show --json` include a `rev` fingerprint. Pass it back as
`--if-rev <rev>` on `edit` or `remove` to reject an operation prepared against a
stale read:

```bash
rev=$(tk show a7b3 --json | jq -r .rev)
tk edit a7b3 -t "New title" --if-rev "$rev"
```

The lock only serializes cooperating `tk` processes. A Git checkout, an editor,
or an older `tk` binary writes without it, so re-read after a pull. To include a
sync step in the same guarantee, run it under the lock; `--scan` locks every
store under a path in sorted order, which is what a multi-store records
checkout needs:

```bash
tk lock -- git pull --ff-only
tk lock --scan ~/records -- git -C ~/records pull --ff-only
```

The command must not run `tk` against a store it already locked; the lock is
advisory and not reentrant. Locks and temp files are covered by the `.gitignore`
tk writes inside `.tasks/` when it creates a store.

## Environment

- `NO_COLOR` — Disable colored output
- `TK_TASKS_DIR` — Task store directory (same contract as `--tasks-dir`)

## Storage

Plain JSON files in `.tasks/` — one file per task, one config file, one `.lock`
for the mutation guard. Task JSON carries the core fields plus optional
`checkpoint`, `links`, `acceptance`, `evidence`, `previous_ids`, and
`archived_at`; older readers ignore them.

## License

[MIT](LICENSE)
