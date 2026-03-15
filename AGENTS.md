# tk-go

Go rewrite of [tk](https://github.com/nijaru/tk) — minimal task tracker CLI.

## Goal

Feature-complete Go port. Same UX, same `.tasks/` storage format, same JSON schema. Ships as a single compiled binary with no runtime dependency.

## Source Reference

Original TypeScript implementation is at `../tk/` (or open both as a Zed workspace folder). Use it as the canonical reference for behavior, storage schema, and command semantics.

## Stack

- **Language:** Go (latest stable)
- **CLI framework:** [cobra](https://github.com/spf13/cobra) or stdlib `flag` — decide in design phase
- **Formatter:** `golines --base-formatter gofumpt`
- **Tests:** `go test ./...`
- **Build:** `go build -o tk .`

## Structure (planned)

```
cmd/          # cobra commands (one file per command)
internal/
  task/       # Task type, storage, CRUD
  format/     # Table/JSON output
  priority/   # Priority parsing
  time/       # Time helpers
main.go
```

## Storage Format

`.tasks/` directory — one JSON file per task, `config.json` for project config. Must be fully compatible with the TypeScript version (same schema, same filenames).

See `../tk/src/db/storage.ts` and `../tk/src/types.ts` for the canonical schema.

## Key Behaviors

- Task IDs: `project-ref` (e.g. `myapp-a7b3`), just the ref works everywhere
- Prefix matching: `a7` resolves unambiguously
- `NO_COLOR` env var disables color; auto-disabled when output is piped
- `--json` flag works globally
- `-C <dir>` runs in a different directory

## Commands to Implement

init, add, ls/list, ready, show, start, done, reopen, edit, log, block, unblock, rm/remove, clean, check, config, completions, help

See `../tk/README.md` for full option reference.

## Development Notes

- Read the TypeScript source before implementing each command — match behavior exactly
- No feature additions, no behavior changes — pure port
- Write tests for priority parsing, time helpers, ID resolution, and storage
