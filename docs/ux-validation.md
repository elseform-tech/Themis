# Workspace UX validation — 2026-09-23

## Implemented

- Sidebar navigation for threads, skills, automations, review queue, settings and help; project-grouped threads with clipped long titles and inline new-thread action.
- Full collapse with 300 ms edge reveal, 700 ms leave delay, focus retention and keyboard toggle. Theme-aware Themis scale/galaxy header.
- System UI typography, 13 px conversation default, approximately 12 px navigation; compact controls and aligned composer model/effort selectors with icon Send/Stop.
- Direct workspace entry without onboarding. Name-only project creation under a configurable `~/ThemisOS/Projects` root, collision validation and Git initialization.
- Settings split into General, Appearance, Models & connections and Permissions; ordinary preferences save independently with errors and rollback.
- Diff sidebar and its entry points hidden. Worktree actions remain in Thread options. Tool calls live in the conversation in execution order, with expandable arguments and completion state; no separate activity area above the composer.
- Per-thread local drafts, conversation display, selected thread and scroll restoration. These are UI history, not a replay of an interrupted agent run.
- No keychain dependency or persisted API keys. Go uses `OPENCODE_KEY`, or entered keys remain in backend memory for the session. Existing keychain entries are untouched. Hashing a key would prevent using it for provider authentication.
- Stop checks safe boundaries, resolves pending approvals and frees run capacity. Merged threads cannot send new work into the main checkout.

## Local checks

- `cargo test --workspace`: 194 passed, 2 ignored (including a doc test).
- `cargo clippy --workspace -- -D warnings`: passed.
- `npm --prefix web run build`: TypeScript and Vite passed.
- `npm --prefix web test`: 136 passed across 16 files.
- Coverage added for project creation/collisions, settings rollback, session-only keys, stopping at approval, post-merge send rejection, inline tool ordering, quiet cancellation, sidebar reveal timing, composer model/effort selection and reasoning-effort request serialization/validation.

## Native Computer verification

Tested the local debug app at `target/debug/bundle/macos/Themis.app`, using the native Computer API and the actual Go service. This is local development evidence; the installed `/Applications/Themis.app` was not replaced.

1. Created the synthetic project **Themis UX Check** through the name-only dialog. An earlier write/approval/diff/merge journey completed before the diff sidebar was removed.
2. Selected **gpt-5.6-luna** and **Low** from the composer. The request reached Go but returned HTTP 503, `Endpoint is unavailable`. Effort serialization is covered by a local mock-server test; successful GPT inference is not claimed.
3. Selected **minimax-m2.5**. Sent a controlled request to write `react-loop-check.txt`, read it back, and report its returned contents.
4. The native approval dialog displayed the exact write arguments. Chose **Allow once**. The write and read both finished inline in the conversation; the final answer reported **ORBIT-17**.
5. Independently read the generated worktree file and confirmed its exact contents were `ORBIT-17`.
6. Started another read-only request, pressed Stop, observed the stopping state and then a released composer. Existing completed work remained available.
7. Collapsed the sidebar completely, revealed it at the left edge, and clicked Settings from the overlay. Tested text size 13 px, autosave, light/dark rendering and the model catalog dropdown.
8. Renamed the synthetic thread to **ReAct loop verified**; the title updated in both the header and sidebar. Final visual inspection confirmed clipped sidebar titles, compact composer, collapsed tool details, and no diff panel.

## Limits

- Effort overrides currently support Go/OpenAI GPT-5 and o3/o4 model families; other models use their provider default. Go GPT inference could not be verified because of the upstream 503.
- Stop finishes the current provider request or tool before ending; it is not immediate process termination.
- The native app uses the local Vite development server. Restarting Vite was required after filesystem changes were missed by its watcher.
- This work was not committed, pushed, signed, packaged for release, or installed over the user's existing application.

## Follow-up: streamed action narration and compact chat

- Removed the redundant thread heading/options/read-only banner and repeated role labels. Edit/remove controls now appear on sidebar thread hover/focus. The chat no longer exposes Merge.
- Added composer + and ⌘/Ctrl+Shift+O for new threads; Enter or ⌘/Ctrl+Enter sends.
- ReAct requests stream public text and tool-call deltas. Public updates and related tool calls are grouped under a few milestones. This is not a separate task-progress checklist. Reasoning-content deltas are discarded.
- Thinking/using-tool status shows elapsed time; pending tool calls shimmer. Reduced-motion disables shimmer and scale animation.
- Tool completion events now carry bounded output (32,768 characters, with a truncation notice); chevrons expose input and actual result/error. Fixed approval trace bookkeeping so completed calls do not remain visually active.
- Added a gently animated header scale on a persistent galaxy background and a matching macOS PNG/ICNS app icon. `scripts/generate-icon.swift` regenerates the local icon assets.
- Follow-up automated checks: 195 Rust tests and 136 frontend tests passed; TypeScript/Vite build and Clippy passed.
- Native verification: ⌘⇧O created a thread; ⌘Enter sent. A live Go/MiniMax request displayed successive list/search/read narrations interleaved with calls and a running 0:08 elapsed indicator. Expanding list and failed read showed their actual JSON result and missing-file error. The requested file was absent from that new worktree, so the model correctly reported that limitation rather than inventing contents.

## Milestones, formatted replies and persistent brand header

- Completed milestones collapse; the current milestone opens. A legacy run or
  model reply without a milestone heading uses a single activity bucket. Final
  answers remain outside the buckets. This is public activity, not private reasoning.
- Markdown headings/lists/tables, syntax-highlighted Python/Bash/code, Copy, and
  Mermaid diagrams with expandable source are implemented. Incomplete streaming
  fences remain code. Raw HTML is skipped; Markdown images are links; Mermaid
  uses strict rendering and a sandboxed frame with no network/script permissions.
- Diagram blocks keep their state across unrelated composer renders.
- The galaxy header uses the same stronger purple/blue background, light wordmark
  and gold animated scale in light/dark themes.
- Final automated checks: 195 Rust tests passed, 2 ignored; 144 frontend tests
  passed across 18 files; TypeScript/Vite, Clippy and native debug build passed.
  The Swift icon generator ran successfully.
- Native Computer: sent a live Go/MiniMax read/verify request in the synthetic
  project. Both read_file outputs contained STREAM-OK-23. Completed buckets were
  collapsed; expanding a bucket and its tool exposed the actual JSON output.
  The final table, highlighted Python/Bash, and Inspect → Verify → Done Mermaid
  diagram rendered in the app. Copy followed by paste into the composer reproduced
  the exact Python source; the unsent test draft was cleared.
- Native Computer: verified the unchanged bold header in dark and light settings,
  renamed the synthetic thread to Formatted responses verified from the sidebar,
  and opened then cancelled Remove. No thread was deleted.
- Dependency audit: production dependencies have zero reported vulnerabilities.
  Two pre-existing moderate development-tool advisories remain in Vitest/mocker;
  their offered fix is a breaking test-runner upgrade, outside this UI change.
  Mermaid is lazy-loaded; its largest diagram chunks trigger Vite's size warning.
