# Themis

Desktop coding-agent app: AutoAgents (Rust) agent core + Tauri shell + React UI,
Codex-app-style threads, OpenCode Go as the default model provider.

Plan: [.agents/plans/2026-09-23-themis-coding-agent.md](.agents/plans/2026-09-23-themis-coding-agent.md)

## Features

- Thread-based coding agent with shared project workspaces and diff review
- Approval-gated tools (read/write/execute/network/destructive risk levels)
- Skills, scheduled automations, and a review queue
- OpenCode Go model catalog and model-specific effort controls
- Name-only project creation, collapsible navigation, compact system typography
- Organized settings with autosave, manual update checks, one-click diagnostics

Docs: [user guide](docs/user-guide.md) · [sandbox model](docs/sandbox.md) ·
[distribution guide](docs/maintainer-distribution.md).

## Layout

- `crates/core` (`themis-core`) — agent runtime, providers, and tools
- `crates/desktop` (`themis-desktop`) — local app server, CLI, and Tauri shell
- `web/` — React + Vite + TypeScript frontend, Themis theme tokens in `src/theme/`

## Commands

```sh
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
npm --prefix web test
npm --prefix web run build
npx tauri dev        # from crates/desktop
npx tauri build      # from crates/desktop
python3 -m quality_dashboard.dashboard # local quality dashboard
```

The desktop starts a separate local app server process and connects to it. The
server owns thread state and keeps scheduled automations available after the
window closes. The optional `themis` CLI connects to that same server and starts it when a
conversation needs one:

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
passed as command-line arguments. Enter keys in desktop Settings.

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
