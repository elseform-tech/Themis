# Repository Guidelines

Rust, Tauri, and React/TypeScript code belong in their owning layers; preserve cross-layer behavior.

## Project Structure

- `crates/core/`: runtime, providers, tools, skills, CLI, and integration tests.
- `crates/desktop/`: Tauri app, tests, shared `tests/common/` fixtures, and icons.
- `web/src/`: React UI, state, and adjacent tests. `docs/`: project documentation.
- `quality_dashboard/`: local quality dashboard and its tests.

## Configuration and Integrations

Read [docs/configuration.md](docs/configuration.md) and [docs/plugins.md](docs/plugins.md) before changing configuration, approvals, discovery, imports, or diagnostics.

- Runtime configuration belongs in `crates/core/src/configuration.rs`; desktop and CLI share the resolver through the app server. Preserve defaults → user `runtime.jsonc` → project `.themis/config.jsonc` → explicit run override, validation, and field provenance. Arrays replace earlier arrays; project approval rules may only add restrictions. Capture configuration and approval mode once per run.
- Global plugins, skills, MCP connections, and hooks use the existing revisioned app-data `plugins/registry.json`. Project imports use `.themis/plugins/`. Do not introduce a second writable MCP/plugin registry or put integration definitions in `runtime.jsonc`. Use validated store APIs and expected revisions for edits.
- User skill discovery uses `~/.agents/skills`, `~/.codex/skills`, and `~/.claude/skills`. Configured `skills.paths` changes project discovery only. Keep project paths bounded and reject traversal and symlink escapes.
- Advanced import inspection exposes compatibility reasons and offending fields. Strict CLI imports require partial-import opt-in; one-click catalog installation installs supported components and records skipped capabilities. Malformed or unsafe definitions still fail. Imported MCP servers and hooks begin disabled.
- Keep the composer shield limited to YOLO/Custom selection, with no editor links or advanced configuration controls. Settings → Permissions only opens the global `runtime.jsonc` in the chosen text or code editor, shows whether it was read, and lists problems with it; there is no in-app configuration editor, except Settings → Browser and Settings → Web, which edit only the `browser` and `web` blocks in place through field-level writes. Custom applies ordered rules and interactive approvals (Allow once, Allow in this chat, Deny; "in this chat" grants live in the chat's own app-data `chat_grants/` file, never cover destructive variants, and lose to deny rules); YOLO bypasses harness tool restrictions, approval prompts and the OS sandbox's walls (it runs an open plan: writes anywhere, secret reads, network), except that agent writes to the app data and the project's `.themis/` stay refused, and the Settings › Browser permissions' Block (and the Developer mode switch) still apply; a YOLO-only system prompt cautions against unrequested destructive actions. Configuration and mode changes affect subsequent runs, not active snapshots. Hooks and external instructions never grant permissions.
- Diagnostics are bounded, rotating JSONL under app-data `logs/`, separate from runtime JSONC and transcripts. Log metadata and correlation IDs, never prompts, tool payloads, provider bodies, or credentials. Redact actual referenced environment values, including variables with arbitrary names. Keep debug stderr bounded and filtered. Do not expose raw configuration editors or log viewers in the desktop UI. Plugin/skill catalog installation should take one click; MCP diagnostics belong to advanced CLI/file workflows; MCP file configuration is the CLI, or Integrations › MCP › Edit as mcp.json, a revision-checked copy of your personal servers applied through the store on save (never a second registry).
- Compaction execution controls belong in captured runtime configuration. Keep per-document job deadlines separate from the overall attachment-batch deadline; all stages/retries share their job deadline. Completed summary cache identity binds semantic inputs and resolved prompts, not execution-only timeout/backoff/retry settings. Preserve validated originals and completed stages on failure; never install a partial batch or expose a manual retry-compaction flow.
- Maintenance must preserve delivered ordinary observations (including plain reads, shell and MCP results) until semantic summarization consumes them. Offloading invocation arguments must retain original/recovery classification without granting permissions. Check net savings before publishing maintenance snapshots; cover mixed protected/redundant batches and retained checkpoint tails in provider-wire and shared CLI regressions.
- New features come with the controls and visibility they need: a user-facing setting or runtime configuration (in the existing resolver, Settings section or app setting) when people may reasonably want to change the behavior, and bounded diagnostics events (metadata only, per the logging rules) or a status the UI shows when the feature can fail or wait. Skip either only when it would add nothing.
- Add focused shared-server/CLI regression checks for backend changes and browser/native checks for UI changes. Update the configuration, integration, and user guides with changed fields, limits, scope, and behavior. Label benchmark audits separately from automatic runtime checks; preserve ongoing live tests.

## Prompts

Model-facing text (system prompt pieces, summary and compaction prompts, goal prompts, runtime nudges and notes, tool-result notes, eval prompts) lives in `crates/core/builtins/prompts/*.md`, one prompt per file, loaded through `crates/core/src/prompts.rs` (`include_str!`, `{{snake_case}}` placeholders filled with `prompts::fill`). Never write a new prompt inline in Rust or TypeScript: add a file and a constant, keep only assembly (separators, optional parts) in code, and add the file to the inventory test. Files have no trailing newline. Text the runtime parses back from saved transcripts (checkpoint envelopes, ledger markers, receipt notes, the goal turn marker) stays in code and must not change. Changing a prompt changes model behavior: verify the constructed system message and request path, update byte-identity tests deliberately, and run the relevant evals. See [docs/prompts.md](docs/prompts.md).

## Desktop UI

Follow [docs/ui-rules.md](docs/ui-rules.md) and [DESIGN.md](DESIGN.md) for layout, tokens and approved treatments.

- Keep every screen clean: each visible word, badge, section and control must tell the person something about the current task or let them act. Follow DESIGN.md "Only what earns its place" for new and changed UI everywhere, not just Settings, and remove existing elements that add nothing when you touch a screen.

- Workspace is a read-only monitor of what agents write: no accept, discard, merge, commit, push or other state-changing Git actions. The exceptions are the Tasks tab, where Stop (current run) and Pause (turn an automation off) are allowed for the current chat (nothing else there changes state, and neither control undoes completed work), and the Browser tab, where the person may drive the chat's built-in browser (address, navigation, input, take over and hand back, DevTools) without changing project files. File and directory reads stay bounded and inside the project; the change list shows only readable text files.
- Per-turn edited files come from the backend `files_changed` event (before/after fingerprints of readable changes), not from parsing tool summaries.
- Deleting a project moves its folder to the Trash and never deletes in place; the built-in Themis project cannot be deleted. Projects live in the fixed projects folder, so there is no folder-picker flow.
- The Bubbles companion is an assistant, not a persistent agent; keep its speech to the documented state notes.

## Build, Test, and Development

Run from the repository root:

```sh
python3 -m pip install -r requirements-dev.txt
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
mkdir -p coverage
cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten
npm --prefix web run lint
npm --prefix web test
npm --prefix web run build
ruff check quality_dashboard
python3 -m unittest quality_dashboard.test_dashboard
npm --prefix web run dev
(cd crates/desktop && npx tauri dev)
python3 -m quality_dashboard.dashboard
(cd fuzz && cargo +nightly fuzz build --sanitizer none)   # optional; nightly only
```

Run checks and start local apps with these root-level commands.

## Style and Testing

Use `rustfmt`, Rust `snake_case`, and `PascalCase` types/components. Frontend uses two spaces and `camelCase`. Required linters are Clippy, ESLint (React Hooks), and Ruff. Rust tests use the built-in harness, web uses Vitest, and dashboard uses `unittest`; no coverage threshold is configured. Keep `*.test.tsx` beside frontend modules and Rust integration tests in crate `tests/`. Add focused regression cases.

Code that parses untrusted input (model output, provider JSON, Git output, checkpoint history, paths from the UI) gets a property test, and a fuzz target when it is a parser; see [docs/fuzzing.md](docs/fuzzing.md). Commit every fuzz crash as a regression test.

Exercise backend features through the local app server and `themis` in isolated automated tests. Keep UI and OS verification separate from the dashboard's automated test inventory. Add a focused CLI end-to-end case for a changed backend flow and report any unverified UI behavior explicitly.

For long-chat history diagnostics, prefer `get_thread_history_page` with a byte bound, the returned snapshot watermark and a progressing cursor. Carry unfinished JSONL records between pages and decode only complete lines. Paging preserves exact durable history; the compatibility `get_thread_history` adapter still reassembles the full result in client memory. See `docs/configuration.md` for the shared API contract.

After every merge to `main`, run the live check in [docs/live-verification.md](docs/live-verification.md) on the freshly built app with real (free or cheap) models: the desktop app by hand, long tasks through the real app server and CLI (`python3 -m evals.hard run --via app`), and the short tool tasks. Use an isolated HOME and data folder, never the real app data or Keychain. Report it as a benchmark audit and fix any bug it finds on its own branch.

Before considering any task done, run fresh, relevant verification and read the complete output. For UI changes, verify the affected journey in a browser or native app when available; for system-prompt changes, verify the constructed system message and the LLM request path. Report exact commands and results, and label any remaining evidence as unverified.

## Quality Dashboard

Maintain `quality_dashboard/` as the always-on product-health companion. Keep it easy to launch and aligned with source and tests. Product metrics cover `crates/` and `web/src/`, excluding dashboard/tooling. Sort all analyzed app files by LOC both ascending and descending. Inline Rust `#[cfg(test)]` code stays in source LOC/AST; label that scope. Separate dashboard self-tests. Use `rust-code-analysis-cli` 0.0.25; show unavailable analysis instead of estimates. Preserve metric definitions without imposing a fixed maintainability threshold. Generate Rust line coverage with open-source `cargo-llvm-cov`; test counts are not coverage.

Before marking work done, run dashboard metric tests and relevant checks, refresh it, and inspect code health and complexity for the touched modules and overall product. Review cyclomatic and cognitive complexity, maintainability, and available line coverage; make only reasonable changes that improve clarity or address a concrete risk, and call out regressions or missing metrics rather than inventing values. Do not trim working code merely to raise a score. Run relevant CLI end-to-end checks and report their outcomes with the automated test inventory. Report failures, skips, missing coverage, and estimates accurately.

## Commits, Pull Requests, and Security

Before making changes for a new task, create a dedicated `codex/<task-name>` branch from up-to-date `main`. When continuing an existing task, use its branch. Check the working tree first and preserve unrelated local changes; never implement directly on `main`.

Commit regularly as each focused task or logical subtask is completed and relevant checks pass. Keep commits small and reviewable, include the corresponding tests and documentation, inspect the staged diff before committing, and stage only changes belonging to that task. Do not defer all work to one large final commit.

Use short imperative commit subjects such as `feat:` and `fix:`. PRs summarize changes, list checks, include relevant UI screenshots, and link issues when available. Never commit credentials or test with real ones. Secrets use macOS Keychain; OpenCode Go may read `OPENCODE_KEY` from the app environment.
