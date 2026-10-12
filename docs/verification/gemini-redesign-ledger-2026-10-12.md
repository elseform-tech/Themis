# Redesign and task-ledger verification — 2026-10-12

Branch `codex/gemini-redesign-ledger`, created from `codex/bounded-tool-context` at `1298db9` (that branch is not yet merged into `main`, and this work refactors its compaction code). Worktree: `../ThemisOS-redesign`.

## Scope

Backend (`01578bf`):

- One runtime task ledger (`crates/core/src/runtime/ledger.rs`) replaces the separate completed-action, bounded-read and evidence-location caches. Legacy checkpoint blocks restore into it.
- Internal `update_plan` tool for production runs; the plan is kept first in the ledger.
- The model summary is narrowed to `Goal/constraints`, `Unresolved work`, `Next action` (old five-section checkpoints still validate).
- Freshness receipts replace byte-identical repeats still in context; the separate archive duplicate counter is removed (the unchanged-result guard already stops identical repeats); the 64-call recovery cap stays.
- Maintenance may reduce results a checkpoint already summarized (`summarized_until`); newer observations stay protected.
- Content-free `context_budget` run event before each bounded request.

Frontend (`e28a779`, `2498449`, `3729552`, `ff4ee22`): neutral palette with a warm gradient, pill composer with **+** menu, header model pill, greeting with starters, "Show thinking" chips, Copy action, checkpoint divider, context meter with heads-up, review side panel with a grouped/filterable tree and viewed progress, pill/card utility screens, ⋮ thread menus, Bubbles companion.

Not done (deliberately): a dedicated `recall` tool (it would replace `read_file` JSONL selectors and needs its own migration); a degraded continuation when summarization fails (left as a product decision; the run still stops and preserves originals).

## Automated checks

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | passed |
| `cargo test --workspace --no-fail-fast -- --skip entered_keys_survive_new_store_and_can_be_forgotten` | 478 passed, 2 failed, 3 ignored |
| Same two failing binaries with an empty `HOME` (`automation_e2e`, `bridge_e2e`) | 8/8 and 7/7 passed |
| `cargo llvm-cov --workspace --lcov …` (empty `HOME`) | see Coverage |
| `npm --prefix web run lint` | passed |
| `npm --prefix web test` | 317 passed (32 files) |
| `npm --prefix web run build` | passed (existing chunk-size warning) |
| `ruff check quality_dashboard` | passed |
| `python3 -m unittest quality_dashboard.test_dashboard` | 14 passed |

The two failures (`automation_runs_use_shared_checkout`, `scripted_write_file_run_finishes_and_shows_in_diff`) are environmental and pre-existing: the unmodified base also fails `automation_runs_use_shared_checkout`. User-level skills under the real home directory are materialized into the test project's `.themis/plugin-files/`, the diff reaches 361 files and truncates at 200 before `hello.txt`. With an empty `HOME` both pass. Showing materialized plugin files as changes is a separate product issue.

New or changed regression cases:

- Core: five ledger unit tests (plan validation, plan survives restart, user text cannot inject entries, legacy blocks restore, actions keep room and evicted reads keep locations); maintenance watermark test; existing cache tests moved to the ledger API with the same invariants.
- Runtime integration: repeated reads still in context receive a freshness receipt pointing at the earlier call.
- CLI end to end (`attachments_e2e::cli_plan_survives_checkpoint_in_task_ledger_and_budget_events_reach_clients`): an isolated app server and `themis` CLI run a thread whose plan survives a real checkpoint, and every bounded request emits one `context_budget` whose breakdown sums to its used tokens without content.
- Web: reducer budget/divider handling, context meter states, file tree filter/viewed, Bubbles speech.

## Code health

`rust-code-analysis` 0.0.25 over 119 product files (inline Rust tests included in source LOC/AST): mean cyclomatic 2.55, cognitive 2.17, maintainability 73.2 (base: 116 files, 2.55 / 2.17 / 73.1).

| File | Base LOC · cyc / cog / MI | Now |
| --- | --- | --- |
| `runtime.rs` | 6166 · 2.78 / 2.68 / 66.9 | 6064 · 2.67 / 2.39 / 66.8 |
| `runtime/ledger.rs` | — | 619 · 2.72 / 3.07 / 72.5 |
| `runtime/maintenance.rs` | 390 · 2.75 / 2.27 / 60.9 | 457 · 2.61 / 2.12 / 61.2 |
| `ThreadConversation.tsx` | 224 · 3.94 / 4.62 / 67.5 | 243 · 3.46 / 3.64 / 71.3 |
| `ThreadComposer.tsx` | 197 · 3.46 / 2.91 / 73.7 | 165 · 3.31 / 3.27 / 73.2 |
| `DiffReview.tsx` | 366 · 2.86 / 3.11 / 73.6 | 401 · 2.84 / 3.31 / 74.0 |
| `FileTree.tsx` | 98 · 1.71 / 1.40 / 69.2 | 158 · 2.35 / 2.20 / 76.3 |

Compaction code is simpler per function, not smaller overall: the ledger file includes its tests, legacy-checkpoint migration and the plan tool. `ThreadView.css` and `Sidebar.css` were consolidated to one definition per selector, removing 33 dead selectors.

## Browser verification

Vite dev server on port 1431 with a mocked Tauri bridge (fixture projects, threads, history, 23-file diff, automations, review items) driven by Playwright at 1440×900, 1100×720 and 960×640, with and without reduced motion:

- Greeting, starter chip filling the draft, send, then live events: warm meter and one-time heads-up near the threshold, run ring, "Compacting…" chip, "Context compacted · 82% → 35%" divider, Copy and model label on the reply.
- Meter popover: breakdown, reserves and thresholds.
- Review side panel: grouped tree, filter, Unviewed count, viewed progress.
- Integrations, Automations, Review queue and Settings with pill tabs and cards.
- Bubbles picker (Honey, Pearl, Ink) and the "All done!" speech note.
- Keyboard: Tab reaches the ⋮ thread menu and project actions; Enter opens Rename/Remove. Reduced motion stops the gradient and wraps titles.

A narrow-sidebar truncation found here was fixed (project actions now overlay on hover/focus). The only console noise is the mock's missing native window API ("Companion unavailable"), which does not occur in the app.

## Not verified

- Native Tauri app: not launched. The desktop app has no data-directory override, so a native run would use the real app data, keychain and scheduled automations. Window chrome, the floating desktop Bubbles window and native diff/merge flows remain unverified.
- Live model behavior: no real-provider run; whether `update_plan` and the narrower summary reduce repeated inspection needs the existing 24k/200k live trials.
- Frontend line coverage: not configured.
