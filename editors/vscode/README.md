# SAS Linter for VSCode

Lint and format SAS source files using the [sas-linter-rs](https://github.com/mes-amis/sas-linter-rs) `sas-lint` binary.

- **Diagnostics**: rule findings as squiggles on save (configurable: `onSave` / `onType` / `manual`).
- **Format Document**: wires `sas-lint --format` to VSCode's format-document hook. Respects the same YAML `[format]` block as the CLI.
- **Quick Fix**: per-rule autofix code action for any rule that supports it.
- **Fix all**: command palette → `SAS Linter: Run all autofixes on current file`.

The extension auto-downloads the matching `sas-lint` binary from GitHub Releases on first activation and caches it under `globalStorage`. Set `sasLinter.version` to pin a different release tag — the extension downloads it on the next lint run, no extension update required. Or override with `sasLinter.path` to use a locally built binary instead — useful when developing this repo (`cargo build --release` then point the setting at `target/release/sas-lint`).

## Settings

| key | default | meaning |
|---|---|---|
| `sasLinter.path` | `""` | Absolute path to `sas-lint`. Empty = use the auto-downloaded binary. Takes precedence over `sasLinter.version`. |
| `sasLinter.version` | `""` | Release tag of the `sas-lint` binary to auto-download (e.g. `v0.3.2`; a bare `0.3.2` also works). Empty = the version pinned by the extension. Changing it triggers a download and re-lint immediately. |
| `sasLinter.config` | `""` | Path to a sas-linter YAML config (workspace-relative or absolute). Empty = sas-lint's default lookup (`config/lint.yaml` in cwd). |
| `sasLinter.run` | `onSave` | When to run the linter: `onSave`, `onType`, or `manual`. |
| `sasLinter.format.enabled` | `true` | Register `sas-lint --format` as the SAS document formatter. |

## Development

```sh
cd editors/vscode
npm install
npm run compile        # one-off build to dist/extension.js
npm run watch          # rebuild on change
npm run typecheck      # tsc --noEmit
```

Press `F5` in VSCode with this folder open to launch an Extension Development Host with the extension loaded. Open a `.sas` file in the host window to exercise it. For iteration on the binary side, point `sasLinter.path` at `../../target/release/sas-lint` so the host picks up rebuilt rules without a re-download.

## Supported platforms (prebuilt binary)

| OS / arch | release target |
|---|---|
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-musl` |
| Linux arm64 | `aarch64-unknown-linux-musl` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

## License

AGPL-3.0-or-later. Same as the parent crate.
