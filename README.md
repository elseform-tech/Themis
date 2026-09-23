# Themis

Desktop coding-agent app: AutoAgents (Rust) agent core + Tauri shell + React UI,
Codex-app-style threads, OpenCode Go as the default model provider.

Plan: [.agents/plans/2026-09-23-themis-coding-agent.md](.agents/plans/2026-09-23-themis-coding-agent.md)

## Features

- Thread-based coding agent with per-thread git worktrees and diff review
- Approval-gated tools (read/write/execute/network/destructive risk levels)
- Skills, scheduled automations, and a review queue
- Providers: OpenCode Go (default), OpenAI, Anthropic, Ollama, custom
- Name-only project creation, collapsible navigation, compact system typography
- Organized settings with autosave, manual update checks, one-click diagnostics

Docs: [user guide](docs/user-guide.md) · [sandbox model](docs/sandbox.md) ·
[distribution guide](docs/maintainer-distribution.md).

## Layout

- `crates/core` (`themis-core`) — agent definitions, providers, tools, thread state
- `crates/desktop` (`themis-desktop`) — Tauri app shell
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
```

Keys entered in Settings live only in backend memory for the current app session.
OpenCode Go can use `OPENCODE_KEY` exported in the app launch environment. Keys
are never written to the keychain, SQLite, settings, or frontend storage.
