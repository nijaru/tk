# tk-go

Go port of [tk](https://github.com/nijaru/tk) — minimal task tracker CLI.
Feature-complete, single-binary, no-dependency successor.

## Project Structure

| Directory  | Purpose                                                              |
| ---------- | -------------------------------------------------------------------- |
| `cmd/`     | CLI commands (Kong), subcommands, and flags                          |
| `internal/`| Core logic (not for external use)                                    |
| `  task/`  | Storage (atomic write), CRUD, Root discovery, ID resolution         |
| `  format/`| Table/JSON output, color handling, truncation                        |
| `  priority/`| Priority parsing (0-4, none-low)                                   |
| `  timeutil/`| Relative dates (+7d), overdue logic, RFC3339Nano precision         |
| `ai/`      | Local-only AI context — excluded via `.git/info/exclude`             |
| `.tasks/`  | Local-only task state — excluded via `.git/info/exclude`             |

## AI Context Organization

**Purpose:** Persistent memory for AI assistants without polluting git history.
**Reference original source:** `../tk/` is the canonical reference for behavior and schema.

**Session files** (local only):
- `ai/STATUS.md` — Current state, findings, verification status (Read FIRST)
- `ai/DESIGN.md` — Architecture decisions, Go vs TS differences
- `ai/DECISIONS.md` — Append-only design log

## Technology Stack

| Component  | Technology                                      |
| ---------- | ----------------------------------------------- |
| Language   | Go 1.23+                                        |
| CLI        | [Kong](https://github.com/alecthomas/kong)       |
| Formatting | `golines --base-formatter gofumpt`              |
| Colors     | `github.com/fatih/color` (NO_COLOR support)     |
| Testing    | `go test` + `github.com/stretchr/testify`       |

## Code Standards

| Aspect         | Standard                                                                 |
| -------------- | ------------------------------------------------------------------------ |
| Performance    | Use `strings.Builder` for ID generation; pre-allocate slices             |
| Durability     | Atomic writes must `f.Sync()` and close before `os.Rename`               |
| Precision      | Use `time.RFC3339Nano` for sub-second precision (matching TS ISO strings) |
| Error handling | Propagate errors; return clear context for CLI reporting                 |
| Naming         | Descriptive suffixes OVER versioning (e.g. `_async`); keep scope small   |
| Interfaces     | Functional core (types) vs Imperative shell (storage/cmd)                |

## Verification Steps

Commands that must pass before any milestone:
- **Build**: `go build -o tk .`
- **Unit Tests**: `go test ./...`
- **Manual Check**: `tk ready` and `tk list -a` output verification

## Distribution Plan

- **Homebrew**: Push to `nijaru/homebrew-tap` (using Makefile/GoReleaser local test first)
- **Go Install**: `go install github.com/nijaru/tk@latest`
