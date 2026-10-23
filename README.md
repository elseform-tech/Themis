# Themis

Desktop coding-agent app: AutoAgents (Rust) agent core + Tauri shell + React UI,
Codex-app-style threads, OpenCode Go as the default model provider.

Plan: [.agents/plans/2026-09-23-themis-coding-agent.md](.agents/plans/2026-09-23-themis-coding-agent.md)

## Features

- Thread-based coding agent with shared project workspaces and a read-only
  Workspace monitor (changes, files, branches, pull requests) plus a per-turn
  list of edited files
- Approval-gated tools (read/write/execute/network/destructive risk levels)
- Skills, scheduled automations, and a review queue
- OpenCode Go model catalog and model-specific effort controls
- Name-only project creation in a fixed projects folder (deleting moves the folder
  to the Trash), a project-scoped sidebar, and the Bubbles desktop companion
- Organized settings with autosave and manual update checks; diagnostics stay in
  the CLI (`themis doctor`) and bounded log files
- Adaptive context management: bounded tool results, durable checkpoints and
  automatic compaction near the model's context budget

Docs: [user guide](docs/user-guide.md) · [sandbox model](docs/sandbox.md) ·
[configuration and diagnostics](docs/configuration.md) · [context and attachments](docs/context-and-attachments.md) ·
[plugins and skills](docs/plugins.md) · [automations](docs/automations.md) ·
[UI rules](docs/ui-rules.md) · [appearance](docs/appearance.md) ·
[property tests and fuzzing](docs/fuzzing.md) · [distribution guide](docs/maintainer-distribution.md). Verification records live in
[docs/verification](docs/verification).

## Layout

- `crates/core` (`themis-core`) — agent runtime, providers, and tools
- `crates/desktop` (`themis-desktop`) — local app server, CLI, and Tauri shell
- `web/` — React + Vite + TypeScript frontend, Themis theme tokens in `src/theme/`

## Commands

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix web run lint
npm --prefix web test
npm --prefix web run build
npx tauri dev        # from crates/desktop
npx tauri build      # from crates/desktop
python3 -m quality_dashboard.dashboard # local quality dashboard
```

`AGENTS.md` lists the full check sequence, including Rust coverage and the dashboard tests.

The desktop starts a separate local app server process and connects to it. The
server owns thread state and stops when the app quits, crashes or is force-quit, taking
its background programs, MCP servers and built-in search with it. If a server is already
running (started by the CLI), the app uses it and leaves it running. The optional `themis`
CLI connects to that same server and starts one that keeps running (until `themis server
stop`) when a conversation needs one:

```sh
cargo run -p themis-desktop --bin themis -- serve
cargo run -p themis-desktop --bin themis -- status
cargo run -p themis-desktop --bin themis -- thread create /path/to/project
cargo run -p themis-desktop --bin themis -- thread send THREAD_ID "task"
cargo run -p themis-desktop --bin themis -- chat THREAD_ID
cargo run -p themis-desktop --bin themis -- call get_settings
cargo run -p themis-desktop --bin themis -- server stop
```

Pass `--data-dir DIR` before the command to use an isolated server and data
store. `call METHOD JSON_ARGS` exposes backend commands with the same camelCase
arguments as the desktop bridge; it rejects `set_secret` so real keys are never
passed as command-line arguments. Enter keys with `themis providers` or desktop Settings.

`thread send` streams the run and prompts for tool approvals in a terminal.
`--json` emits newline-delimited run events; `--detach` returns a run handle.
Unattended sends deny tool approvals. `chat THREAD_ID` accepts repeated prompts
until `/quit`; followups reuse the same persisted history, model, effort and skills.
The old standalone `themis run` and `themisctl` commands are retired.

The dashboard refreshes repository and test data every five seconds and counts
automated Rust, frontend, dashboard, and CLI end-to-end tests.
Use an isolated `themis` server for backend journeys; verify UI behavior
separately in the browser or native app.
Generate Rust line coverage with `cargo-llvm-cov` as documented in
`quality_dashboard/README.md`; the dashboard reads the resulting LCOV report.

Keys entered in Settings are stored in macOS Keychain, Windows Credential Manager,
or the Linux Secret Service and remain available after restart when that store is
available. OpenCode Go can also use `OPENCODE_KEY` in the server launch environment.
Conversation history is stored in SQLite under the app-data directory; keys are
never stored in SQLite, settings, or frontend storage.

`themis doctor [--json] [--deep]` checks installation, authenticated server discovery,
JSON stores, SQLite schema/integrity, and history/context/checkpoint consistency.
It runs offline without starting the server, migrating stores, or repairing data.
With a running server it uses the live SQLite connection. `--deep` selects the full
SQLite integrity check. Failures exit nonzero; missing unused stores are warnings.
Reports contain counts and status, not transcript contents or keys. Readability
checks do not prove successful writes or crash durability.

`themis providers` offers OpenCode Go and reads its API key with terminal echo
disabled. It saves to the same OS credential store as Settings, including when
the desktop/server is closed. Redirected input is rejected; keys are never accepted
as command arguments. Ctrl-C cancels key entry and restores terminal echo.
`OPENCODE_KEY` remains an alternative in the **server launch** environment;
changing the shell environment does not change an already-running server.

The macOS app bundle also hosts the CLI. After moving ThemisCode into Applications,
install the optional command without a second download:

```sh
/Applications/ThemisCode.app/Contents/MacOS/themis-desktop --themis-cli cli install
# If ~/.local/bin is not already on PATH, add it in your shell configuration.
```

The installer creates a `themis` symlink in `~/.local/bin` (or an explicit absolute
directory), refuses to overwrite existing commands, and leaves shell configuration
unchanged. Moving or deleting the app breaks that link. On Windows, build the
console CLI with `cargo install --path crates/desktop --bin themis`.
