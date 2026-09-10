# tk

A task tracker for a Git checkout: one static binary, one JSON document per task
in `.tasks/`, no daemon, no database, no index.

- Each task is a plain file you can read, diff, grep, and edit by hand.
- Identity is a 4-character `ref` that never changes; the filename carries it
  plus a title mnemonic, so a directory listing is readable.
- Three states, labels, and one kind of dependency. That is the whole model.

Deliberately out of scope: due dates, estimates, priority, assignment, claims,
and projects. Across 169 real tasks in the store this was built from, `priority`
was never set to anything but its default, `project` was always the store's own
name, and `due_date`, `estimate`, `parent`, `links`, and `assignees` were never
used at all. What a task needs beyond its title, state, labels, and dependencies
goes in its `status` line and its log.

## Install

```bash
brew install nijaru/tap/tk          # Homebrew
cargo install --git https://github.com/nijaru/tk
npm install -g @nijaru/tk
```

## Quick start

```bash
$ cd myapp
$ tk init
initialized /home/me/myapp/.tasks/

$ tk add "Implement auth" -l backend
55ka  Implement auth

$ tk add "Write tests" -b 55ka        # waits on the auth work
fqne  Write tests

$ tk ready                            # open, and nothing in the way
55ka | open | backend | Implement auth

1 entry
```

Everything that happens to a task is a small change to one document:

```bash
$ tk note 55ka "Using the JWT approach"
$ tk status 55ka "middleware done; needs review"
$ tk accept 55ka "parity test passes"
$ tk done 55ka
55ka  done  Implement auth

$ tk ready                            # the blocked task is unblocked now
fqne | open |                          | Write tests

1 entry

$ tk ls -a                            # everything, including finished work
55ka | done | backend                  | Implement auth
fqne | open |                          | Write tests

2 entries
```

## Commands

| Command | Description |
| ------- | ----------- |
| `tk init` | Create `.tasks/` here |
| `tk add <title>` (`new`) | Create a task (`-l` labels, `--accept`, `-b` blocker, `--status`, `-q` ref only) |
| `tk list` (`ls`) | List tasks: open by default, `-a` for everything, `-s`/`-l`/`-q` to filter |
| `tk ready` (`rdy`) | List open tasks that nothing is blocking |
| `tk show <ref>` | Show one task in full |
| `tk note <ref> <text>` (`log`) | Append a log entry |
| `tk status <ref> [text]` | Replace the current status (`--clear`) |
| `tk state <ref> <state>` | `open`, `done`, or `dropped` |
| `tk done <ref>` / `tk drop <ref>` | Shorthands for the two closing states |
| `tk label <ref> [+x\|-x\|x]` (`tag`) | Add, remove, or replace labels |
| `tk accept <ref> <criterion>` | Acceptance criteria (`--set`, `--remove`, `--clear`) |
| `tk block <ref> <blocker>` | Add a blocking dependency |
| `tk unblock <ref> <blocker>` | Remove one |
| `tk edit <ref>` | Change any field in one write (see below) |
| `tk purge <ref>` (`rm`) | Delete a task, and drop references to it |
| `tk check` (`ck`) | Check store integrity; non-zero exit on findings |
| `tk apply` | Run a batch of intents from stdin under one lock |
| `tk path` | Print the store this directory resolves to |
| `tk lock -- CMD` | Run a command while holding the store lock |
| `tk config` | Show the store, or name a directory for `-C` |

A `<ref>` is the 4-character ref or part of the title: `tk show 55ka`,
`tk show auth`, and `tk show AUTH` all work. Two matches is an error, not a
guess.

## The record

```json
{
  "ref": "55ka",
  "title": "Implement auth",
  "state": "open",
  "labels": ["backend"],
  "created": "2026-09-10T15:10:32.379590000Z",
  "updated": "2026-09-10T15:10:32.439805000Z",
  "done": null,
  "blocked_by": ["fqne"],
  "status": "middleware done; needs review",
  "acceptance": ["parity test passes"],
  "log": [
    {"ts": "2026-09-10T15:10:32.4Z", "msg": "Using the JWT approach"}
  ]
}
```

- `ref`, `title`, `state`, `created`, `updated` are required.
- `labels`, `blocked_by`, `acceptance`, and `log` are always present, possibly
  empty. `done` is always present and `null` unless the state is `done` or
  `dropped`. `status` is omitted when there is none.
- Key order is the file's shape; the parser does not care, but writing it back
  keeps that order.
- **Keys tk does not know are preserved.** Add `"assignee": "nick"` by hand and
  it survives every tk write, so a field you need today does not require a
  format change.

`status` is the current summary — result, blocker, next step. The log is what
happened, appended and never rewritten. References to files and URLs live in the
`status` or a log entry; evidence of verification is a log entry beginning
`verified:`.

## Identity and files

A ref is four characters from the Crockford base32 alphabet
(`0123456789abcdefghjkmnpqrstvwxyz`), assigned once, unique in the store, never
reused. Crockford leaves out `i`, `l`, `o`, and `u`, so a ref read off a screen
or typed from memory cannot be confused with `1` or `0`.

An entry lives at `.tasks/<ref>-<slug>.json`. The slug is made from the title
when the task is created and does not follow later title changes — renaming a
title therefore moves nothing and breaks no reference. `tk edit --slug` moves
the file on request.

## Blocking and readiness

`blocked_by` is the only constraint, and it is what makes `ready` mean
something. An entry is ready when it is open and everything it names is
finished. A blocker that names nothing in the store counts as blocking, never as
done, so a lost reference cannot make a task look startable; `tk show` and
`tk check` both say so. A blocking loop is refused when you try to create it,
with the loop printed.

Finishing a blocker is enough to unblock dependents — nothing needs updating.
`tk purge` removes references to the task it deletes, so nothing is left
permanently unready.

## JSON output

Every command answers `--json` with the same envelope, including failures:

```json
{
  "ok": true,
  "command": "show",
  "rev": "addd674e715fd5c2",
  "data": { "ref": "55ka", "title": "Implement auth", "state": "done", "…": "…" },
  "issues": [],
  "error_code": null
}
```

The keys are always present, on success and failure alike, so a caller indexes
into either without a shape check. A failing run exits non-zero and still writes
the envelope to stdout with `ok: false`.

`error_code` is a stable kind: `not_found`, `ambiguous`, `stale_revision`,
`not_a_store`, `store_not_found`, `invalid_input`, `check_failed`, `io`,
`parse`, `usage`, `error`. A human sees a sentence; a machine sees the code.
`issues` carries anything true but not fatal — an unresolvable blocker, files
the store could not read.

## Batches

`tk apply` runs many intents in order under one lock:

```bash
echo '{"intents":[
  {"op":"add","title":"Write tests","blocked_by":["55ka"]},
  {"op":"status","ref":"55ka","text":"review pending"},
  {"op":"note","ref":"55ka","message":"wired up"},
  {"op":"state","ref":"55ka","state":"done"}]}' | tk apply --json
```

Intents are `add`, `note`, `status`, `state`, `edit`, `block`, `unblock`,
`label`, `accept`, and `purge`. Each one calls exactly the same operation as the
matching command, so the two cannot disagree — a test runs both and compares the
resulting documents field by field.

A batch that cannot be parsed costs no writes: the shape of every intent is
checked first. It is **not** a transaction, though. Intents are written one at a
time in order, a failure stops the batch, and earlier intents stay applied. The
report says so, and says how many intents were not attempted.

`--dry-run` resolves the refs each intent names and reports what would happen,
writing nothing. It cannot check a constraint that depends on an earlier intent
in the same batch — a blocking loop created by two intents in one dry run is not
caught, because nothing is written for the second to see. Running it for real
does catch it, and refuses it.

## Store selection

Without an override, tk walks up from the working directory to the nearest
`.tasks/` or `.git`. `--tasks-dir <dir>` (or `TK_TASKS_DIR`) names the store
exactly: no walking, and a missing store is an error rather than a new one. Only
`tk init` creates a store, so a stray `tk add` reports a missing store instead
of starting a second task queue. `tk path` says which store was chosen and how.

A store written by an older layout is refused with the reason and the remedy
rather than read as empty:

```
$ tk ls
Error: not a tk format-3 store at /home/me/myapp/.tasks
  found store.json and records/, the v1 layout (an appended event log)
  This binary reads only format 3 stores. Convert an older store with
  tools/migrate_to_v3.py, or start a fresh one with 'tk init'.
```

## Concurrency and integrity

A change is a read-modify-write of a whole document, so every write holds an
advisory lock on `.tasks/.lock`. Read-modify-write under a lock means
concurrent `tk note`, `tk label +x`, and `tk add` from several agents all
survive: eight concurrent appends produce eight log entries, and eight
concurrent creates allocate eight distinct refs. Reads take no lock and never
wait.

`show --json` carries a `rev`, a fingerprint of the document as read. Pass it
back to refuse a change prepared against a stale read — the only defense against
two writers silently clobbering each other:

```bash
rev=$(tk show 55ka --json | jq -r .data.rev)
tk edit 55ka --title "New title" --if-rev "$rev"
```

The lock serializes cooperating `tk` processes only. An editor, a Git checkout,
or an older binary writes without it, so re-read after a pull, and use
`tk lock -- git pull --ff-only` to include a sync step in the same guarantee.

Reads report rather than repair: `tk show` names an unresolvable blocker and
`tk check` exits non-zero on any finding — an unparseable file, a filename that
disagrees with its contents, a duplicate ref, a blocking loop, a state that
disagrees with its `done` time. A hand edit that breaks the JSON is reported by
name and line, not silently skipped.

## Migrating from v0 or v1

Older layouts are refused, not read. Convert one once:

```bash
tools/migrate_to_v3.py .tasks            # add --dry-run to plan only
tk check                                 # expect no findings
```

It handles both older layouts — v0 (`config.json` plus one document per task)
and v1 (`store.json` plus `records/<ulid>.jsonl` event logs, including the
event fold) — reuses a legacy ref as the new ref when it is valid and free,
remaps `blocked_by`, turns `checkpoint` into `status` and `evidence` into
`verified:` log entries, writes `MIGRATION.md` mapping every old handle to its
new ref, and moves the consumed files into `.tasks/legacy/` rather than deleting
them. Dropped fields are counted and reported by name. It is deliberately not a
subcommand: it runs once per store and then has no reason to exist.

**Stop older writers first.** An older `tk` sees zero tasks in a v2 store and
will write its own files beside them; the v2 side refuses the older layout, the
older side does not notice.

## Config

```bash
tk config                        # the store, its format, its entry count, its aliases
tk config alias web src/web      # name a directory so -C web finds it
tk config alias web              # forget it
```

`.tk.json` holds the format version and those aliases:

```json
{
  "format": 3,
  "aliases": {"web": "src/web"}
}
```

## Shell completions

Completions and manpages are generated from the CLI spec with the
[usage CLI](https://usage.jdx.dev) (`tk __usage_spec__` prints the spec).

```bash
usage generate completion fish tk --usage-cmd "tk __usage_spec__"
usage generate completion bash tk --usage-cmd "tk __usage_spec__"
usage generate completion zsh  tk --usage-cmd "tk __usage_spec__"
tk __usage_spec__ | usage generate manpage -f -
```

## Storage

```
.tasks/
  .tk.json                        # {"format": 3, "aliases": {...}}
  .lock                           # the mutation lock
  .gitignore                      # .lock, .tmp.*
  55ka-implement-auth.json        # one task per file
  legacy/                         # whatever a migration moved aside
  MIGRATION.md                    # old handles -> refs, if migrated
```

There is no index and nothing derived: `list` reads every document, which
measured 20ms for 161 entries on a laptop. An index only becomes worth its
synchronization cost somewhere in the low thousands of tasks, and there is no
measurement yet that says otherwise.

## Environment

- `NO_COLOR` — disable colored output
- `TK_TASKS_DIR` — task store directory (same contract as `--tasks-dir`)

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=3
```

The concurrency tests spawn eight processes each, so the limited thread count
keeps a busy machine clear of the per-user process limit. `tests/migration.rs`
needs `python3` and skips itself when it is missing.

## License

[MIT](LICENSE)
