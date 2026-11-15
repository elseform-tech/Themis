# Visions quiet channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a team channel quiet and plain: the chief talks only when handing work out and when it is done, approvals live in the worker's thread, the channel is left/right bubbles with a small floating chief, the team sits in the sidebar, autonomy sets the approval mode, and the 3D cloud is the icon's outline with eyes only.

**Architecture:** Server: workers' approvals and permission questions stop waking the chief; a chief's channel message records the workers it tasked in that run; a worker that ends a task turn without doing anything wakes its chief; a channel's approval mode is derived from its autonomy and a Full access switch. Web: the channel view drops its header for a small name and a floating chief, renders bubbles and thread indicators, moves approvals into the worker panel, adds a Team section to the channels bar, and swaps the Approval mode field for Autonomy and Full access. The 3D character is rebuilt from the icon outline.

**Tech Stack:** Rust (crates/desktop state, crates/core prompts and visions), React + TypeScript (web/src/screens/visions), three.js.

**Spec:** `docs/superpowers/specs/2026-11-13-visions-teams-design.md` (teams design) amended by the owner's approved boards on the design canvas https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq: TeamChannel v87 (thread indicator, approval in the worker's thread), ChannelPlain, ChannelAutonomy and Cloud3D (v91, titled "Approved 2026-11-15"). The boards bind the UI.

## Global Constraints

- Work on branch `codex/visions-design` in `/Users/cutedandelion/Documents/ChatGPT/ThemisOS-teams`; build with `CARGO_TARGET_DIR=/Users/cutedandelion/Documents/ChatGPT/ThemisOS-target-visions CARGO_BUILD_JOBS=4`.
- Prompts live in `crates/core/builtins/prompts/*.md` (no trailing newline), constant in `prompts.rs`, row in `prompts/tests.rs`.
- No hint text in the UI; every visible word must earn its place (DESIGN.md "Only what earns its place").
- No backward compatibility: rename or remove fields outright (`Channel.approval_mode` goes); report what breaks.
- Ratchets (`python3 -m quality_dashboard.ratchets`) and chained `&&` checks before every commit; stage paths explicitly.
- Isolated HOME for any live run; free model (`beam-free`); never the real app data or Keychain.
- Locked rows stay the person's whatever the autonomy: sandbox escapes, sensitive files, deny rules, destructive steps (Auto's question box), anything that leaves the computer, spend past the budget.
- Clouds show only where they say who is talking: the floating chief, the welcome, the Team section rows, the worker panel header, profiles. Not on channel rows, bubbles or thread indicators.
- **Open owner decision (Task 5):** with no `approval.auto_model` and no memory model, Auto either (A) reviews with the chat's own model or (B) the channel falls back to Custom. Task 5 implements whichever the owner picks; do not start it before the answer.

## Review Focus

1. A worker chat's permission question (not an approval box) must still reach the person once the chief stops relaying it: it shows in the worker's panel and turns the worker's indicator and sidebar dot to "Needs you".
2. An approval waiting while the worker panel is closed: the thread indicator says "Needs you", the worker's sidebar dot is amber and the Visions rail count includes it.
3. A chief run that tasks the same worker twice, or two workers: `tasked` lists each worker once, in first-task order; one indicator per worker under that message.
4. Changing a channel's autonomy or Full access while a worker chat runs: the chat's approval mode changes for its next action, not only for new chats.
5. A worker woken for something other than a task (a redirect, a resume) that ends quietly must not wake the chief as "silent"; only a `Tasked` wake counts.

---

### Task 1: Workers' approvals and permission questions no longer wake the chief

**Files:**
- Modify: `crates/desktop/src/state/visions/wakes.rs` (`visions_question_asked`, `visions_approval_waiting`)
- Test: `crates/desktop/src/state/visions/wakes.rs` tests module; `crates/desktop/tests/visions_e2e/` (team suite)
- Docs: `docs/configuration.md` (Visions wakes), `docs/user-guide.md`

**Interfaces:**
- Produces: a work chat owned by a worker (`owner.vision_id != chief`) never wakes anyone for an approval box or a permission question. A chat owned by the chief itself keeps today's `ApprovalWaiting` (none exist in teams, but single-member channels may).

- [ ] **Step 1: Write the failing tests** — `a_worker_approval_wakes_no_one`: a worker's chat opens an approval box; assert no wake recorded for the chief and `wakes_used` unchanged. `a_worker_permission_question_wakes_no_one`: same for a permission question. E2E in the team suite: a worker's chat that needs one write approval and then finishes produces exactly two chief lines (hand-off, done) and no "paused after many turns" notice with `max_wakes` 6.
- [ ] **Step 2: Run them, watch them fail** — `cargo test -p themis-desktop --lib a_worker_approval_wakes_no_one a_worker_permission_question_wakes_no_one`. Expected: FAIL (a wake is recorded).
- [ ] **Step 3: Implement** — in both functions return early when `owner.vision_id` is a worker of its channel (not the chief). Keep the `first_approval_notice` bookkeeping only for the chief-owned path.
- [ ] **Step 4: Run tests and the visions e2e** — `cargo test -p themis-desktop --lib visions && cargo test -p themis-desktop --test visions_cli_e2e`. Expected: PASS (YOLO e2e may flake under load; rerun alone).
- [ ] **Step 5: Docs and commit** — configuration.md: approvals and permission questions in a worker's chat show in that worker's thread and wake no one. `git add` the files; `git commit -m "fix: a worker's approvals no longer wake its chief"`.

### Task 2: A chief's message names the workers it tasked

**Files:**
- Modify: `crates/desktop/src/state/visions/mod.rs` (`ChannelEntry::Vision` gains `#[serde(default, skip_serializing_if = "Vec::is_empty")] tasked: Vec<String>`), `crates/desktop/src/state/visions/runs.rs` (`visions_post_reply`), the `assign_task` handler (records the worker on the running chief's run)
- Modify: `web/src/lib/visions.ts` (`ChannelEntry` vision variant gains `tasked?: string[]`)
- Test: runs/team tests; `web/src/lib/visions` type tests if any

**Interfaces:**
- Consumes: the chief's run id / thread id at `assign_task` time.
- Produces: `ChannelEntry::Vision { vision_id, text, tasked }` — worker ids tasked during the run whose final reply this is, deduplicated, in first-task order.

- [ ] **Step 1: Failing test** — `a_chief_message_lists_the_workers_it_tasked`: a chief run calls `assign_task` for Rook, then Wren, then Rook again, then finishes with text; the posted Vision entry has `tasked == [rook, wren]`. A run that tasks no one posts `tasked` absent.
- [ ] **Step 2: Run, watch fail** — `cargo test -p themis-desktop --lib a_chief_message_lists_the_workers_it_tasked`. Expected: compile error or FAIL.
- [ ] **Step 3: Implement** — keep a per-run `Vec<String>` in the visions cache keyed by the chief's thread id; `assign_task` pushes if absent; `visions_post_reply` takes and clears it.
- [ ] **Step 4: Run** — `cargo test -p themis-desktop --lib visions && cargo test -p themis-desktop --test visions_cli_e2e`. Expected: PASS.
- [ ] **Step 5: Commit** — `feat: a chief's message names the workers it tasked`.

### Task 3: A worker that ends a task turn without doing anything wakes its chief

**Files:**
- Create: `crates/core/builtins/prompts/visions-wake-worker-silent.md` (wake line text), constant `VISIONS_WAKE_WORKER_SILENT` in `crates/core/src/prompts.rs`, row in `prompts/tests.rs`
- Modify: `crates/desktop/src/state/visions/wakes.rs` (`WakeReason::WorkerSilent(String)`, its `line()` and log name `worker_silent`), `runs.rs` (on a worker run's end)
- Test: wakes/runs tests; team e2e
- Docs: `docs/agent-weaknesses.md` (the Oli row: harness now catches it), configuration.md

**Interfaces:**
- Produces: when a worker's run that was woken with `WakeReason::Tasked` ends (Finished/Incomplete) having started or driven no chat and added no team-log entry of its own, its chief is woken with `WorkerSilent(worker_id)`. Line text (prompt file): `worker_silent {{worker}}: your worker ended its turn without starting the task or reporting; read its team log and give the task again or tell the person.`

- [ ] **Step 1: Failing tests** — `a_tasked_worker_that_does_nothing_wakes_its_chief` (mock provider replies plain text, no tool call; chief wake recorded with reason `worker_silent`). `a_redirected_worker_that_ends_quietly_wakes_no_one` (wake reason not `Tasked`). `a_tasked_worker_that_reports_wakes_no_silent` (the worker calls its report; no `worker_silent`).
- [ ] **Step 2: Run, watch fail.** Expected: FAIL (no wake).
- [ ] **Step 3: Implement** — record at run start whether the reasons include `Tasked`, the worker's team-log length and its owned-chat count; at run end compare; wake the chief when nothing changed.
- [ ] **Step 4: Run** — lib visions, prompts tests (`cargo test -p themis-core prompts`), visions e2e. Expected: PASS.
- [ ] **Step 5: Commit** — `fix: a worker that ends a task turn idle wakes its chief`.

### Task 4: Autonomy sets a channel's approval mode; Full access replaces it

**Files:**
- Modify: `crates/desktop/src/state/visions/mod.rs` (`Channel`: remove `approval_mode`, add `#[serde(default)] full_access: bool`), `drive.rs` (adopt with the derived mode), `store.rs` (save re-applies the mode to the channel's running work chats), `admin.rs`, `admin_teams.rs`, `wakes.rs`, `runs.rs` (drop the field from constructors)
- Create: `fn channel_approval_mode(channel: &Channel, config: &VisionsConfig) -> ApprovalMode` in `crates/desktop/src/state/visions/mod.rs`: `Yolo` when `full_access`; `Custom` when the effective autonomy (channel's, else Settings') is `Ask`; otherwise `Auto`.
- Modify: `web/src/lib/visions.ts` (`ChannelInfo`: drop `approval_mode`, add `full_access: boolean`), `NewChannelSheet.tsx`, `ChannelComposer.tsx` (no shield), `ChannelDetails.tsx` (Autonomy select with the five levels and their mode note, Full access switch; Approval mode row removed)
- Test: `mod.rs` unit tests; `TeamScreens.test.tsx`; visions e2e
- Docs: configuration.md, user-guide.md

**Interfaces:**
- Produces: `channel_approval_mode` (above); `save_channel` accepts `full_access`.

- [ ] **Step 1: Failing tests** — Rust `approval_mode_follows_autonomy`: Ask→Custom, Small/Most/Nearly-all/Custom→Auto, full_access→Yolo whatever the autonomy. `changing_autonomy_changes_a_running_chats_mode`: a running worker chat's thread mode follows a save. Web `channel_details_set_autonomy_and_full_access` (the select lists Always ask … Custom, the switch saves `full_access`), `the_channel_composer_has_no_shield`.
- [ ] **Step 2: Run, watch fail.**
- [ ] **Step 3: Implement** as in Files.
- [ ] **Step 4: Run** — lib, e2e, `npm --prefix web test`, `npm --prefix web run lint`.
- [ ] **Step 5: Commit** — `feat: a channel's autonomy sets its approvals; Full access is its own switch`.

### Task 5: Auto with no reviewer model (OWNER DECISION A or B)

**Files:** A: `crates/desktop/src/state/auto_review.rs` (`ask_model` and `auto_try_review` fall back to the chat's own model after `auto_model` and the first memory model). B: `channel_approval_mode` returns `Custom` instead of `Auto` when neither `approval.auto_model` nor a memory model is set.
- [ ] **Step 1: Failing test** — A: `auto_reviews_with_the_chat_model_when_none_is_set` (no auto_model, no memory models; review request goes to the chat's model; verdict allow). B: `no_reviewer_keeps_a_channel_on_custom`.
- [ ] **Step 2–4:** run, implement, run lib + `approval` tests + e2e.
- [ ] **Step 5: Docs and commit** — configuration.md Auto paragraph; `fix: ...` per choice.

### Task 6: The plain channel view (no header, floating chief, bubbles, thread indicators)

**Files:**
- Modify: `web/src/screens/visions/ChannelView.tsx` (no `ChannelHeader`; renders `ChannelTop` and keeps `ChiefWelcome` for a fresh channel; approvals removed from the log), `ChannelEntry.tsx` (person → right bubble `.themis-bubble-me`; vision → left bubble `.themis-bubble-chief`; no avatar or name; `tasked` → one `ThreadIndicator` per worker under the bubble), `Visions.css`
- Create: `web/src/screens/visions/ChannelTop.tsx` (`# name` button opening the details; floating chief `CharacterAvatar` 40 live, name, state dot beside it, opening the profile; a fade band under them), `web/src/screens/visions/ThreadIndicator.tsx` (`{worker}'s thread`, state dot and label — Working, Needs you, Done, Idle — and a chevron; opens the worker panel; state from the worker's chats and their approvals/questions)
- Delete: `ChannelHeader.tsx`, `WorkerChips.tsx` and their CSS
- Test: `TeamScreens.test.tsx`, `VisionsView.test.tsx`

- [ ] **Step 1: Failing tests** — `the_person_writes_on_the_right_and_the_chief_on_the_left` (no avatar or name in either bubble); `a_handoff_shows_one_indicator_per_tasked_worker` (state "Needs you" while that worker's chat waits on an approval, "Done" after); `the_channel_name_opens_the_details`; `the_floating_chief_opens_its_profile`; `approvals_are_not_in_the_channel_log`.
- [ ] **Step 2: Run, watch fail** — `npm --prefix web test -- TeamScreens VisionsView`.
- [ ] **Step 3: Implement** per the ChannelPlain board (sizes: floating chief 40px; fade band 78px; log top padding 84px; bubble max width 72%).
- [ ] **Step 4: Run** — web test, lint, build.
- [ ] **Step 5: Commit** — `feat: a plain team channel: bubbles, a floating chief, thread indicators`.

### Task 7: The worker panel in bubbles, with its approvals and questions

**Files:**
- Modify: `web/src/screens/visions/TeamThread.tsx` (header: worker cloud, "{worker} with", chief's small cloud, "{chief}"; chief lines right bubbles, worker lines left with a small "name · time" above; the worker's chats' approvals (`ChannelApproval`) and permission questions below the log; chat link at the bottom)
- Test: `TeamScreens.test.tsx`

- [ ] **Step 1: Failing tests** — `a_workers_approval_waits_in_its_thread` (Allow once answers it); `a_workers_permission_question_waits_in_its_thread`; `the_thread_header_shows_both_clouds`.
- [ ] **Step 2–4:** run, implement, run web test/lint/build.
- [ ] **Step 5: Commit** — `feat: a worker's thread holds its approvals`.

### Task 8: The Team section in the sidebar; channels without clouds

**Files:**
- Modify: `web/src/screens/visions/ChannelsBar.tsx` (channel rows `#` + name; under the list a `Team` section for the open channel: chief row with `CharacterAvatar` 22 without accessory, name, role, state dot → profile; worker rows indented with `CharacterAvatar` 18, name, role, state dot → worker panel; selected row highlighted), `Visions.css`; `CharacterAvatar.tsx` (`accessory={false}` below 40px)
- Test: `VisionsView.test.tsx`

- [ ] **Step 1: Failing tests** — `channels_list_without_clouds`; `the_team_section_lists_the_chief_then_its_workers`; `a_worker_row_opens_its_thread`; `the_chief_row_opens_its_profile`; `a_worker_waiting_on_you_shows_needs_you`.
- [ ] **Step 2–4:** run, implement, run.
- [ ] **Step 5: Commit** — `feat: the team in the sidebar`.

### Task 9: The cloud in 3D from the icon's outline, eyes only

**Files:**
- Modify: `web/src/screens/visions/three/buildCharacter.ts` (body = `THREE.Shape` from the outline `M13 39h22a8 8 0 0 0 .8-15.96A12 12 0 0 0 13.2 20.2A9.4 9.4 0 0 0 13 39Z` in a 48 box, `ExtrudeGeometry` depth ~10, bevel ~3 with 6 segments; accessories re-anchored to that outline), `three/faces.ts` (no mouth in any expression; Calm, Working, Needs you, Done by eyes and brows), `cast.ts` (2D bodies use the same outline), `CharacterAvatar.tsx` / `StillPicture.tsx`
- Test: `three/sceneRuntime.test.ts`, `cast.test.tsx`, `ChiefScene.test.tsx`

- [ ] **Step 1: Failing tests** — `every_expression_has_no_mouth` (faces.ts); `the_body_is_one_outline` (cast bodies equal the outline path for every character); `the_scene_builds_one_body_mesh`.
- [ ] **Step 2–4:** run, implement, run.
- [ ] **Step 5: Commit** — `feat: the cloud in 3D from its icon, eyes only`.

### Task 10: Whole-branch checks, live check and docs

- [ ] `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p themis-desktop --lib && cargo test -p themis-desktop --test visions_cli_e2e && npm --prefix web run lint && npm --prefix web test && npm --prefix web run build && python3 -m quality_dashboard.ratchets`
- [ ] `docs/live-verification.md`: update the Visions team rows (thread indicator, Team section, autonomy → approvals); `live_coverage` test passes.
- [ ] Real app, isolated HOME, `beam-free`, all four skies against the boards: a task without naming a worker; an approval with the panel closed (indicator, dot, rail count); Allow in the panel; autonomy Always ask vs Most decisions; Full access on/off; the floating chief and `# name`; the Team section; the 3D cloud in welcome, float and profile.
- [ ] Commit docs: `docs: the quiet team channel`.

---

## Addendum 2026-11-15 15:10 (owner): fewer approvals, requests that complete

Owner: approvals should stop destructive actions, not legitimate work; answered cards pile up; Take it on is annoying and results never come back.

### Task 12: Team chats do project work without asking; destructive steps still stop
- A team work chat whose run is in Auto (Task 4) gets two allow rules appended to its run's captured approval policy: `write` and `execute` risks proceed (after any deny rule; the sandbox stays: no network, no secrets, no writes outside the project). Destructive variants keep Auto's question box (only the person answers); sandbox escapes and sensitive files keep theirs; anything leaving the computer is the sandbox's block, answered only by the person's Allow once.
- The reviewer of a team work chat reads, as the person's words, the person's latest messages in its channel (bounded as today: 12, 6000 characters), so what it still judges (Custom-asks that are not plain writes or commands, for example MCP or browser steps) is judged against what the person asked.
- Tests: `a_team_chat_writes_and_runs_without_asking` (unit on the captured policy: allow rules present for a Vision work chat in Auto, absent for a plain chat and for Custom/YOLO), `a_destructive_step_in_a_team_chat_still_asks` (destructive variant → question box path unchanged), `the_reviewer_reads_the_persons_channel_words`.
- Docs: configuration.md (Visions approvals), user-guide.

### Task 13: Answered cards fold; dead cards go
- A `passed_on` card whose question is answered folds to one line: "{verb}: {short action}" (Allowed once / Skipped / Answered). A card whose chat no longer has the question open and that was never answered (the run moved on, was stopped, or the question expired) is not shown.
- Tests (web): `an_answered_question_folds_to_one_line`, `a_question_its_chat_dropped_is_not_shown`.

### Task 14: A chief takes another channel's request on itself
- Accepting a request from a channel of the same project is the receiving chief's own call (no `scope` gate) unless the request changes its own channel's goal or spends money, which it passes to the person as today. The prompt (`visions-chief.md`) says so; `requests.rs` drops the scope check for accept. Take it on / Decline stay on the line only while a request waits on the person.
- Tests: `a_chief_takes_a_request_on_without_the_person` (Rust), prompt tests; eval A/B before merge (prompt change).

### Task 15: Results come back
- New chief call `send_result {requestId, text}` (receiving chief, accepted request, once; 1–2000 characters): posts `request_result {request_id, channel_id, vision_id, text}` in the asking channel (announced), wakes the asking chief with `request_answered RID` (the entry shows the result), and a short line in the receiving channel. Grant: CHIEF_CALLS; refused for workers and for requests not accepted by this channel.
- Web: the asking channel shows a result card: "# design sent:" with the text.
- Tests: `a_result_reaches_the_asking_channel` (e2e through themis), grant test, web `a_request_result_shows_in_the_asking_channel`.
