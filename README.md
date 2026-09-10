# tk

Minimal task tracker for a Git checkout: one static binary, append-only JSON
records in `.tasks/`, no daemon and no database.

- Every task is a file of immutable events, so history is kept and two clones
  that touched different tasks never conflict.
- Identity never changes: a task has a ULID plus a 4-character alias, and
  `project` is just a display field.
- Appends need no lock; the store lock covers only the operations that must see
  a consistent store.

## Install

```bash
brew install nijaru/tap/tk          # Homebrew
cargo install --git https://github.com/nijaru/tk
npm install -g @nijaru/tk
```

## Quick Start

```bash
$ cd myapp
$ tk init
Initialized empty tk project in /path/to/myapp/.tasks

$ tk add "Implement auth" -p 1
Created task a7b3 (01m25qbfpr5ekbr9zxh0xc93kx)

$ tk add "Write tests" -p 2
Created task x9k2 (01m25qbfqa26kxz59mxg4va3mq)

$ tk block x9k2 a7b3                 # tests are blocked by auth
Blocked x9k2 by a7b3

$ tk ready                           # what can I work on?
REF  | PRIO | STATUS       | TITLE
----------------------------------
a7b3 | p1   | open         | Implement auth

$ tk start a7b3
Started a7b3: Implement auth

$ tk log a7b3 Using the JWT approach
Logged to a7b3: Using the JWT approach

$ tk done a7b3
Completed a7b3: Implement auth

$ tk ready                           # tests are unblocked now
REF  | PRIO | STATUS       | TITLE
----------------------------------
x9k2 | p2   | open         | Write tests
```

## Commands

| Command                     | Description                                |
| --------------------------- | ------------------------------------------ |
| `tk init`                   | Create `.tasks/` (project name from directory) |
| `tk add <title>`            | Create task                                |
| `tk list` / `tk ls`         | List tasks (hides done/closed by default)  |
| `tk ready` / `tk rdy`       | List active/open and unblocked tasks       |
| `tk show <ref>`             | Show task details                          |
| `tk start <ref>` / `active` | Start working (open → active)              |
| `tk open` / `defer` / `done` / `close` | Set status                       |
| `tk edit <ref>`             | Edit task fields                           |
| `tk checkpoint <ref> [TEXT]` | Replace the current checkpoint (`ck`; no text prints it) |
| `tk link <ref> <REF…>`      | Add document links                         |
| `tk unlink <ref> <REF…>`    | Remove document links                      |
| `tk accept <ref> [TEXT…]`   | Add or show acceptance criteria            |
| `tk evidence <ref> [TEXT…]` | Add or show completion evidence            |
| `tk log <ref> <msg>`        | Append a log entry                         |
| `tk block <ref> <blocker>`  | Add a blocking dependency                  |
| `tk unblock <ref> <blocker>` | Remove a blocking dependency              |
| `tk relate <ref> <other>`   | Record a non-blocking relationship         |
| `tk unrelate <ref> <other>` | Remove a non-blocking relationship         |
| `tk archive <ref>`          | Retire a done/closed task without deleting it |
| `tk unarchive <ref>`        | Return an archived task to active views    |
| `tk purge <ref>` / `tk rm`  | Delete a record (refuses while referenced) |
| `tk recover [ref]`          | Drop a record's torn last line             |
| `tk mv <ref> <project>`     | Change a task's project                    |
| `tk apply`                  | Apply a batch of intents from stdin        |
| `tk clean`                  | Archive old terminal tasks (default 14d; `--purge` deletes) |
| `tk check`                  | Check store integrity (non-zero on findings) |
| `tk path`                   | Print the resolved task store location     |
| `tk lock -- CMD`            | Run a command while holding the store lock |
| `tk config`                 | Show or set configuration                  |

A `<ref>` is the 4-character alias, the full ULID, or an unambiguous prefix of
one. `tk show a7b3` and `tk show 01m25qbf` both work.

## Add and Edit Options

```bash
tk add Title -p 2                  # Priority (0-4, p0-p4, or none/urgent/…)
tk add Title -P api                # Project (display grouping)
tk add Title -d "Description"
tk add Title -l bug,urgent         # Labels (CSV)
tk add Title -A nick,alice         # Assignees (CSV)
tk add Title --parent a7b3         # Parent task
tk add Title --estimate 3
tk add Title --due 2026-01-15      # or +7d / +2w / +1m

tk edit a7b3 -t "New title"
tk edit a7b3 -l +urgent            # add a label
tk edit a7b3 --remove-label bug
tk edit a7b3 --due -               # clear
tk edit a7b3 --parent -
```

## List Options

```bash
tk list database               # search title, description, alias, and ID
tk list -a                     # include done/closed
tk list -s done                # filter by status
tk list -p 1                   # filter by priority
tk list -P api                 # filter by project
tk list -l bug                 # filter by label
tk list --assignee nick
tk list --parent a7b3
tk list --roots
tk list --overdue
tk list -n 10                  # limit (default 20)
tk list --archived             # archived only
```

## Identity, Moves, and Renames

A task's ID is a ULID and never changes. Its alias is assigned once and never
changes. `project` is a display field, so `tk mv` and
`tk config project rename` change one field and rewrite no references — a
blocker or parent recorded before the move still points at the same task.

References in `blocked_by`, `parent`, and `related` hold task IDs. Human output
renders them as aliases, so `tk show` reads the way you type.

## Checkpoints, Links, and Evidence

```bash
tk checkpoint a7b3 "Parity passes; blocked on docs. Next: write the guide."
tk checkpoint a7b3                       # print the current checkpoint
tk checkpoint a7b3 --clear
tk link a7b3 agent-context/projects/x/research/y.md src/store.rs:120
tk accept a7b3 "parity test passes" "docs updated"
tk evidence a7b3 "cargo test --all-targets"
```

The checkpoint is one replaceable summary of where the work stands — current
result, blocker, next action, verification. The log stays history.
`checkpoint` and `archive` accept `--if-rev`, so a replacement written from a
stale read is refused instead of silently overwriting a newer one.

`links` holds documents. Use `tk relate` for another task.

## Archives and Deletion

`tk clean` archives old terminal tasks by default: the record stays, references
to it stay resolvable, and it drops out of `list`/`ready`. `tk list --archived`
shows the archived set and marks it `[archived]`; `-a` includes it.

`tk purge` deletes a record and refuses while other tasks still reference it,
naming them. `tk purge <ref> --scrub -f` deletes anyway and removes those
references. It never deletes without `-f` when stdin is not a terminal, so an
unattended script cannot remove a task by forgetting a flag.

## JSON Output and Batches

Every command answers `--json` with the same envelope, including failures:

```json
{"ok": true, "command": "show", "rev": "93645-s1h6:1:9c3856f1",
 "data": { … }, "issues": [], "error_code": null}
```

`error_code` is a stable kind — `not_found`, `ambiguous`, `stale_revision`,
`not_a_v1_store`, `invalid_input`, `check_failed`, `io`, `parse` — so a caller
can branch without reading prose. A failing `--json` run exits non-zero and
still writes the envelope to stdout.

`tk apply` runs a whole change under one lock, validated in full before
anything is written:

```bash
echo '{"intents":[
  {"op":"block","id":"x9k2","blocker":"a7b3"},
  {"op":"checkpoint","id":"a7b3","text":"review pending"},
  {"op":"log","id":"a7b3","msg":"wired up"},
  {"op":"status","id":"a7b3","status":"done"}]}' | tk apply --json
```

Intents are `add`, `checkpoint`, `status`, `log`, `edit`, `block`, `unblock`,
`relate`, `unrelate`, `link`, `unlink`, `accept`, `evidence`, `archive`,
`unarchive`, `mv`, and `purge`. Unknown or misspelled fields are rejected. The
batch is validated whole, so a wrong batch is rejected whole, and `--dry-run`
reports the plan without writing. It is not a cross-record transaction: an I/O
failure partway through reports how many intents had landed.

## Store Selection

Without an override, tk walks up from the working directory to the nearest
`.tasks/` or `.git`. `-C <dir>` runs that search from another directory.
`--tasks-dir <dir>` (or `TK_TASKS_DIR`) names the store exactly: no walking, and
a missing store is an error rather than a new one.

Only `tk init` creates a store, including an explicitly designated one. `tk add`
never bootstraps, so a stray command in the wrong directory reports a missing
store instead of starting a second task queue. `tk path --json` reports the
selected store, how it was selected, and whether it exists.

## Concurrency and Integrity

Records are append-only event logs. An append is a single `O_APPEND` write, so
concurrent `tk log` and label-delta writers cannot lose each other's events even
without coordination. The advisory lock on `<store>/.lock` is taken only where
one process must see a consistent store: creating a task (alias uniqueness),
adding a blocking edge (cycle check), `--if-rev` conditional writes,
multi-record operations (`purge --scrub`, project rename, `clean --purge`), and
`tk apply`.

Reads take no lock and never repair. `tk show` reports an unresolvable blocker
or parent; a missing prerequisite counts as blocking, so it never makes a
dependent look ready. `tk check` exits non-zero on any finding and prints
`{"ok":false,"issues":[…]}` with `--json`. A write interrupted before its
newline leaves a torn tail that the fold ignores and `tk recover` truncates.

`show --json` carries a `rev`. Pass it back as `--if-rev <rev>` to reject an
operation prepared against a stale read:

```bash
rev=$(tk show a7b3 --json | jq -r .data.rev)
tk edit a7b3 -t "New title" --if-rev "$rev"
```

The lock only serializes cooperating `tk` processes. A Git checkout, an editor,
or an older `tk` binary writes without it, so re-read after a pull. To include a
sync step in the same guarantee, run it under the lock; `--scan` locks every
store under a path in sorted order:

```bash
tk lock -- git pull --ff-only
tk lock --scan ~/records -- git -C ~/records pull --ff-only
```

The command must not run `tk` against a store it already locked; the lock is
advisory and not reentrant.

## Migrating a v0 Store

The previous layout (`.tasks/config.json` plus one JSON document per task) is
refused, not read. Convert it once with the script:

```bash
tools/migrate-v0.py .tasks             # add --dry-run to plan only
```

It writes `store.json` and `records/`, keeps each task's old `project-ref` ID
resolvable as a legacy alias, reuses the old ref as the new alias when it is
unique, and rewrites the dependency graph. The old files move to
`.tasks/legacy/`. The script is deliberately not a subcommand — it is meant to
be deleted after the cutover.

## Config

```bash
tk config                                  # show all config
tk config project set api                  # default project
tk config project rename old new           # rewrite every task's project field
tk config alias web src/web                # directory alias for -C
tk config defaults priority 2
tk config clean-after days 30
```

## Shell Completions

Completions and manpages are generated from the CLI spec with the
[usage CLI](https://usage.jdx.dev) (`tk __usage_spec__` prints the spec).

```bash
usage generate completion fish tk --usage-cmd "tk __usage_spec__"
usage generate completion bash tk --usage-cmd "tk __usage_spec__"
usage generate completion zsh  tk --usage-cmd "tk __usage_spec__"
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

- `-C <dir>` — run in a different directory
- `--tasks-dir <dir>` — use this store exactly (no discovery; must exist)
- `-j, --json` — output as JSON
- `-V, --version`, `-h, --help`

## Storage

```
.tasks/
  store.json              # {"format": 2, project, defaults, clean_after, aliases}
  .lock                   # mutation guard
  .gitignore              # .lock, .tmp.*
  records/<ulid>.jsonl    # one task per file, one JSON event per line
```

Each line is `{"ts", "writer", "op", "data"}`. Nothing is overwritten: the
current state is a fold over the lines, and `created` carries the full initial
state so a record is self-describing from its first line. Unknown operations
are ignored, so a newer writer's events do not break an older reader.

## Environment

- `NO_COLOR` — disable colored output
- `TK_TASKS_DIR` — task store directory (same contract as `--tasks-dir`)

## Development

```bash
cargo build
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

## License

[MIT](LICENSE)
