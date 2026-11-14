# Visions Phase 2 (teams) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each channel becomes a team: one chief of staff the person talks to, workers who take
the chief's tasks, `# administration` where Atlas builds teams, chiefs that cooperate across
channels and run routines, all drawn as the approved canvas boards.

**Architecture:** Built on the merged Phase 1 code (`crates/desktop/src/state/visions/`, the
Vision grant in `crates/desktop/src/server/agent/vision.rs`, `web/src/screens/visions/`). A
channel gains a chief and channel-level autonomy and budget; a worker is a Phase 1 Vision whose
wakes come from its chief's tasks and its own chats; the chief gets its own grant (reads, team,
company calls, never chat driving). Worker–chief exchanges live in bounded per-worker team logs.
Routines are a Visions-only timer beside the Phase 1 wakes. The 3D header is a bundled WebGL
scene, built last.

**Tech Stack:** Rust (themis-core, themis-desktop app server and `themis` CLI), React and
TypeScript (web), a bundled WebGL library (three.js, vendored into the web build), Python
(`evals/visions`).

**Spec:** `docs/superpowers/specs/2026-11-13-visions-teams-design.md` (read its "Design source"
section first: the four approved canvas boards bind every screen).

## Global Constraints

- The four boards on https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq (`TeamChannel`,
  `ChiefMoods`, `AdminChannel`, `ChiefProfile`, canvas version 81) are the UI reference: where
  text and a board differ, the board wins. Every UI task compares its screen with its board in
  Morning, Noon, Night and Classic, in the real app, before it is done. A design change needs the
  owner's yes on the canvas first.
- No hint text on any Visions screen (DESIGN.md "Only what earns its place").
- Plain voice for every agent-facing prompt the person reads the result of (Phase 1's rules).
- Approvals stay the person's; no agent switches on a hook or a command MCP server; the locked
  autonomy rows (`external`, `spend`) are always the person's.
- Model-facing text lives in `crates/core/builtins/prompts/*.md` (one per file, no trailing
  newline, a constant in `crates/core/src/prompts.rs`, the inventory test).
- Diagnostics are metadata only: ids, counts, reasons, outcomes; never message, task, soul or
  routine text.
- Ratchets: files ≤ 1000 code lines (2000 with tests), functions ≤ 100 lines, cognitive ≤ 25,
  cyclomatic ≤ 20; listed code only shrinks; never raise an entry to pass.
- No backward compatibility except the one-time store conversion in Task 1 (spec "What changes
  from Phase 1").
- Built-ins ship on macOS, Windows and Linux; the 3D library is bundled with the app; where WebGL
  is missing the still picture shows.
- Every backend change gets a focused CLI end-to-end case (`crates/desktop/tests/visions_e2e/`,
  one module per area; `visions_cli_e2e.rs` is at its line limit).
- Growth: team logs, request lines, routine history and owner lookups get a scale test counting
  work at two sizes (`docs/scale-tests.md`).
- Limits, defaults: team wake cap = `visions.max_wakes` (Phase 1 field, now counted per channel);
  requests between channels `visions.requests_per_day` default 10 (1–100); routines at most 10 per
  chief, never more often than once an hour (`visions.routines_per_chief` 1–50 default 10,
  `visions.routine_min_minutes` 60–1440 default 60); quiet hours `visions.quiet_hours`
  `{"start": "HH:MM", "end": "HH:MM"}` or null (default null); `soul.md` at most 16 KiB.

## Review Focus

1. **A chief and a worker keep handing work back and forth** (task, pushback, redirect, report…)
   with no person message: the channel's shared wake count reaches the cap, the server stops
   starting wakes in that channel and posts one notice; the person's next message resets it.
   Owner: Task 4, test `a_team_bouncing_work_stops_at_the_channel_cap_with_one_notice`.
2. **A worker's chat asks a question no agent may answer and the worker forgets to pass it on**:
   the server shows it in the channel, attributed to the chief, so it reaches the person.
   Owner: Task 4, test `a_question_no_agent_may_answer_reaches_the_person_anyway`.
3. **Two chiefs keep asking each other for work**: each channel's daily request cap stops it and
   both channels show why once. Owner: Task 5, test `requests_between_channels_stop_at_the_daily_cap`.
4. **A routine comes due during quiet hours, over budget, or after the app was closed for days**:
   it does not fire then, fires once at the next allowed moment, and never as a burst of missed
   runs. Owner: Task 7, test `missed_routines_fire_once_at_the_next_allowed_moment`.
5. **The Phase 1 store is converted and the app quits half-way**: the next start finishes the
   conversion, posts the `# administration` note once, and loses no channel, Vision, owned chat
   or log. Owner: Task 1, test `an_interrupted_conversion_finishes_once_on_the_next_start`.

---

### Task 1: Team store and the one-time conversion

**Files:**
- Modify: `crates/desktop/src/state/visions/mod.rs` (types), `store.rs`, `admin.rs` (`HQ` → `ADMINISTRATION`), `listing.rs`, `crates/core/src/configuration/visions.rs`
- Create: `crates/desktop/src/state/visions/convert.rs` and `convert_tests.rs`
- Test: `crates/desktop/tests/visions_e2e/teams.rs` (new module)

**Interfaces:**
- Produces:
  - `Channel { …, chief: String /* Vision id */, autonomy: Option<Autonomy>, custom: Option<BTreeMap<String,bool>>, daily_budget_usd: Option<f64> }`; `members` keeps every Vision in the team, chief included; workers are `members` minus `chief`.
  - `Vision` loses `autonomy`, `custom`, `daily_budget_usd` and `soul` (Task 2 moves souls to files); gains `look: Look { shape: String, color: String, accessory: String }` (accessory ids: `clipboard`, `crown`, `hard_hat`, `glasses`, `goggles`, `headphones`, `magnifier`, `none`).
  - `fn is_chief(&self, channel: &Channel, vision_id: &str) -> bool`, `fn workers(channel: &Channel) -> Vec<String>`.
  - `ADMINISTRATION: &str = "administration"`; it replaces `# hq` everywhere (Atlas its chief, no workers); listings sort it first.
  - Server: `list_channels` returns `chief`, `autonomy`, `daily_budget_usd`; `save_channel` validates that `chief` is a member and not the admin outside `# administration`.
  - Config: `visions.requests_per_day`, `visions.routines_per_chief`, `visions.routine_min_minutes`, `visions.quiet_hours` (validated, provenance, a project may only lower them).
- [ ] **Step 1:** Write the tests:
  - unit `a_channel_names_its_chief_among_its_members`; `the_admin_is_chief_only_of_administration`
  - unit `phase1_store_converts_first_member_to_chief_and_drops_vision_overrides` (a Phase 1 `visions.json`/`channels.json` fixture: `# hq` becomes `# administration`, each channel's first member its chief, per-Vision autonomy and budget overrides dropped (each channel uses Settings › Visions until the person sets its own), each Vision's `soul` text moved to its `soul.md` (Task 2) when it becomes a chief and dropped for workers, heartbeat files deleted, logs kept)
  - Review Focus 5: `an_interrupted_conversion_finishes_once_on_the_next_start` (crash after channels are written, before Visions: next start completes; the note in `# administration` exactly once)
  - config `requests_per_day_and_routine_limits_are_validated_and_only_lowered_by_a_project`
  - CLI e2e `list_channels_shows_chief_autonomy_and_budget`
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement. The conversion runs at store open, writes a `conversion: 2` marker last, and is idempotent; its note says in plain words what changed ("Each channel now has a chief of staff: …").
- [ ] **Step 4:** Run `cargo test -p themis-desktop visions` and the new e2e module, plus `cargo test -p themis-core configuration`. Expected: PASS.
- [ ] **Step 5:** Commit `feat: visions channels have a chief and workers`.

### Task 2: Souls as `soul.md`

**Files:**
- Create: `crates/desktop/src/state/visions/souls.rs`, `souls_tests.rs`
- Modify: `runs.rs` (the run head reads the soul file), the server method table, `crates/desktop/src/state/editors.rs` use for "Open soul.md"

**Interfaces:**
- Produces: `soul_path(vision_id) -> PathBuf` (app-data `visions/souls/<id>.md`); `read_soul(id) -> Result<Option<String>, SoulProblem>`; `write_soul(id, text, revision) -> Result<String /*revision*/, String>`; server `get_soul {visionId}` → `{text, revision, problem?}`, `save_soul {visionId, text, revision}`, `open_soul {visionId}` (opens in the person's chosen editor, like Settings › Permissions' file). Only chiefs and Atlas have souls: `save_soul` refuses a worker.
- [ ] **Step 1:** Tests: `a_soul_round_trips_and_checks_its_revision`; `a_soul_over_16_kib_is_refused`; `an_unreadable_soul_is_a_problem_not_silence`; `a_change_applies_from_the_next_wake_not_a_running_one` (CLI e2e: a run in progress keeps its head; the next wake's head has the new text); `workers_have_no_soul`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. The run head embeds the soul read at wake start; external edits are picked up the same way.
- [ ] **Step 4:** Run the visions tests and the new e2e cases. Expected: PASS.
- [ ] **Step 5:** Commit `feat: chiefs and Atlas keep their soul in soul.md`.

### Task 3: The chief's grant and the worker's grant

**Files:**
- Create: `crates/desktop/src/server/agent/vision/chief.rs`, `crates/desktop/src/state/visions/team_log.rs` (+ tests)
- Modify: `crates/desktop/src/server/agent/vision.rs` (dispatch by role), `crates/desktop/src/state/visions/drive.rs`

**Interfaces:**
- Produces:
  - Team log: bounded JSONL per worker (`visions/teams/<channel>/<worker>.jsonl`, rotated like channel logs): entries `Task {from_chief, text}`, `Redirect`, `Report {text}`, `Pushback {text, safer}`, `Paused`, `Resumed`, `Question {thread, question_id}`.
  - Chief methods (all `themis call`): `assign_task {workerId, task}`, `redirect_worker {workerId, text}`, `pause_worker {workerId}`, `resume_worker {workerId}`, `get_team_log {workerId, beforeSeq?}`, `list_team {}`, chat reads on its workers' chats, and `answer_question` on a question in a worker's chat within the channel's level. Plus Tasks 5–7's company calls. Refused: `create_thread`, `send_message`, `enqueue_message`, `steer_queued_message`, `stop_thread` on any chat, `approve_action`, modes, settings, secrets, `plugin_action`, anything on another channel's workers.
  - Worker: Phase 1's Vision grant on its own chats only, plus `report {text}` and `push_back {text, safer}` (append to its team log and wake its chief). A worker cannot `post_channel_message`, task another worker, or call company methods.
  - Grants decide on the request (method + ids + owner records), never on command text.
- [ ] **Step 1:** Tests (grant unit tests and a proptest like Phase 1's `arbitrary_vision_calls_pass_only_for_owned_chats`): `a_chief_reads_and_tasks_its_workers_but_drives_no_chat`; `a_chief_cannot_task_another_channels_worker`; `a_worker_reports_and_pushes_back_only_to_its_chief`; `a_worker_cannot_post_in_the_channel_or_task_others`; proptest `arbitrary_team_calls_stay_inside_the_team`. CLI e2e `a_task_reaches_the_worker_and_its_report_reaches_the_chief`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the grant tests, the proptest and the e2e. Expected: PASS.
- [ ] **Step 5:** Commit `feat: a chief manages its workers and drives no chat`.

### Task 4: Wakes, routing and the team cap

**Files:**
- Modify: `crates/desktop/src/state/visions/wakes.rs`, `control.rs` (`visions_post`), `runs.rs`, `crates/desktop/src/state/questions.rs`
- Test: `crates/desktop/tests/visions_e2e/routing.rs`

**Interfaces:**
- Produces: `WakeReason` adds `Task(worker log seq)`, `Report(worker_id)`, `Request(request_id)` (Task 5), `Routine(routine_id)` (Task 7). Routing: a person's channel post wakes only the chief (no @mention); work-chat events wake the worker that owns the chat; take over / hand back act on that worker and the chief gets a `Report`-like event. One shared wake count per channel (all agents, routines and requests included): at `visions.max_wakes` without a person post the server stops starting wakes there and posts one `team_paused` notice; the person's next post resets it. Questions: worker answers within the channel's level, else passes to the chief, chief answers within level, else the server posts it in the channel attributed to the chief (`pass_question` unchanged for the person's answer). Approvals from any team chat show in the channel as in Phase 1, under the chief. Budget is the channel's: the spend of all its agents and their chats counts, with Settings › Visions' total as the ceiling; at the limit the server stops every wake in the channel and posts the notice once, with no model call (Phase 1's budget code moves from per-Vision to per-channel).
- [ ] **Step 1:** Tests: `a_person_post_wakes_only_the_chief`; `a_work_chat_event_wakes_its_worker`; Review Focus 1 `a_team_bouncing_work_stops_at_the_channel_cap_with_one_notice`; Review Focus 2 `a_question_no_agent_may_answer_reaches_the_person_anyway`; `hand_back_wakes_the_owning_worker_and_tells_the_chief`; `a_workers_approval_shows_in_the_channel_under_the_chief`; `the_channel_budget_stops_every_wake_with_one_notice`; scale test `team_routing_work_grows_linearly_with_log_size`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement. Keep Phase 1's queued-not-dropped wake slots per Vision.
- [ ] **Step 4:** Run the routing e2e module, the visions unit tests and the scale test. Expected: PASS.
- [ ] **Step 5:** Commit `feat: visions teams route work through the chief`.

### Task 5: Requests between channels

**Files:**
- Create: `crates/desktop/src/state/visions/requests.rs` (+ tests)
- Modify: `ChannelEntry` (`RequestSent`, `RequestReceived` lines), the chief grant (Task 3)
- Test: `crates/desktop/tests/visions_e2e/requests.rs`

**Interfaces:**
- Produces: chief methods `request_channel {channelId, ask}` → `{requestId}` (same project only; the target's chief wakes with `Request`), `answer_request {requestId, accept: bool, note}`; both channels show one line each way ("Asked # api to …", "# release asked us to …", and the answer), opening the exchange. Daily cap `visions.requests_per_day` per sending channel; at the cap the call is refused with a plain reason and the channel shows it once. A request's work is paid by the receiving channel, including a decline's wake.
- [ ] **Step 1:** Tests: `a_request_wakes_the_other_chief_and_shows_in_both_channels`; `a_chief_declines_within_its_autonomy`; Review Focus 3 `requests_between_channels_stop_at_the_daily_cap`; `a_request_to_another_project_is_refused`; `the_receiving_channel_pays_for_a_request`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the requests e2e module and unit tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat: chiefs ask other channels for help`.

### Task 6: Atlas builds teams and hires

**Files:**
- Modify: `crates/desktop/src/state/visions/admin.rs`, `crates/desktop/src/server/agent/vision/admin.rs`, `crates/core/builtins/prompts/visions-admin.md`
- Test: `crates/desktop/tests/visions_e2e/admin.rs` (extend)

**Interfaces:**
- Produces: proposal schema `{summary, team?: {channel: {name, topic?}, chief: {name, look?, soul?, model?}, workers: [{name, look?, role, model?}], autonomy?, daily_budget_usd?}, hire?: {channelId, worker: {name, look?, role, model?}}, changes?}`; created field by field; the newest-proposal and person-replied checks stay; `autonomy` never above Settings › Visions and the budget never above Settings' total. Chief method `ask_for_worker {role, why}` posts a request into `# administration` and wakes Atlas (counted in its cap).
- [ ] **Step 1:** Tests: `a_team_proposal_creates_exactly_what_it_says`; `a_hire_needs_the_persons_yes`; `a_chief_asking_for_a_worker_wakes_atlas_and_shows_in_administration`; `a_proposal_cannot_raise_autonomy_or_budget` (extend Phase 1's).
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement; update `visions-admin.md` for teams in plain voice (one proposal per message, act only on a clear yes).
- [ ] **Step 4:** Run the admin e2e module and prompt tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat: atlas builds teams and adds workers on a yes`.

### Task 7: Routines and quiet hours

**Files:**
- Create: `crates/desktop/src/state/visions/routines.rs` (+ tests)
- Modify: `crates/desktop/src/state/scheduler.rs` (a Visions-only tick, separate from Automations), the chief grant
- Test: `crates/desktop/tests/visions_e2e/routines.rs`

**Interfaces:**
- Produces: `Routine { id, chief: String, channel_id, schedule: Schedule /* weekly days + "HH:MM" local */, instruction: String, enabled: bool, next_due_ms: u64 }` in app-data `visions/routines.json` (revisioned); chief methods `save_routine {routine}` / `delete_routine {routineId}` within its autonomy (a new `routines` decision row, default Decides from Most decisions up) and the limits; person methods `set_routine_enabled`, `delete_routine` (profile); each chief add/change posts one channel line. Firing: the chief wakes with `Routine` as if the person had written the instruction; never in quiet hours, never over budget; missed fires collapse to one at the next allowed moment.
- [ ] **Step 1:** Tests: `a_chief_adds_a_routine_within_its_autonomy_and_says_so`; `routine_limits_hold`; Review Focus 4 `missed_routines_fire_once_at_the_next_allowed_moment`; `quiet_hours_hold_routines_but_not_the_persons_messages`; `the_person_can_pause_and_remove_any_routine`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement with an injectable clock for tests.
- [ ] **Step 4:** Run the routines e2e module and unit tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat: chiefs run routines`.

### Task 8: Prompts for chiefs and workers

**Files:**
- Create: `crates/core/builtins/prompts/visions-chief.md`, `visions-worker.md`, `visions-wake-task.md`, `visions-wake-report.md`, `visions-wake-request.md`, `visions-wake-routine.md`
- Modify: `crates/core/src/prompts.rs` (+ inventory), `crates/desktop/src/state/visions/runs.rs` (head by role), `docs/prompts.md`

**Interfaces:**
- Consumes: the methods of Tasks 3–7.
- Produces: the chief's head (soul, team, plain voice, "never change anything yourself: give the task to a worker", check a worker's evidence before saying done, tell the person what matters in their own words, address the person as "you"); the worker's head (role, take tasks only from the chief, push back with a reason and a safer way, report results with evidence, never talk to the person). Both carry the live-check lessons from `docs/agent-weaknesses.md` 2026-11-14 as plain rules, without naming them.
- [ ] **Step 1:** Tests: prompt inventory and byte-identity updated deliberately; `a_chief_head_lists_its_team_and_soul`; `a_worker_head_names_its_chief_and_role`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Write the prompt files; assemble only in code.
- [ ] **Step 4:** Run `cargo test -p themis-core prompts` and the visions tests; check one constructed head of each role. Expected: PASS.
- [ ] **Step 5:** Commit `feat: chiefs and workers have their own instructions`.

### Task 9: The team screens (board `TeamChannel`, `AdminChannel`, `ChiefProfile`)

**Files:**
- Modify: `web/src/screens/visions/ChannelsBar.tsx`, `ChannelView.tsx`, `ChannelComposer.tsx`, `ChannelEntry.tsx`, `SidePanel.tsx`, `VisionProfile.tsx` (becomes the chief's profile), `ProposalCard.tsx`, `web/src/state/visions.ts`, `Visions.css`, `VisionsPanels.css`
- Create: `web/src/screens/visions/ChannelHeader.tsx`, `WorkerChips.tsx`, `TeamThread.tsx`, `Routines.tsx` (+ tests)

**Interfaces:**
- Consumes: Tasks 1–7's server fields and methods; Task 10's `CharacterAvatar` and `Expression`.
- Produces: exactly the boards. Sidebar: project switcher, Channels with `# administration` first, each row with its chief's small cloud; no Visions list. Header: chief picture (Task 10's still picture until Task 11), channel name in the display font with the channel gradient, "<chief>, Chief of staff" with state dot and label, worker chips (outline on the open one). Channel: person and chief only, request and routine lines, thinking row, "Message <chief>" with the mode shield and no @mention. Worker chip → `TeamThread` read-only panel ("<worker> with <chief>", log, work-chat links, Close). `# administration`: team and new-worker cards, no shield. Chief profile: Pause, Soul box + "Open soul.md", Model, Autonomy, Budget (today's spend of the limit), Team, Routines (switch, Remove).
- [ ] **Step 1:** Web tests: `the_sidebar_pins_administration_and_shows_chiefs`; `the_channel_shows_only_the_person_and_the_chief`; `a_worker_chip_opens_its_thread_read_only`; `the_chief_profile_edits_soul_and_routines`; `a_team_card_shows_what_was_created`; `no_hint_text_on_team_screens` (text scan like Phase 1's).
- [ ] **Step 2:** Run `npm --prefix web test`. Expected: FAIL.
- [ ] **Step 3:** Implement against the boards.
- [ ] **Step 4:** Run the web suite, lint and build. In the real app (scratch HOME, free model), compare each screen with its board in all four skies; fix differences or ask the owner on the canvas. Expected: PASS and matching.
- [ ] **Step 5:** Commit `feat: visions team screens`.

### Task 10: The cast and the expressions (board `ChiefMoods`)

**Files:**
- Create: `web/src/screens/visions/cast.ts` (each character's body circles, colour ramp hi/mid/lo, accessory, eye positions), `CharacterAvatar.tsx` (flat avatar with accessory), `Expression.ts` (state → face), still header pictures
- Modify: `CloudIcon.tsx` (replaced by `CharacterAvatar`), `VisionMark.tsx`, `NewVisionSheet.tsx` (pick a look)

**Interfaces:**
- Produces: `CharacterAvatar {look, size}` used by every small avatar; `expressionFor(state): "calm"|"working"|"done"|"needs"|"stuck"|"paused"` from server state (running, finished with evidence, waiting on the person, failed or blocked, paused/over budget); the still header picture per expression (used by Task 9 and as Task 11's fallback). Character data comes from the `ChiefMoods` board (bodies, colours, accessories, Tess's coral body and magnifier).
- [ ] **Step 1:** Web tests: `every_character_has_its_own_colour_and_accessory`; `each_state_maps_to_one_expression_with_a_label`; `reduced_motion_shows_a_still_face`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement from the board's geometry.
- [ ] **Step 4:** Run the web suite; compare with the board in all four skies in the real app. Expected: PASS and matching.
- [ ] **Step 5:** Commit `feat: the visions cast with accessories and expressions`.

### Task 11: The 3D chief

**Files:**
- Create: `web/src/screens/visions/three/ChiefScene.tsx`, `buildCharacter.ts` (sphere union from `cast.ts` circles, accessory primitives), `faces.ts` (animated face shapes)
- Modify: `web/package.json` (three.js pinned, bundled), `ChannelHeader.tsx`, `ChiefProfile` header

**Interfaces:**
- Consumes: Task 10's `cast.ts` and `expressionFor`.
- Produces: `ChiefScene {look, expression}` rendering the board's look (glow, ground shadow, soft light), animating expressions, rendering only while on screen; with reduced motion or no WebGL (`WebGLRenderingContext` missing or context lost) the still picture shows.
- [ ] **Step 1:** Web tests: `the_scene_stops_when_hidden`; `no_webgl_falls_back_to_the_still_picture`; `reduced_motion_keeps_the_still_picture`.
- [ ] **Step 2:** Run. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the web suite and build; in the real app on macOS compare with the boards in all four skies and record frame time (Workspace resource meter) with the scene on screen; CI builds on Windows and Linux. The PR says how Linux (WebKitGTK) was checked or that it falls back. Expected: PASS.
- [ ] **Step 5:** Commit `feat: the chief of staff in 3D`.

### Task 12: Settings, configuration, docs and live checks

**Files:**
- Modify: `web/src/screens/VisionsSettings.tsx` (quiet hours, requests per day, routine limits; autonomy and budget defaults now per channel), `ChannelDetails.tsx` (channel autonomy and budget), `docs/configuration.md`, `docs/user-guide.md`, `docs/live-verification.md` (rows `feature:visions-teams`, `feature:visions-requests`, `feature:visions-routines`, `feature:visions-souls`, `feature:visions-3d`), `AGENTS.md` (add Settings › Visions to the in-app block editors list), `crates/desktop/tests/live_coverage.rs`
- [ ] **Step 1:** Run `cargo test -p themis-desktop --test live_coverage`. Expected: FAIL (missing rows).
- [ ] **Step 2:** Add the rows (each with its live-check steps and its board), the settings fields and the docs.
- [ ] **Step 3:** Run the coverage test, the web suite and `cargo test -p themis-core configuration`. Expected: PASS.
- [ ] **Step 4:** Commit `docs: visions teams settings, guides and live checks`.

### Task 13: The team eval

**Files:**
- Modify: `evals/visions/` (`run.py`, `grade.py`, `scenarios.py`, `README.md`, `test_harness.py`)

**Interfaces:**
- Produces: a `team` arm (one chief, two workers) beside `vision` (one Phase 1-style Vision is now a chief with one worker) and `goal`; new measures `agent_messages` (team log entries per task), `requests_sent`, `false_relay_claims`, `third_person_lines`, `false_facts` (the three 2026-11-14 entries in `docs/agent-weaknesses.md`); spend caps as before. The redesign ships when the team needs no more person messages than a single chief with one worker, with no false "done", at a cost the owner accepts (owner sets the model and run count).
- [ ] **Step 1:** Self-tests for each new measure from the live-check text (`CLAIM_TABLE`-style tables).
- [ ] **Step 2:** Run `python3 -m unittest evals.visions.test_harness`. Expected: FAIL.
- [ ] **Step 3:** Implement; dry run `python3 -m evals.visions run --dry --scenarios trace-rounding --arms team --timeout 420`.
- [ ] **Step 4:** Self-tests and dry run. Expected: PASS, $0. Update the three weakness entries' "Measured by".
- [ ] **Step 5:** Commit `test(evals): visions team arm`.
