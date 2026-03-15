# tk-go

Go rewrite of [tk](https://github.com/nijaru/tk) — minimal task tracker CLI.

Same UX and `.tasks/` storage format as the original, compiled to a single binary with no runtime dependency.

## Status

Work in progress.

## Install

```bash
go install github.com/nijaru/tk-go@latest
```

Or build from source:

```bash
git clone https://github.com/nijaru/tk-go
cd tk-go
go build -o tk .
```

## Usage

See [tk](https://github.com/nijaru/tk) for full command reference — the Go version is a feature-complete port with identical behavior.

## Storage

Plain JSON files in `.tasks/` — fully compatible with the TypeScript version.

## License

[MIT](LICENSE)
