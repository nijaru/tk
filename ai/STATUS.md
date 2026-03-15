# Status

**State:** `v0.1.1` released. CI/CD Pipeline fully operational and validated for tokenless OIDC npm publishing.

## What's on Main

Full review pass fixes + npm pipeline with GoReleaser v2 corrections:

- ParseID regex, writeTaskFileExclusive cleanup, CleanTasks O(n), GetTask N+1, block/unblock/log storage funcs
- completions real scripts, applySliceUpdates sorted, CI race tests
- npm platform packages + wrapper: `npm/main/package.json`, `npm/main/bin/tk.js`
- CI and release workflows fully set up and fixed for GoReleaser v2 requirements

## npm Distribution Setup

- `@nijaru/tk` wrapper and 5 platform packages (`@nijaru/tk-{darwin-arm64,darwin-x64,linux-arm64,linux-x64,win32-x64}`) are live on the registry.
- `v0.1.1` was manually published to npm to circumvent initial CI workflow errors.
- **Workflow is fixed for future releases:** The `release.yml` uses 100% tokenless OIDC for all 6 packages. The `actions/setup-node@v4` no longer uses `registry-url`, preventing `.npmrc` conflicts with OIDC.

## Distribution

| Method     | Command                                                     |
| ---------- | ----------------------------------------------------------- |
| Homebrew   | `brew install nijaru/tap/tk`                                |
| Go install | `go install github.com/nijaru/tk@latest`                    |
| npm        | `npm install -g @nijaru/tk`                                 |
| Release    | `gh workflow run release.yml -R nijaru/tk -f version=X.Y.Z` |

## Key Files

- `.goreleaser.yaml` — updated to v2 specs (`formats: ["tar.gz"]`, uses `brews` for CLI formulas)
- `.github/workflows/release.yml` — explicitly creates local git tags for GoReleaser v2 compliance; single npm step via OIDC
- `.github/workflows/ci.yml` — concurrency cancel on PR/push
- `npm/main/package.json` — version template (jq substitutes at release)
- `npm/main/bin/tk.js` — JS wrapper resolving platform binary
