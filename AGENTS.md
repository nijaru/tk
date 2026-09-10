# tk

Minimal task tracker CLI — append-only JSON event records in `.tasks/`, single
binary, no runtime.

## Project Structure

| Directory / File   | Purpose                                                        |
| ------------------ | -------------------------------------------------------------- |
| `src/main.rs`      | Binary entry — calls `tk::cli::run()`                          |
| `src/lib.rs`       | Library root (all modules `pub` for integration tests)         |
| `src/cli.rs`       | Root `Cli` derive, global flags (`-j/--json`, `-C/--dir`), error envelope |
| `src/commands/`    | One module per command; `misc.rs` holds Init/Mv/Clean/Check/Purge/Recover/Path/Lock, `detail.rs` the detail fields, `batch.rs` the `apply` entry point |
| `src/model.rs`     | `TaskState`, `TaskView`, `Config`, `Status`, `Priority` — lenient serde |
| `src/record.rs`    | The event log: `Event`, `Record`, append, fold, torn-tail recovery |
| `src/store.rs`     | `Ctx` (location + format gate), `Store` (reads, lock-free appends), `Txn` (locked operations), list/filter, integrity |
| `src/ids.rs`       | ULID generation, aliases, reference resolution                 |
| `src/apply.rs`     | `tk apply`: intent parsing, whole-batch validation, execution   |
| `src/output.rs`    | The JSON envelope and its stable error codes                   |
| `src/timeutil.rs`  | Due parsing (`+7d`), calendar-day overdue, RFC3339Nano stamps  |
| `src/format.rs`    | Table/JSON output, color handling, unicode-safe truncation     |
| `tests/cli.rs`     | Help-drift snapshot + end-to-end and concurrency tests         |
| `tests/migration.rs` | Runs `tools/migrate-v0.py` against a v0 fixture             |
| `tools/migrate-v0.py` | One-shot v0 → v1 conversion; not a subcommand, delete after cutover |
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

- Root: `#[derive(Cli)]` + `#[usage(bin, version, run_with)]` gives `run_command_with(ctx)`.
- Command enum: `#[derive(Subcommands)]` + `#[usage(run_with)]`; each variant holds
  an `Args` struct that implements `RunWith<AppCtx>`.
- `delimiter = ','` takes a **char** literal; aliases go on variants (`#[usage(alias = "ls")]`).
- Bare `tk config` shows config: model as `Option<Subcommand>` and match `None`.
- `AppCtx { store: store::Ctx, json: bool, color: bool }` threads through `RunWith` —
  no global working directory.
- Completions/manpages come from `tk __usage_spec__` via the `usage` CLI; there is
  no `completions` subcommand.
- Every command reports through `ctx.emit(...)` (human text lazily, envelope when
  `--json`) and `cli::run` prints the failure envelope once. Do not print JSON
  directly from a command, or the shape drifts.

## Compatibility Notes

- The on-disk format is versioned, not negotiated. `store.json` carries
  `format: 2`; anything else (missing `store.json`, a v0 `config.json`, or
  top-level `*.json` task files) is refused with the reason, never read as an
  empty store and written over. `tools/migrate-v0.py` is the one-way path.
- Reading is lenient where leniency cannot hide a mistake: unknown fields and
  unknown event `op`s are ignored, explicit `null` slices read as empty, and
  legacy `cancelled` maps to `closed`. Unknown *intent* fields in `tk apply` are
  rejected, because a misspelled key silently doing nothing is worse.
- A v0 writer must not run against a v1 store: it would see zero tasks and
  create v0 files beside the records. Stop old writers before cutting over.
- Deliberate breaks from the 0.x line: project-prefixed IDs and `previous_ids`
  are gone (identity is a ULID), `tk repair` became `tk recover`, `tk rm` became
  `tk purge` (refuses while referenced), and `tk add` no longer bootstraps a
  store.

## Code Standards

| Aspect         | Standard                                                                 |
| -------------- | ------------------------------------------------------------------------ |
| Durability     | Appends fsync the file (and the directory, when the record is new); `store.json` writes use temp-file + fsync + rename |
| Locking        | `Store` = reads and lock-free appends (LWW/commutative ops). `Txn` = holds `<store>/.lock` and owns every composite operation: create, block, `--if-rev`, multi-record, apply. Never open two transactions on one store. |
| Reads          | Take no lock and never repair; `show` reports, `check` reports for the store, `recover` truncates a torn tail |
| Identity       | ULID + immutable alias; `project` is display only. Resolution: alias → ID → unique prefix |
| Revision       | `rev` is `writer:line_count:content_hash8`; `--if-rev` compares under the lock so it means something |
| Errors         | Store and input errors convert with `?`, not `into_diagnostic()` — the latter wraps them opaquely and loses `error_code` |
| Precision      | RFC3339Nano stamps; calendar-day (not 24h) overdue math                  |
| Testing        | `usage::test` harness for help drift; `assert_cmd` for end-to-end flows  |

## Verification Steps

Commands that must pass before any milestone:
- **Build**: `cargo build`
- **Lint**: `cargo clippy --all-targets -- -D warnings`
- **Format**: `cargo fmt --all --check`
- **Unit + integration tests**: `cargo test --all-targets`
- **Manual Check**: `tk ready` and `tk list -a` output verification
- **Concurrency**: `cargo test --test cli concurrent` — real multi-process appends,
  plus a lock-held append that must proceed and a block that must wait
- **Migration**: `cargo test --test migration` (needs `python3`; skips without it)

Note for local runs: the concurrency tests spawn eight processes each, so
`cargo test --all-targets -- --test-threads=3` avoids exhausting the process
table on a busy machine.

## Distribution

- **Homebrew**: `nijaru/homebrew-tap` (see release workflow)
- **Cargo**: `cargo install --git https://github.com/nijaru/tk`
- **npm**: `@nijaru/tk` wrapper + platform packages (see release workflow)
