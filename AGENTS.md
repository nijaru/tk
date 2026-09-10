# tk

Minimal task tracker CLI — one JSON document per task in `.tasks/`, single
binary, no runtime, no database, no index, no configuration.

## Project Structure

| Directory / File   | Purpose                                                        |
| ------------------ | -------------------------------------------------------------- |
| `src/main.rs`      | Binary entry — calls `tk::cli::run()`                          |
| `src/lib.rs`       | Library root (all modules `pub` for integration tests)         |
| `src/cli.rs`       | Root `Cli` derive, global flags (`-j/--json`, `-C/--dir`), the subcommand list, manual `RunWith` for the bare `tk`, error-code recovery |
| `src/commands.rs`  | `AppCtx` and the `emit`/`fail` pair every command reports through |
| `src/commands/`    | One module per command group: `add.rs` (Init/Add), `list.rs` (List/Ready/Show), `edit.rs` (Note/Done/Drop/Open/Label/Block/Unblock/Edit), `misc.rs` (Purge/Check/Path), `batch.rs` |
| `src/model.rs`     | `Entry` (the document: 9 keys), `EntryView` (entry + `rev` + derived blockers), `State`, `LogEntry`, `Config` (the format number) |
| `src/ops.rs`       | Every change, once. Commands and `apply` both call these; nothing else writes an entry. |
| `src/store.rs`     | `Ctx` (location + format gate), `Store` (reads), `Txn` (the only writer), `Filter`, `check_integrity` |
| `src/ids.rs`       | `ref` generation, slugs, filenames, reference resolution       |
| `src/apply.rs`     | `tk apply`: intent parsing, whole-batch validation, dispatch to `ops` |
| `src/output.rs`    | The JSON envelope and its stable error codes                   |
| `src/format.rs`    | Table, detail view, JSON, color, truncation                    |
| `src/timeutil.rs`  | RFC3339Nano stamps and their rendering                         |
| `tests/cli.rs`     | Help-drift snapshot + end-to-end, error-code, and concurrency tests |
| `tests/migration.rs` | Runs `tools/migrate_to_v3.py` against v0 and v1 fixtures     |
| `tools/migrate_to_v3.py` | One-shot v0/v1 → v3 conversion; not a subcommand, delete after cutover |
| `.local/`          | Clone-local working notes — excluded via `.git/info/exclude`   |
| `.tasks/`          | Local-only task state — excluded via `.git/info/exclude`       |

This repository carries `AGENTS.md` only. It deliberately has no `CLAUDE.md`
symlink and no `ai/` symlink into the knowledge store: the central knowledge
store is read and written through `context`, not through a path inside the
checkout.

## Technology Stack

| Component  | Technology                                      |
| ---------- | ----------------------------------------------- |
| Language   | Rust 2024 edition                               |
| CLI        | [usage-rs](https://usage.jdx.dev) v6 (derive)   |
| Errors     | `miette` (fancy) + `thiserror`                  |
| Colors     | `owo-colors` (NO_COLOR support)                 |
| Testing    | `cargo test` + `assert_cmd`/`predicates`/`insta`|

## CLI Notes (usage-rs v6)

- Root: `#[derive(Cli)]` + `#[usage(bin, version)]`; the command enum is
  `#[derive(Subcommands)]` + `#[usage(run_with)]`, and each variant holds an
  `Args` struct implementing `RunWith<AppCtx>` with an explicit
  `type Output = miette::Result<()>`.
- The root does **not** take `run_with`: `command` is an `Option<Commands>`, and
  usage-rs refuses to generate a forwarder for an optional subcommand because
  nothing generated can decide what "no subcommand" means. `cli.rs` implements
  `RunWith` for `Cli` by hand and sends the bare case to `Ready`. If you make the
  subcommand required again, `run_with` comes back with it.
- `delimiter = ','` takes a **char** literal; aliases go on variants
  (`#[usage(alias = "ls")]`).
- `AppCtx { store: store::Ctx, json: bool, color: bool }` threads through
  `RunWith` — no global working directory.
- Completions/manpages come from `tk __usage_spec__` via the `usage` CLI; there is
  no `completions` subcommand.
- A command either calls `ctx.emit(...)` (success, human text built lazily) or
  `ctx.fail(...)` (failure, envelope with `data`/`issues`), never both: `fail`
  prints the envelope itself and `cli::run` must not print a second one. `emit`
  followed by `fail` prints `ok: true` and then an error, which is a bug — it
  happened in `check` and `apply` and both are now single-report.
- `emit` prints non-fatal `issues` on stderr in human mode before the table, and
  the exit code stays zero. A read that quietly shows only the healthy entries
  is the read that lies; `tests/cli.rs::a_broken_entry_is_reported_by_a_read_too`
  is what holds that.

## Compatibility Notes

- The on-disk format is versioned, not negotiated. `.tk.json` carries
  `format: 3` and nothing else — no settings, no aliases. Anything that is not a
  format-3 store (a missing `.tk.json`, a v1 `store.json`, a v0 `config.json`,
  or files whose names are not entry names) is refused with the reason and the
  remedy, never read as an empty store and written over.
  `tools/migrate_to_v3.py` is the one-way path.
- Reading is lenient where leniency cannot hide a mistake: unknown keys are
  ignored *and preserved* on write, explicit `null` lists read as empty, and
  legacy `closed`/`cancelled`/`canceled` read as `dropped`. `active` and
  `deferred` are errors, because neither is a state the tool can enforce. Fields
  a removed version wrote (`status`, `acceptance`) survive as unknown keys rather
  than being thrown away.
- `tk apply` intents are an internally tagged enum, so serde cannot reject
  unknown fields (the two features are incompatible). A misspelled *required*
  field fails loudly; a misspelled optional one is ignored. Do not "fix" this by
  hand-rolling validation — the tag is worth more than the check. A batch has one
  accepted shape (`{"intents": [...]}`) because a second spelling is a second
  thing to document, test, and get wrong.
- Removed deliberately, each with its measurement, from a store of 169 real
  tasks: the event log and `records/` (nobody read the history — `logs` was the
  history people wrote), priority (always the default), projects (always the
  store's own name), due dates, estimates, assignees, parents, links, evidence,
  archives, `recover`, `clean`, the calendar subsystem, the `checkpoint` field
  behind `tk status` (0 uses, and the log's last entry is the status), acceptance
  criteria (0 uses, and nothing could check them), `--task-dir` aliases and
  `tk config` (nothing used them; the wrapper passes `--dir <path>`), and
  `tk lock` (nothing syncs under the lock but tk itself).
- `.local/design-v2.md` records the design and the measurements;
  `.local/simplify-plan.md` records the earlier round.

## Code Standards

| Aspect         | Standard                                                                 |
| -------------- | ------------------------------------------------------------------------ |
| Durability     | Every write is temp file + `fsync` + `rename` + directory `fsync` (`store::atomic_write`). There is no append path. |
| Locking        | Everything that writes goes through `Ctx::txn()`, which holds `<store>/.lock`; there is no unlocked write path, so the lock cannot be forgotten. Reads take no lock. Never open two transactions on one store in one process. |
| Operations     | One implementation per change, in `ops.rs`. A command is resolve → call → emit; `tk apply` dispatches to the same functions. A second implementation is how the two paths drift — v1 had one and its multi-field edit already disagreed on field order. `tests/cli.rs::a_batch_and_the_commands_agree` is what catches it. |
| Reads          | Take no lock and never repair; `show` reports an unresolvable blocker, `ls` reports unreadable files on stderr, `check` reports findings for the whole store and exits non-zero |
| Identity       | `ref` is four Crockford base32 characters, immutable, unique, never reused; the slug in `<ref>-<slug>.json` is a creation-time mnemonic. Resolution: exact ref, else a unique case-insensitive substring of slug or title; ambiguity is an error |
| Revision       | `rev` is FNV-1a over the canonical serialization of the parsed document; `--if-rev` compares inside the transaction, so it means something |
| Errors         | Store and input errors convert with `?`, not `into_diagnostic()` — the latter wraps them opaquely and loses `error_code`. `Diagnostic::code()` is left empty on purpose: miette prints whatever it returns in front of the message a human reads (`Error: invalid_input`). `cli::run` recovers the kind from the error's concrete type for the JSON envelope instead. |
| Testing        | `usage::test` harness for help drift; `assert_cmd` for end-to-end flows; real multi-process tests for concurrency |

## Known boundaries

Stated here so they are decisions rather than surprises:

- **Deleting a state verb deletes the mistake.** `done`, `drop`, and `open` each
  carry their value, so the command line cannot express a state that does not
  exist; only a batch can ask, and a batch is validated whole before any write.
  That is why there is no `tk state <ref> <value>`.
- **Scale.** `list` and `ready` read and fold every document; there is no index.
  Measured on this machine: 20ms for 161 entries, so roughly linear. An index is
  unnecessary below about a thousand tasks; past that, revisit with measurements
  rather than adding one on principle.
- **Lock scope.** `<store>/.lock` serializes cooperating `tk` processes only. A
  Git checkout, an editor, or an older `tk` binary writes without it, so re-read
  after a pull. There is no command for running an external step under the lock;
  bringing a sync into that window is not this tool's job.
- **A slug change is two filesystem operations.** The new file is written first,
  then the old one is removed, so a crash in between leaves two files claiming one
  ref — which `tk check` reports by name rather than silently resolving to one of
  them. The opposite order would lose the entry entirely.
- **`check` ignores directories.** tk writes files, so a directory beside them
  cannot be debris from a crash, and a store legitimately holds one: long-form
  documents attached to a task are the convention in place of the `links` field
  this design dropped. Real stores do contain them — the dotfiles corpus has
  `.tasks/reports/`.
- **Unknown keys round-trip, but not their position.** A hand-added field is
  preserved after a tk write, and it is appended after the known keys, so a
  custom field reorders the file once.

## Verification Steps

Commands that must pass before any milestone:
- **Build**: `cargo build`
- **Lint**: `cargo clippy --all-targets -- -D warnings`
- **Format**: `cargo fmt --all --check`
- **Unit + integration tests**: `cargo test --all-targets -- --test-threads=3`
- **Manual Check**: `tk` (bare), `tk ready`, `tk ls -a` output verification, plus
  `tk check` on the repository's own `.tasks/`
- **Concurrency**: `cargo test --test cli concurrent` and `a_held_lock_` — real
  multi-process appends, creates, and opposite blocks that must not form a loop
- **Migration**: `cargo test --test migration` (needs `python3`; skips without it)

Note for local runs: the concurrency tests spawn eight processes each, so
`cargo test --all-targets -- --test-threads=3` avoids exhausting the process
table on a busy machine.

## Distribution

- **Homebrew**: `nijaru/homebrew-tap` (see release workflow)
- **Cargo**: `cargo install --git https://github.com/nijaru/tk`
- **npm**: `@nijaru/tk` wrapper + platform packages (see release workflow)
