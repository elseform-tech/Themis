## Goal

Build a desktop coding-agent app ("Themis") on the LiquidOS AutoAgents Rust SDK, with a Codex-app-style experience: projects containing parallel agent threads, in-thread diff review, sandboxed tool execution, and provider-agnostic model support — with OpenCode Go as the first/default provider and a purpose-built Themis color system and UI design.

## Success Criteria

- A user can open a project folder, start an agent thread, and watch it reason, call coding tools, and stream progress in the desktop UI.
- The user can review the agent's file changes as a diff inside the thread and accept, comment on, or discard them.
- Multiple agents can run in parallel on the same repo without conflicts, each isolated (worktree-per-thread model).
- Tool execution is sandboxed by default: filesystem tools scoped to the project root, elevated actions gated behind user approval.
- OpenCode Go works out of the box as the default provider (BYOK Go key); the user can switch to other providers (direct OpenAI, Anthropic, local Ollama, custom OpenAI-compatible URL) without changing agent logic.
- The app ships a coherent Themis design system: token-driven dark-first theme with a light mode, consistent status/diff semantics, keyboard-operable approvals and review, and body-text contrast of at least 4.5:1 in both themes.
- The app installs and runs on macOS first; the codebase stays cross-platform-ready for Windows/Linux.

## Context And Current Facts

- Workspace `/Users/cutedandelion/Documents/ChatGPT/ThemisOS` is an empty git repo (no commits). This is greenfield work; no existing code, tests, or conventions constrain the design.
- Local toolchain observed: `rustc/cargo 1.91.1+` required by AutoAgents and satisfied (1.98.1 installed); Node v26.4.0 and npm 11.17.0 available.
- AutoAgents (LiquidOS) is a Rust multi-agent framework (current docs: `autoagents`/`autoagents-derive` 0.4.0) with: Basic (single-turn), ReAct (multi-turn direct tool calls), and CodeAct (multi-turn, model writes TypeScript executed in a sandbox with registered tools bridged as `external_*` functions) executors; pluggable cloud/local LLM providers behind one interface; configurable memory; an actor runtime for multi-agent coordination; and `autoagents-toolkit` with root-scoped filesystem tools plus MCP integration.
- For providers without a named integration, AutoAgents ships `OpenAICompatibleProvider<T: OpenAIProviderConfig>`: a reusable chat-completions-wire-format client with configurable base URL, model, endpoint path, `extra_body`, and static `custom_headers()`. Named backends in `LLMBackend` include openai, anthropic, ollama, deepseek, xai, phind, google, groq, azure-openai, openrouter, and minimax.
- OpenCode Go is a $10/month subscription giving API-key access to open coding models through OpenAI-compatible endpoints (`/zen/go/v1/chat/completions`, `/v1/responses`, `/v1/messages`, `/v1/models`). It "works like any other provider". Third-party clients must send typical coding-agent traffic, identify with their own user agent (e.g. `my-coding-agent/1.0`), and send a stable per-conversation session ID in `x-opencode-session` for routing and prompt caching.
- OpenCode also runs as a headless HTTP server (`opencode serve`, default `127.0.0.1:4096`) with an OpenAPI surface and a type-safe JS/TS SDK (`@opencode-ai/sdk`, `createOpencode`). This is the "drive OpenCode as the agent backend" alternative, addressed in Key Decisions.
- The Codex app reference (OpenAI announcement): a desktop "command center" where agents run in separate threads organized by projects, with in-thread diff review plus comments, built-in worktree support so parallel agents share a repo without conflicts, skills for extensibility, scheduled Automations landing in a review queue, and secure-by-default sandboxing (folder-scoped edits, permission escalation for elevated commands). macOS first, Windows later. (Layout and workflow reference only — Themis gets its own visual identity; no Codex colors are copied.)
- Tauri 2.0 pairs a Rust backend with any web frontend, builds for macOS/Windows/Linux (plus mobile) from one codebase, uses the OS native renderer (small binaries), and bridges UI and Rust via commands and events with a permissions/capabilities security model.
- Styling stack facts: Tailwind CSS is a utility-first CSS framework (classes composed in markup); shadcn/ui is a set of composable, accessible components built to be customized and made your own. Themis theme tokens are implemented as CSS custom properties consumed by both.

## Constraints And Non-goals

- Constraints: Rust-first agent core (AutoAgents); desktop app, macOS-first; provider-agnostic by design with OpenCode Go first; sandboxed-by-default tool execution; secrets (API keys, including the Go key) never committed, stored via OS keychain at runtime; dark-first UI with a supported light theme, token-driven (no hardcoded colors outside the token file).
- Non-goals for the initial build: IDE extensions and CLI parity; cloud-hosted agents or triggers (Automations run only while the app runs); multi-user/team features and SSO; a plugin marketplace; mobile builds (Tauri supports them, but they are out of scope); custom brand illustration/mascot work.

## Key Decisions

1. **Agent runtime: AutoAgents Rust SDK (recommended, unchanged).** It matches the original request, gives memory-safe execution plus sandboxed tool/CodeAct execution, and its provider abstraction covers Go (via the OpenAI-compatible seam) plus cloud and local models. Alternative (Python framework such as LangChain-style stacks) rejected: it would split the codebase across two runtimes and lose the Rust sandboxing story that justifies this stack.
2. **First provider: OpenCode Go via `OpenAICompatibleProvider` (recommended).** Go is an OpenAI-compatible provider by design, AutoAgents has an explicit seam for exactly this ("providers that do not yet have a named integration"), and its low-cost open coding models fit a default. Provider priority: (1) OpenCode Go, (2) direct OpenAI, (3) Anthropic, (4) local Ollama, (5) custom OpenAI-compatible base URL. Model IDs are never hardcoded: Go models resolve from settings with refresh against Go's `/v1/models`.
3. **Go wire details (recommended).** Target Go's `chat/completions` endpoint first, since the AutoAgents seam speaks the chat-completions wire format; Go models served only on `/responses` stay out of scope until validated. Send a Themis user agent (`themis/<version>`) via `custom_headers()`. The per-conversation `x-opencode-session` header is a Phase 1 spike: `custom_headers()` is a static trait hook, so per-thread IDs need an upstream contribution, a thin header-injecting layer, or a static fallback value (degraded prompt caching, still functional). Fallback keeps Go working regardless of spike outcome.
4. **Alternative considered: drive `opencode serve` as the agent backend (deferred, not recommended now).** OpenCode exposes a headless HTTP server plus SDK that a desktop client could drive instead of AutoAgents. Rejected for v1 because it swaps the in-process Rust core for a separately-shipped server sidecar (two runtimes, heavier distribution, loss of the AutoAgents sandboxing story). Kept as a documented future backend option behind the thread-runner boundary if AutoAgents stalls.
5. **Desktop shell: Tauri 2.0 (recommended, unchanged).** The agent core is already Rust, so Tauri embeds it in-process with typed command/event IPC instead of an HTTP sidecar; one codebase covers macOS now and Windows/Linux later; native-renderer binaries stay small. Alternative (Electron) rejected: it embeds Chromium + Node, which duplicates runtimes (larger, heavier) while the Rust core would still need a sidecar or native module.
6. **Frontend: React + Vite + TypeScript with Tailwind + shadcn-style components (recommended).** Tauri supports any web stack; React + TS gives the richest diff-viewer, markdown, and terminal-component ecosystem for a Codex-like thread UI. Tailwind (utility-first) plus accessible shadcn-style primitives, both driven by Themis CSS-variable tokens, keeps theming in one file. Reversible: the Rust core exposes the full API over Tauri commands, so the frontend can be swapped without touching agent logic.
7. **Themis visual identity: dark-first "Ink + Gold" system with a light "Paper" mode (recommended, proposed tokens below).** Gold accent nods to Themis (scales/justice) and distinguishes the app from default blue-accent dev tools; semantic colors stay conventional (green/red/amber/blue) so status and diffs read instantly. Adjustable before Phase 2 without code churn since everything flows from tokens.
8. **Coding executor: start with ReAct, add CodeAct for composed tasks (recommended, unchanged).** ReAct's direct tool calls are simpler to stream, approve per-step, and debug for file edits; CodeAct's sandboxed TypeScript composition fits multi-step orchestration (batch edits, data shaping, computed refactors) once the tool surface stabilizes. Both stay available behind a per-thread strategy switch.
9. **Parallelism model: one git worktree per active thread (recommended, mirrors Codex).** Each thread gets an isolated checkout of the repo; diffs are computed thread-local; merging back to the user's branch is an explicit review action. Alternative (single-checkout with file locking) rejected: it serializes agents and risks interleaved edits.

## UI Design System

Single source of truth: `web/src/theme/tokens.css` (CSS custom properties, `--themis-*`). No hardcoded colors elsewhere. Dark is the default; light follows the OS setting unless overridden in Settings. Proposed tokens (exact values adjustable pre-Phase-2):

Dark "Themis Ink" (default):

- `--themis-canvas: #0B0E13` (app background), `--themis-surface: #12161D` (panels), `--themis-raised: #1A2029` (popovers, modals), `--themis-border: #262E3A`, `--themis-border-subtle: #1C232E`
- `--themis-text: #E8EAF0`, `--themis-text-2: #9AA3B2`, `--themis-text-3: #6B7484`
- `--themis-accent: #D9A73E` (gold: primary CTAs, active thread, streaming caret), `--themis-accent-hover: #E5B95C`, `--themis-accent-ink: #1A1405` (text on gold)
- Semantically fixed: `--themis-success: #3FB950`, `--themis-warning: #D29922`, `--themis-danger: #F85149`, `--themis-info: #58A6FF`
- Diffs: `--themis-add-bg: rgba(63,185,80,.14)`, `--themis-del-bg: rgba(248,81,73,.14)`, with matching border tokens
- Thread status mapping: running = accent pulse, awaiting-approval = warning, done = success, failed = danger, queued = text-3

Light "Themis Paper":

- `--themis-canvas: #F6F4EF`, `--themis-surface: #FFFFFF`, `--themis-raised: #FFFFFF`, `--themis-border: #E2DCD0`, `--themis-text: #1C1A15`, `--themis-text-2: #5C574A`, `--themis-accent: #9A6B1F` (darkened gold for 4.5:1 contrast on light), `--themis-accent-ink: #FFFFFF`; semantics hold with light-tuned add/del washes.

Typography: system UI stack for chrome (`-apple-system, ...`), system monospace stack for code, diffs, and tool traces; scale 12 / 13 / 14 / 16 / 20 / 24 / 28 with 13px body in dense surfaces (sidebar, traces) and 14px in reading surfaces (messages, diffs).

Layout shells (information architecture mirrors the Codex reference; visuals are Themis):

1. Project sidebar: projects → threads with status dots and model badges; `⌘K` palette for thread/project/command jump.
2. Thread view: header (title, provider+model badge, worktree badge, budget meter), message stream, collapsible tool-trace timeline, composer with strategy switch (ReAct/CodeAct) and approval-mode toggle.
3. Diff review panel: file tree + unified/side-by-side toggle, per-file accept / comment / discard, "open in editor".
4. Approval dialog: action summary with risk highlight (writes, shell, network), allow-once / allow-always-for-thread / deny; fully keyboard-operable.
5. Settings: providers + keys (Go key first), theme (dark/light/system), sandbox policy, concurrency and budget limits.
6. Review queue (Phase 4): automation results awaiting human continuation; nothing auto-merges.

Component inventory (Phase 2): Button, Input, Dialog, Dropdown, Tabs, Tooltip, Badge/StatusDot, BudgetMeter, ChatMessage, ToolTrace, CodeBlock (line numbers + language-aware highlighting), DiffView, FileTree, EmptyState, Toast. Motion: 150–200ms ease-out, streaming caret, status pulse; `prefers-reduced-motion` disables non-essential animation. Focus-visible rings on all interactive elements; approvals and diff actions reachable by keyboard alone.

## Recommended Approach

Build a single Rust workspace with the agent core as a library crate consumed in-process by a Tauri desktop app:

- `core` crate: AutoAgents agent definitions; provider registry with OpenCode Go first (OpenAI-compatible config: base `https://opencode.ai/zen/go/v1`, chat-completions path, Themis user agent, session-header mechanism from the Phase 1 spike) plus direct OpenAI, Anthropic, Ollama, and custom URL; coding toolset (root-scoped `autoagents-toolkit` filesystem tools plus custom `shell`, `apply_patch`, and `git` tools with approval hooks); executor selection (ReAct default, CodeAct opt-in); streaming event types; thread/worktree state machine. A thin CLI harness in `core` drives threads headlessly so agent behavior is testable without the UI.
- `desktop` crate + `web` frontend: Tauri app. Rust side exposes commands (`create_thread`, `send_message`, `list_diff`, `approve_action`, `merge_thread`, ...) and streams agent events (tokens, tool calls, approvals, results) to the React UI. UI built per the UI Design System section above: Tailwind + shadcn-style primitives on Themis tokens, six layout shells, `⌘K` palette.
- Phase the build so each phase is independently verifiable: scaffold (+tokens) -> headless Go-first agent -> single-thread designed desktop app -> parallel worktree threads -> skills/automations -> hardening and distribution. Automations and skills ship last because the Codex reference treats them as extensions on top of a solid thread/diff/approval core.
- Assumptions (stated, reversible): macOS-first distribution with Windows/Linux kept building in CI; English-only UI; single local user, no accounts; git-backed projects only for v1 (non-git folders open read-only for the agent); pure BYOK including the user's own Go subscription key; proposed gold-accent identity accepted unless Q4 says otherwise.

## Work Plan

1. **Phase 0 — Scaffold.** Tauri 2.0 workspace (`core` lib + `desktop` app + React/Vite/TS + Tailwind + shadcn-style frontend), AutoAgents 0.4.x dependency with `openai` + `ollama` features, theme token scaffold (`tokens.css`, dark/light, theme toggle plumbing), contrast test harness, `.env`-ignored secrets, keychain-backed settings stub, CI (fmt, clippy, tests, multi-OS build).
2. **Phase 1 — Headless Go-first agent.** Provider registry (Go default, OpenAI, Anthropic, Ollama, custom URL); Go config + Themis user agent + session-header spike with static fallback; Go `/v1/models` refresh (no hardcoded model IDs); coding toolset on root-scoped toolkit tools + custom shell/patch/git tools; ReAct thread loop with structured streaming events and approval checkpoints; CLI harness (`themis run "task" --project DIR`); unit + integration tests with a mock LLM.
3. **Phase 2 — Single-thread designed desktop app.** Tauri commands/events bridge; the six layout shells per the UI Design System (sidebar, thread view, diff review, approval dialog, settings, queue stub); component inventory with both themes; `⌘K` palette; in-thread diff review (accept/comment/discard, open-in-editor); approval dialog for elevated actions; provider switching without restart.
4. **Phase 3 — Parallel threads.** Worktree-per-thread lifecycle (create on thread start, isolate, merge/discard on close); thread list with live status; actor-runtime coordination and global concurrency cap; cross-thread conflict surfacing at merge time.
5. **Phase 4 — Skills + Automations (post-MVP toggle).** Skill bundles (instructions + scripts + tool grants) with a management screen and per-thread enablement; scheduled background automations whose results land in a review queue.
6. **Phase 5 — Hardening + distribution.** Sandbox policy lockdown (default-deny outside project root, network-gated commands), macOS signing/notarization and Windows build, auto-update, crash/error reporting (opt-in), user docs and onboarding.

## Validation Plan

- Phase 0: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `npm --prefix web test` (including token-contrast tests: every text/background pair ≥ 4.5:1, large text ≥ 3:1, in both themes), `cargo tauri build` (macOS) all green; CI reproduces them on macOS/Windows/Linux.
- Phase 1: `cargo test -p themis-core` covers tool sandboxing (path traversal rejected, symlink escape rejected), approval gating (elevated shell blocked until approved), a provider-matrix test (same scripted task produces the same tool-call sequence via Go-shaped mock, OpenAI-shaped mock, and Ollama-shaped mock), an outgoing-header test (Go requests carry Themis user agent + session header per the spike mechanism), and a scripted mock-LLM multi-turn edit task end to end via the CLI harness; live check (marked `#[ignore]`, needs keys): the same task against real Go and real Ollama, confirming streaming + tool calls on Go's chat-completions endpoint.
- Phase 2: `cargo tauri dev` manual script — open fixture repo, run "add a function + test", observe streamed tool trace, review diff in-thread, approve one elevated command, accept changes, verify files on disk; switch provider Go→Ollama mid-project without restart; toggle dark/light/system with no unthemed flash; keyboard-only approval + diff pass; frontend component tests for diff rendering and approval flow.
- Phase 3: scripted check — launch three threads on one repo in parallel, confirm three worktrees exist, no cross-thread file interference, and merge/discard each independently; chaos check: kill the app mid-run and confirm threads recover or report cleanly on relaunch.
- Phase 4: define a sample skill and a 1-minute automation on a fixture repo; confirm scoped tool grants are enforced and results appear in the review queue, never auto-merged.
- Phase 5: install the signed macOS `.dmg` on a clean machine profile, verify first-run onboarding (including Go-key entry), keychain key storage, and update check; highest-risk validation is the Phase 1 sandbox/approval suite — if traversal or approval bypass exists, nothing above it is trustworthy.

## Risks / Rollback

- AutoAgents is pre-1.0 (0.4.x): APIs may shift. Mitigation: pin exact versions, isolate all SDK touchpoints behind the `core` crate boundary, budget upgrade time per phase.
- Go session header is per-conversation but the AutoAgents header hook is static. Mitigation: Phase 1 spike decides (upstream contribution, thin injecting layer, or static fallback); fallback keeps Go functional with degraded prompt caching.
- Go is a paid subscription with usage limits and an evolving model table (some models on `/responses`, some on chat-completions). Mitigation: model IDs from settings + `/v1/models` refresh, never hardcoded; chat-completions models only for v1; surface limit/quota errors in the UI with a provider-switch affordance.
- Prompt-injection via repo content driving tool abuse. Mitigation: default-deny sandbox, approval for shell/network/deletion, worktree isolation, and no auto-merge; treat the approval UX as security UI.
- Long-running parallel agents exhaust tokens/time. Mitigation: per-thread budgets, global concurrency cap, cancellable runs, visible cost/status per thread.
- macOS signing/notarization friction delays distribution. Mitigation: unsigned local builds unblock all dev/P1-P4 validation; signing only gates Phase 5.
- Rollback: each phase merges only when its validation passes; Phase 3+ features sit behind feature flags so a bad parallel-runner or automation can ship disabled without reverting the app.

## Open Questions

1. Product identity: is "Themis" (matching the `ThemisOS` repo) the app name, or a codename? (Default if unanswered: codename Themis, user-facing name TBD before Phase 5.)
2. Billing: pure BYOK where the user brings their own OpenCode Go subscription key as the default, plus optional other-provider keys? (Default: yes, pure BYOK.)
3. Is there an Apple Developer ID / signing identity available for distribution, or should Phase 5 target unsigned distribution first? (Default: unsigned until identity exists.)
4. Visual identity: approve the proposed gold-accent dark-first "Ink + Gold" direction, or prefer a different brand direction (e.g. neutral blue, Codex-adjacent)? (Default: proposed tokens; adjustable before Phase 2 regardless.)

## Sources

- https://liquidos.ai/
- https://liquidos-ai.github.io/AutoAgents/
- https://liquidos-ai.github.io/AutoAgents/quick-start/
- https://liquidos-ai.github.io/AutoAgents/architecture/
- https://liquidos-ai.github.io/AutoAgents/llm-providers/overview/
- https://liquidos-ai.github.io/AutoAgents/core-concepts/tools/
- https://liquidos.ai/blog/codeact-executor-autoagents
- https://raw.githubusercontent.com/liquidos-ai/AutoAgents/main/crates/autoagents-llm/src/providers/openai_compatible.rs
- https://raw.githubusercontent.com/liquidos-ai/AutoAgents/main/crates/autoagents-llm/src/builder.rs
- https://openai.com/index/introducing-the-codex-app/
- https://opencode.ai/
- https://opencode.ai/go
- https://opencode.ai/docs/go/
- https://opencode.ai/docs/sdk/
- https://opencode.ai/docs/server/
- https://v2.tauri.app/
- https://v2.tauri.app/develop/calling-rust/
- https://www.electronjs.org/
- https://tailwindcss.com/
- https://ui.shadcn.com/
