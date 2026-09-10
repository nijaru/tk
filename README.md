# tk

A task tracker for a Git checkout: one static binary, one JSON document per task
in `.tasks/`, no daemon, no database, no index, no configuration.

- Each task is a plain file you can read, diff, grep, and edit by hand.
- Identity is a 4-character `ref` that never changes; the filename carries it
  plus a title mnemonic, so a directory listing is readable.
- Three states, labels, one append-only log, and one kind of dependency. That is
  the whole model.

Deliberately out of scope: due dates, estimates, priority, assignment, claims,
and projects. Across the 169 real tasks this was built from, `priority` was
never set to anything but its default, `project` was always the store's own
name, and due dates, estimates, assignees, parents, and attachments were never
used. A replaceable status field and a list of acceptance criteria were also cut
after the same measurement came back empty — the log is the record, and its last
entry is the current state of play.

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

$ tk                                  # no arguments: what can I start?
55ka | open | backend | Implement auth

1 entry
```

Everything that happens to a task is a small change to one document:

```bash
$ tk note 55ka "Using the JWT approach"
$ tk done 55ka
55ka  done  Implement auth

$ tk ready                            # the blocked task is unblocked now
fqne | open |                          | Write tests

1 entry

$ tk ls -a                            # everything, including finished work
55ka | done | backend                  | Implement auth
fqne | open |                          | Write tests

2 entries

$ tk open 55ka                        # reopening forgets the completion time
```

## Commands

| Command | Description |
| ------- | ----------- |
| `tk` | Same as `tk ready`: what can be started now |
| `tk init` | Create `.tasks/` here |
| `tk add <title>` (`new`) | Create a task (`-l` labels, `-b` blocker, `-q` ref only) |
| `tk list` (`ls`) | List tasks: open by default, `-a` for everything, `-s`/`-l`/`-q` to filter |
| `tk ready` (`rdy`) | List open tasks that nothing is blocking |
| `tk show <ref>` | Show one task: its fields, blockers, and log |
| `tk note <ref> <text>` (`log`) | Append a log entry |
| `tk done <ref>` / `tk drop <ref>` / `tk open <ref>` | Close it, abandon it, reopen it |
| `tk label <ref> +x -y` (`tag`) | Add and remove labels |
| `tk block <ref> <blocker>` / `tk unblock <ref> <blocker>` | Add or remove a dependency |
| `tk edit <ref>` | Change fields, labels, or blockers in one write |
| `tk purge <ref>` (`rm`) | Delete a task, and drop references to it |
| `tk check` (`ck`) | Check store integrity; non-zero exit on findings |
| `tk apply` | Run a batch of intents from stdin under one lock |
| `tk path` | Print the store this directory resolves to |

A `<ref>` is the 4-character ref or part of the title: `tk show 55ka`,
`tk show auth`, and `tk show AUTH` all work. Two matches is an error, not a
guess.

State is three verbs, not a value you can misspell: `done`, `drop`, and `open`.
Any other state is unrepresentable from the command line, and a batch that asks
for one is refused before anything is written.

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
  "log": [
    {"ts": "2026-09-10T15:10:32.4Z", "msg": "Using the JWT approach"}
  ]
}
```

- `ref`, `title`, `state`, `created`, `updated` are required.
- `labels`, `blocked_by`, and `log` are always present, possibly empty. `done` is
  always present and `null` unless the state is `done` or `dropped`.
- Key order is the file's shape; the parser does not care, but writing it back
  keeps that order.
- **Keys tk does not know are preserved.** Add `"assignee": "nick"` by hand and
  it survives every tk write, so a field you need today does not require a
  format change.

The log is the history and it is only ever appended to. Where things stand is
its last entry, which cannot go stale and cannot disagree with itself. Evidence
of verification is a log entry beginning `verified:`; a document or a URL is
named in a log entry, and long-form documents can live in a subdirectory beside
the entries — `.tasks/reports/<ref>-notes.md`, say — which is what `check`
leaves alone on purpose.

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

## Labels

Labels change by delta: `+x` adds, `-x` removes. A bare label is refused, with a
message saying so, because the bare form reads like an add and behaves like a
replacement of whatever was there — the one way a stray command silently discards
someone else's work. Replacing the whole set is `tk edit --label a,b`, which is
what you type when that is what you mean.

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
`issues` carries anything true but not fatal — an entry that could not be read,
a blocker that names nothing — and a human sees those on stderr while the exit
code stays zero, because the read itself succeeded.

## Batches

`tk apply` runs many intents in order under one lock:

```bash
echo '{"intents":[
  {"op":"add","title":"Write tests","blocked_by":["55ka"]},
  {"op":"note","ref":"55ka","message":"wired up"},
  {"op":"state","ref":"55ka","state":"done"}]}' | tk apply --json
```

One shape: an object with an `intents` array. Intents are `add`, `note`,
`state`, `edit`, `block`, `unblock`, `label`, and `purge`. Each one calls exactly
the same operation as the matching command, so the two cannot disagree — a test
runs both and compares the resulting documents field by field.

A batch that cannot be parsed costs no writes: the shape of every intent is
checked first, and a value no state can hold (say `"state": "active"`) refuses
the whole batch before the first intent runs. It is **not** a transaction,
though. Intents are written one at a time in order, a failure stops the batch,
and earlier intents stay applied. The report says so, and says how many intents
were not attempted.

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
advisory lock on `.tasks/.lock`. Read-modify-write under a lock means concurrent
`tk note`, `tk label +x`, and `tk add` from several agents all survive: eight
concurrent appends produce eight log entries, and eight concurrent creates
allocate eight distinct refs. Reads take no lock and never wait.

`show --json` carries a `rev`, a fingerprint of the document as read. Pass it
back to refuse a change prepared against a stale read — the only defense against
two writers silently clobbering each other:

```bash
rev=$(tk show 55ka --json | jq -r .data.rev)
tk note 55ka "after that pull" --if-rev "$rev"
```

The lock serializes cooperating `tk` processes only. An editor, a Git checkout,
or an older binary writes without it, so re-read after a pull.

Reads report rather than repair. `tk show` names an unresolvable blocker, `tk ls`
says on stderr which files it could not read, and `tk check` exits non-zero on
any finding — an unparseable file, a filename that disagrees with its contents, a
duplicate ref, a blocking loop, a state that disagrees with its `done` time, or a
file in the store that tk did not write (debris from an interrupted write, say).
Directories are ignored: tk writes files, so a directory beside them is yours.

## Migrating from v0 or v1

Older layouts are refused, not read. Convert one once:

```bash
tools/migrate_to_v3.py .tasks            # add --dry-run to plan only
tk check                                 # expect no findings
```

It handles both older layouts — v0 (one document per task, with or without the
old `config.json`)
and v1 (`store.json` plus `records/<ulid>.jsonl` event logs, including the event
fold) — reuses a legacy ref as the new ref when it is valid and free, remaps
`blocked_by`, and writes `MIGRATION.md` mapping every old handle to its new ref.
Fields that v3 does not have become marked log entries rather than being
dropped: `checkpoint` becomes `status: …`, acceptance criteria become
`acceptance: …`, and evidence becomes `verified: …`. Consumed files move to
`.tasks/legacy/` rather than being deleted, and fields with no equivalent at all
(`description`, `priority`, `project`, due dates, estimates, assignees, parents)
are counted and reported by name. It is deliberately not a subcommand: it runs
once per store and then has no reason to exist.

**Stop older writers first.** An older `tk` sees zero tasks in a v3 store and
will write its own files beside them; the v3 side refuses the older layout, the
older side does not notice.

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
  .tk.json                        # {"format": 3}
  .lock                           # the mutation lock
  .gitignore                      # .lock, .tmp.*
  55ka-implement-auth.json        # one task per file
  reports/                        # optional: documents a task refers to
  legacy/                         # whatever a migration moved aside
  MIGRATION.md                    # old handles -> refs, if migrated
```

The store file carries the layout version and nothing else: there is nothing to
configure, and a settings file that exists to hold defaults is a place for
defaults to hide.

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
