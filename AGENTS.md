# tk

Minimal task tracker CLI — plain JSON in `.tasks/`, single binary, no runtime.

## Project Structure

| Directory / File   | Purpose                                                        |
| ------------------ | -------------------------------------------------------------- |
| `src/main.rs`      | Binary entry — calls `tk::cli::run()`                          |
| `src/lib.rs`       | Library root (all modules `pub` for integration tests)         |
| `src/cli.rs`       | Root `Cli` derive, global flags (`-j/--json`, `-C/--dir`)      |
| `src/commands/`    | One module per command; `misc.rs` holds Remove/Init/Mv/Clean/Check, `detail.rs` holds checkpoint/links/acceptance/evidence/archive |
| `src/commands/config.rs` | Nested `config` subcommands (project/alias/defaults/clean-after) |
| `src/model.rs`     | `Task`, `Config`, `Status`, `Priority` — lenient serde for old files |
| `src/store.rs`     | `Ctx` (store resolution), `Txn` (locked mutations), atomic writes, CRUD, list/filter, integrity |
| `src/ids.rs`       | Project/ref validation, ref generation, ID resolution          |
| `src/timeutil.rs`  | Due parsing (`+7d`), calendar-day overdue, RFC3339Nano stamps  |
| `src/format.rs`    | Table/JSON output, color handling, unicode-safe truncation     |
| `tests/cli.rs`     | Help-drift snapshot + end-to-end CLI tests                     |
| `ai/`              | **Symlink** into the central knowledge store (`agent-context/projects/github.com/nijaru/tk/ai`). Owned by the context system — do not write here from a repo session |
| `.local/`          | Clone-local working notes — excluded via `.git/info/exclude`   |
| `.tasks/`          | Local-only task state — excluded via `.git/info/exclude`       |

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

## Compatibility Notes

- Reads task files written by the old Go binary: unknown fields ignored, explicit
  `null` slices read as empty, legacy string logs and `cancelled` status parsed.
- New task fields (`checkpoint`, `links`, `acceptance`, `evidence`,
  `previous_ids`, `archived_at`) are additive and optional. An old writer that
  rewrites a record drops them, so mixed-version writers need a controlled
  upgrade rather than coexistence.
- Deliberate breaks from Go: `mv` moves tasks only (project rename lives under
  `config project rename`); `external` provider stubs dropped; `@me` was never
  implemented (help text only) and is gone.

## Code Standards

| Aspect         | Standard                                                                 |
| -------------- | ------------------------------------------------------------------------ |
| Durability     | Atomic writes must `f.Sync()` file and dir before `rename`               |
| Mutations      | Every write runs inside `Txn` (one advisory lock over resolve→read→validate→edit→persist). Never open two transactions on one store. |
| Reads          | Read-only commands take no lock and never repair; `show` reports, `repair` fixes |
| Precision      | RFC3339Nano stamps; calendar-day (not 24h) overdue math                  |
| Error handling | `miette` diagnostics that read like what the user sees                   |
| Testing        | `usage::test` harness for help drift; `assert_cmd` for end-to-end flows  |

## Verification Steps

Commands that must pass before any milestone:
- **Build**: `cargo build`
- **Lint**: `cargo clippy --all-targets -- -D warnings`
- **Format**: `cargo fmt --all --check`
- **Unit + integration tests**: `cargo test --all-targets`
- **Manual Check**: `tk ready` and `tk list -a` output verification
- **Concurrency**: `cargo test --test cli concurrent` (real multi-process appends/labels; fails without the store lock)

## Distribution

- **Homebrew**: `nijaru/homebrew-tap` (see release workflow)
- **Cargo**: `cargo install --git https://github.com/nijaru/tk`
- **npm**: `@nijaru/tk` wrapper + platform packages (see release workflow)
