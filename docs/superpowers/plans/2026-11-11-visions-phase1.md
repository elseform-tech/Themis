# Visions Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One working loop. The person talks to a Vision in a channel, and the Vision starts,
prompts, steers and checks ordinary Themis chats it owns. It answers their questions within the
autonomy level, relays approvals, stays inside a server-enforced budget, and never restarts what
the person stopped.

**Architecture:**
- **The Vision's chat.** A Vision runs as an ordinary agent run in a hidden "Vision chat" (one per
  Vision per channel), with its own system head (`with_system_head`, as subagents do) and
  read-only tools.
- **Driving chats.** It drives chats through the `themis` CLI with a **Vision grant**: the existing
  run credential plus a `VisionScope`. The app server checks every call against an ownership
  record in a revision-checked app-data store (`visions/`).
- **Waking.** Owned-chat events (finished, failed, stopped, question asked, taken over) queue
  bounded wakes, modelled on `hook_wakes.rs`.
- **Configuration.** Settings live in a new `visions` block in `runtime.jsonc`, written field by
  field.
- **Front end.** A Visions view: the rail cloud, a channels bar, a channel view, a profile panel
  and Settings › Visions.

**Tech Stack:** Rust (crates/core, crates/desktop, tokio), React + TypeScript (Vitest), Python
evals (`evals/kit`).

**Spec:** docs/superpowers/specs/2026-11-11-visions-design.md. Canvas boards Visions,
VisionProfile, VisionChannel, VisionsNew, VisionsStart, VisionsQuestions, VisionsThread,
VisionsTrouble, VisionsEntry and VisionsSettings at
https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq.

**Prerequisite:** Reskin plan (`docs/superpowers/plans/2026-11-11-reskin.md`) Tasks 1-3 are merged.
That means Bubbles is retired, the rail lives in `web/src/screens/Rail.tsx`, and it has the `extra`
slot after Chats.

## Global Constraints

- Agent permission rules:
  - A Vision grant **never** allows `approve_action`, `set_thread_approval_mode`, settings or
    secret writes, `save_runtime_configuration`, switching on hooks or command MCP servers, or
    any call on a chat the Vision does not own.
  - Ownership is checked on the request's `threadId`, never on command text.
- Settled decisions:
  - Default autonomy is `small`.
  - Each channel has one approval mode, set by the person and copied onto every chat the Vision
    starts.
  - A default overall daily cap is always on.
- Autonomy rows, as ids, in this order:
  - `wording`, `tests`, `undoable`, `retry`, `scope`, `delete`, `dependency`, `external`,
    `spend`.
  - `external` and `spend` are always the person's, in every level including `custom`.
- Preset levels and the first level at which the Vision decides each row:
  - `ask` < `small` < `most` < `nearly_all`.
  - `wording` and `tests` from `small`.
  - `undoable` and `retry` from `most`.
  - `scope`, `delete` and `dependency` from `nearly_all`.
  - `external` and `spend` never.
- Naming: new code uses `visions` (plural). Do not touch `crates/core/src/vision.rs` or
  `crates/desktop/tests/vision_cli_e2e.rs`, which are about images.
- Logs carry metadata only (ids, counts, reasons), never message text.
- Prompts live in `crates/core/builtins/prompts/*.md` with no trailing newline, registered in
  `prompts.rs` and in the inventory `crates/core/src/prompts/tests.rs::ALL`.
- Ratchets: files ≤ 1000 code lines, functions ≤ 100 lines, cognitive ≤ 25, cyclomatic ≤ 20.
  New code goes in new focused files.
- Every backend task adds a CLI end-to-end case in `crates/desktop/tests/visions_cli_e2e.rs`,
  modelled on `chat_reads_cli_e2e.rs`: `ONE_SERVER` mutex, `fixture()`, `cli()`, scripted mock
  LLM.
- One PR per task on its own `codex/visions-<n>` branch. Stage paths explicitly (evals append to
  `history.jsonl`).

## Review Focus

1. **The person stops a Vision's chat, and the Vision's next wake tries `send_message` to it**:
   the server refuses with "stopped by the person" until the person hands it back or writes in
   the channel. Owner: Task 4, test `a_person_stop_blocks_the_visions_sends_until_handed_back`.
2. **Two owned chats finish in the same second**: one Vision run starts, and the second event is
   queued for it, not lost and not run as a second parallel run. Owner: Task 5, test
   `events_while_a_vision_runs_are_queued_not_dropped`.
3. **The budget is reached in the middle of a chat's run**: that run ends, no new Vision run or
   send starts, and the channel shows the budget notice exactly once. Owner: Task 7, test
   `reaching_the_cap_stops_runs_and_posts_once`.
4. **A Vision answers a question with a row its level does not allow** (or `external`/`spend`
   under `custom`): the answer is refused and the question stays open for the person. Owner:
   Task 6, test `answers_outside_the_level_are_refused_and_stay_open`.
5. **The person deletes a channel while its chats run**: the chats are stopped, then become
   ordinary chats in the plain sidebar, with nothing deleted. Owner: Task 3, test
   `deleting_a_channel_stops_and_releases_its_chats`.

---

### Task 1: Autonomy rules and the `visions` configuration block

**Files:**
- Create: `crates/core/src/visions.rs` (pure rules), `crates/core/src/configuration/visions.rs` (the block)
- Modify: `crates/core/src/lib.rs`, `crates/core/src/configuration.rs:37` (add `pub visions: VisionsConfig`), `crates/core/src/configuration/resolve.rs:35-64,206-290` (restriction + validation), `crates/core/src/configuration/schema.rs:11-17`, `crates/core/src/configuration_reference.jsonc`, `docs/configuration.md` (Supported limits rows), `AGENTS.md:20` (add Settings → Visions and the `visions` block to the in-place list)
- Test: `crates/core/src/visions.rs` (unit), `crates/core/src/configuration/settings_tests.rs`

**Interfaces:**
- Produces:
  - `pub enum Decision { Wording, Tests, Undoable, Retry, Scope, Delete, Dependency, External, Spend }`, with `Decision::ALL: [Decision; 9]`, `fn id(self) -> &'static str` and `fn from_id(&str) -> Option<Decision>`.
  - `pub enum Autonomy { Ask, Small, Most, NearlyAll, Custom }`, serialized as `ask|small|most|nearly_all|custom`.
  - `pub fn vision_decides(level: Autonomy, custom: &BTreeMap<String, bool>, decision: Decision) -> bool`. It is false for `External` and `Spend` always.
  - `pub struct VisionsConfig` with:
    - `enabled: bool` (true)
    - `autonomy: Autonomy` (Small)
    - `custom: BTreeMap<String, bool>` (empty; unknown keys rejected)
    - `daily_budget_usd: f64` (1.0, range 0.1–100)
    - `max_wakes: u32` (6, range 1–20)
  - `pub const MAX_VISIONS_BUDGET_USD: f64 = 100.0` and `pub const MAX_VISION_WAKES: u32 = 20`.
  - `check_visions_restriction(layer, origin)`: a project or run layer may lower autonomy, lower the budget and lower the wakes, and may not raise them or set `custom` rows to true.
- Budget default: the spec defers the starting value to measured eval costs. Ship 1.0, and Task 12 records the measured cost per scenario and adjusts this constant in its own commit if the measurement says so.

- [ ] **Step 1:** Write unit tests:
  - `presets_follow_the_ladder`: a table over all 4 presets × 9 decisions, matching Global Constraints.
  - `external_and_spend_are_never_the_visions`: under `Custom` with both set to true, the result is false.
  - `unknown_custom_rows_are_rejected`.
  - `a_project_cannot_raise_autonomy_or_budget`: a project sets `most` over a user `small` and resolution fails. Lowering to `ask` passes.
  - Extend `the_reference_is_valid_and_lists_every_setting` (it reads the new block).
- [ ] **Step 2:** Run `cargo test -p themis-core visions` and `cargo test -p themis-core settings_tests`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement following the `MemoryConfig` pattern (`sections.rs:63`, `validate_memory`, `check_memory_restriction`). Document each field in `configuration_reference.jsonc` with `//` comments.
- [ ] **Step 4:** Run `cargo test -p themis-core` and `cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Update `docs/configuration.md` (one row per field, "your own runtime.jsonc; a project may only lower these") and the AGENTS.md list. Commit with `feat: visions configuration and autonomy rules`.

### Task 2: The Visions store

**Files:**
- Create: `crates/desktop/src/state/visions/mod.rs`, `store.rs`, `channel_log.rs`
- Modify: `crates/desktop/src/state.rs:277-346,595-638` (add `visions_dir`, `visions: Mutex<VisionsCache>`)
- Test: `crates/desktop/src/state/visions/store.rs` (unit), `crates/desktop/src/state/visions/channel_log.rs` (unit + scale)

**Interfaces:**
- Produces:
  - Files under app data `visions/`: `visions.json`, `channels.json`, `owners.json` and `channels/<channel id>.jsonl`. They are written with the plugin store's tmp-file, fsync, 0600 and rename pattern, refuse symlinks, and are capped at 32 MiB when read.
  - `pub struct Vision { id, name, role, shape: String, color: String, model: Option<String>, autonomy: Option<Autonomy>, custom: Option<BTreeMap<String,bool>>, daily_budget_usd: Option<f64>, soul: String, paused: bool, revision: String }`. `soul` is at most 4 KiB.
  - `pub struct Channel { id, name, topic, project_root: PathBuf, approval_mode: ApprovalMode, members: Vec<String>, revision: String }`. `name` matches `[a-z0-9-]{1,40}` and is unique per project. `members` are Vision ids.
  - `pub struct Owner { thread_id, vision_id, channel_id, kind: OwnedKind, hold: Hold }`.
  - `pub enum OwnedKind { VisionChat, Work }` and `pub enum Hold { None, StoppedByPerson, TakenOver }`.
  - `pub enum ChannelEntry { Person { text }, Vision { vision_id, text }, ChatStarted { vision_id, thread_id, title }, Answered { vision_id, thread_id, question_id, decision, question, answer }, PassedOn { vision_id, thread_id, question_id, decision }, Notice { kind: NoticeKind, vision_id: Option<String>, thread_id: Option<String> } }`. Each line is `{ seq, at_ms, entry }`.
  - `pub enum NoticeKind { Stopped, Paused, Resumed, BudgetReached, ChatFailed, HandedBack }`.
  - `AppState` methods:
    - `visions_list() -> Vec<Vision>`
    - `vision_save(spec, expected_revision: Option<&str>) -> Result<Vision, String>`. A stale revision fails with "Vision changed since it was opened; reload before saving".
    - `channel_list(project_root: &Path) -> Vec<Channel>`
    - `channel_save(spec, expected: Option<&str>) -> Result<Channel, String>`
    - `owner_of(thread_id: &str) -> Option<Owner>`, an O(1) map lookup
    - `owner_set(owner: Owner)`
    - `owner_release(thread_id: &str)`
    - `channel_append(channel_id: &str, entry: ChannelEntry) -> Result<u64, String>`
    - `channel_page(channel_id: &str, before_seq: Option<u64>, max_bytes: usize) -> Result<ChannelPage, String>`

- [ ] **Step 1:** Write the tests:
  - `saving_with_a_stale_revision_fails`
  - `channel_names_are_unique_per_project_and_validated`
  - `owners_survive_a_restart`: new `AppState` on the same dir.
  - `a_damaged_store_file_is_reported_not_overwritten`: garbage in `channels.json` makes the load error, and the file bytes are unchanged.
  - `channel_pages_walk_back_by_seq_within_max_bytes`
  - Scale test `channel_page_reads_stay_bounded_by_page_not_log_size`: `crate::scale::measure(&[1_000, 50_000], …)` counting bytes read per page, with `growth_rate` below 0.2.
- [ ] **Step 2:** Run `cargo test -p themis-desktop state::visions`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement. `channel_page` reads from the end of the JSONL in chunks, never the whole file. Keep the last seq per channel in `VisionsCache`.
- [ ] **Step 4:** Run the tests and clippy. Expected: PASS. Add the scale row to `docs/scale-tests.md`.
- [ ] **Step 5:** Commit with `feat: visions store`.

### Task 3: Person-facing server methods and the `visions-changed` event

**Files:**
- Create: `crates/desktop/src/server/visions_commands.rs`
- Modify: `crates/desktop/src/server/commands.rs:84` (route `visions_*` and `channel_*` methods to the new file), `crates/desktop/src/sink.rs:16` (add `fn emit_visions_changed(&self, change: &VisionsChange)` to `EventSink` and every implementation), `crates/desktop/src/server.rs:64` (`"visions-changed"`), `crates/desktop/src/state/threads.rs:179` (`list_threads` skips owned chats), `crates/desktop/src/state/threads.rs:659,681` (stop and discard update owners), `crates/desktop/src/types/events.rs:306` (`SettingsScope::written_by` += `update_visions_settings`), `crates/desktop/src/state/configuration.rs:333-440` (visions view and patch, like subagents)
- Test: `crates/desktop/tests/visions_cli_e2e.rs` (new)

**Interfaces:**
- Consumes: Task 2's store and Task 1's `VisionsConfig`.
- Produces: server methods, all person-only, none in `agent.rs::ALLOWED`:
  - `list_visions`, `save_vision {vision, revision}`, `pause_vision {visionId, paused}`, `delete_vision {visionId}`
  - `list_channels {projectRoot}`, `save_channel {channel, revision}`, `delete_channel {channelId}`
  - `get_channel_page {channelId, beforeSeq?, maxBytes?}`, `post_channel_message {channelId, text}`, `list_channel_threads {channelId}` (`ThreadInfo[]` of owned `Work` chats)
  - `hand_back_thread {threadId}`
  - `get_visions_settings` and `update_visions_settings {changes, revision}`, with EDITABLE = `autonomy`, `custom`, `daily_budget_usd`, `max_wakes`, `enabled`
  - `pub struct VisionsChange { channel_id: Option<String>, vision_id: Option<String> }` → event `visions-changed`.
- Rules:
  - `post_channel_message` appends `Person`. It clears any `Hold` on that channel's chats except `TakenOver`, and it wakes Visions via Task 5's `visions_wake(...)`, which is a no-op until Task 5 lands.
  - `delete_channel` stops each owned chat, releases its owner (the chats return to the plain sidebar) and keeps the log file. `delete_vision` does the same for its chats.
  - A person's `stop_thread` on an owned `Work` chat sets `Hold::StoppedByPerson` and appends `Notice Stopped`.
  - A person's `send_message` on an owned `Work` chat sets `Hold::TakenOver`. Use the existing person-or-server distinction (`threads.rs:276-292`).
  - `hand_back_thread` clears it and appends `Notice HandedBack`.

- [ ] **Step 1:** Write the CLI end-to-end tests, each through `themis call` with the person token:
  - `a_channel_and_a_vision_round_trip_with_revisions`
  - `owned_chats_leave_the_plain_list_and_show_in_the_channel`
  - `deleting_a_channel_stops_and_releases_its_chats`
  - `a_person_stop_marks_the_chat_held` and `a_person_message_in_an_owned_chat_is_a_take_over`
  - `visions_settings_write_one_field_and_refuse_a_stale_revision`
  - `an_agent_credential_is_refused_every_visions_method`
- [ ] **Step 2:** Run `cargo test -p themis-desktop --test visions_cli_e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement. Emit `visions-changed` after each successful write.
- [ ] **Step 4:** Run the tests, `cargo test -p themis-desktop` and clippy. Expected: PASS.
- [ ] **Step 5:** Commit with `feat: visions server methods`.

### Task 4: The Vision grant

**Files:**
- Modify: `crates/core/src/agent_cli.rs:100-160` (`RunScope.vision: Option<VisionScope>`), `crates/desktop/src/server/agent.rs:118-126,304-328,418` (Grant + refusal + post-dispatch hook), `crates/core/src/tools/themis_cli.rs:54-87,166` (classify driving methods as `Execute`)
- Test: `crates/desktop/src/server/agent.rs` tests module, `crates/desktop/tests/visions_cli_e2e.rs`

**Interfaces:**
- Consumes: `AppState::owner_of` (Task 2).
- Produces:
  - `pub struct VisionScope { pub vision_id: String, pub channel_id: String, pub project_root: PathBuf }`.
  - `Grant.vision: Option<VisionScope>`.
  - `const VISION_DRIVE: &[&str] = &["create_thread","send_message","enqueue_message","steer_queued_message","stop_thread","answer_question","list_questions","list_channel_threads"]`.
  - `pub(crate) fn refusal(grant: &Grant, method: &str, args: &Value, owned: &dyn Fn(&str) -> Option<Owner>) -> Option<Refusal>`. The extra argument is the ownership lookup; existing callers pass `&|_| None`.
- Rules for a `Some(vision)` grant:
  - `create_thread`: `projectRoot` must equal `vision.project_root`. After dispatch, `screen` records `Owner { kind: Work, vision_id, channel_id }`, sets the chat's approval mode to the channel's (server side, not through the refused method) and appends `ChatStarted`.
  - Every other `VISION_DRIVE` method plus `OWN_CHAT` reads: `threadId` must be owned by `vision.vision_id` with kind `Work`.
  - `send_message`, `enqueue_message` and `steer_queued_message` are refused while `hold != None`, with the new `Reason::HeldByPerson` (label `held_by_person`; message "The person stopped or took over this chat; wait for them to hand it back."). They are also refused while the Vision is paused, with `Reason::Paused`.
  - The Vision's sends go through `send_from_server` (`queue.rs:527`), so they never count as the person's message.
  - The existing refused list stays refused, and the test at L532 gains a Vision-grant variant.

- [ ] **Step 1:** Write the tests:
  - Unit: `a_vision_grant_reaches_only_its_own_work_chats`, `a_vision_grant_never_approves_or_changes_modes` (loop over the refused list of L543-559), `create_thread_is_refused_outside_the_channels_project`.
  - Proptest: `arbitrary_vision_calls_pass_only_for_owned_chats`.
  - CLI end-to-end: `a_person_stop_blocks_the_visions_sends_until_handed_back`, `a_visions_send_is_not_the_persons_message` (open questions stay open; hook wakes are not reset).
  - Keep `the_shell_reads_exactly_the_allowed_reading_methods` green: the driving methods are not reads.
- [ ] **Step 2:** Run `cargo test -p themis-desktop agent` and `--test visions_cli_e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement. `screen` does the ownership lookup (it has `state`), and `refusal` stays a pure function for the proptest.
- [ ] **Step 4:** Run all `themis-desktop` tests, the scale test `the_guard_check_stays_flat_with_many_issued_credentials` (extend it with 8000 owners) and clippy. Expected: PASS.
- [ ] **Step 5:** Commit with `feat: vision grant drives only owned chats`.

### Task 5: Vision runs and bounded wakes

**Files:**
- Create: `crates/desktop/src/state/visions/runs.rs`, `crates/desktop/src/state/visions/wakes.rs`, `crates/core/builtins/prompts/visions-system.md`, `visions-wake-turn.md`, `visions-read-only-note.md`
- Modify: `crates/desktop/src/state.rs:151` (`RunSnapshot.vision: Option<VisionRunSpec>`), `crates/desktop/src/state/run.rs:31,235-271,942-945` (system head, tools and scope for Vision runs), `crates/desktop/src/state/queue.rs:496` (`after_run` calls `visions_after_run`), `crates/desktop/src/state/questions.rs:301` (`ask_questions` calls `visions_question_asked`), `crates/core/src/prompts.rs`, `crates/core/src/prompts/tests.rs::ALL`, `docs/prompts.md`
- Test: `crates/desktop/src/state/visions/wakes.rs` (unit), `crates/desktop/tests/visions_cli_e2e.rs`

**Interfaces:**
- Consumes: Tasks 2-4.
- Produces:
  - `pub(crate) struct VisionRunSpec { vision_id, channel_id }`.
  - `AppState::visions_wake(sink, channel_id: &str, vision_id: &str, reason: WakeReason)`.
  - `pub enum WakeReason { Mentioned, ChatFinished(thread), ChatFailed(thread), ChatStopped(thread), QuestionAsked(thread, question_id), HandedBack(thread) }`.
  - `AppState::visions_after_run(sink, thread_id, terminal: &ThreadEvent)`.
  - `AppState::visions_question_asked(sink, thread_id, question_ids)`.
- A Vision run:
  - It uses the Vision chat (`OwnedKind::VisionChat`), created on first wake with title `"{vision name} · #{channel}"`.
  - `with_system_head(fill(VISIONS_SYSTEM, [name, role, soul, channel, project, autonomy_table]))`, where `autonomy_table` lists the 9 rows with "you decide" or "ask the person" from `vision_decides`.
  - Tools: `READ_ONLY_TOOLS` + `shell`, with a sandbox confined to no write paths (as subagents with empty `write_paths`, `running.rs:243`).
  - `RunScope.vision = Some(..)`.
  - Its final assistant text is appended to the channel as `ChannelEntry::Vision`.
- Wake rules:
  - One Vision run at a time per Vision chat. Reasons arriving meanwhile go to a per-Vision queue and are passed together in the next wake's text (`VISIONS_WAKE_TURN` with `{{reasons}}`).
  - At most `visions.max_wakes` runs in a row without a channel message from the person; `post_channel_message` resets the count.
  - None while the Vision is paused or over budget (Task 7). None for a chat whose `hold` is `StoppedByPerson`, except the single `ChatStopped` reason, which tells the Vision not to restart it.
  - Mentions: `@Name` in a person message wakes that Vision. With no mention and exactly one member, that member wakes. Otherwise nobody wakes, and the channel shows "Mention a Vision" (UI, Task 9).
  - Logs: `visions/wake {vision_id, channel_id, reason, outcome}`.

- [ ] **Step 1:** Write the tests:
  - Unit: `wakes_stop_at_max_until_the_person_writes`, `events_while_a_vision_runs_are_queued_not_dropped`, `no_wake_while_paused`, `a_mention_wakes_only_that_vision`.
  - CLI end-to-end with a scripted LLM: the Vision run calls `themis call create_thread` then `send_message`, the work chat finishes, and the Vision wakes with reason ChatFinished and posts one channel entry. Name it `a_vision_starts_a_chat_and_reports_when_it_finishes`.
  - Add the three prompt files to the inventory.
- [ ] **Step 2:** Run `cargo test -p themis-desktop visions` and `--test visions_cli_e2e`. Expected: FAIL.
- [ ] **Step 3:** Implement. Write the prompt texts:
  - Report only what you read yourself: `files_changed`, test output, the goal verdict. Never repeat a chat's own summary as proof.
  - Never restart a chat the person stopped.
  - Decide questions only per the table.
  - Keep channel messages short.
- [ ] **Step 4:** Run all Rust tests and clippy, and print the constructed system head in a test (`vision_system_head_contains_the_autonomy_table`) to verify the request path. Expected: PASS.
- [ ] **Step 5:** Commit with `feat: vision runs and wakes`.

### Task 6: Answering questions within the autonomy level

**Files:**
- Modify: `crates/desktop/src/server/agent.rs` (`answer_question` under a Vision grant), `crates/desktop/src/server/commands.rs:688`, `crates/desktop/src/state/questions.rs:370-399`
- Test: `crates/desktop/tests/visions_cli_e2e.rs`, `crates/desktop/src/server/agent.rs` tests

**Interfaces:**
- Consumes: `vision_decides` (Task 1) and the effective autonomy, which is the Vision override else the config.
- Produces:
  - Under a Vision grant, `answer_question` requires `decision: <row id>` and `reason: string` (≤ 200 chars).
  - Refused when `!vision_decides(level, custom, decision)`, with `Reason::AboveAutonomy` (label `above_autonomy`, message "Your autonomy level leaves this to the person; pass it on in the channel."). The question stays open.
  - Accepted answers append `ChannelEntry::Answered { decision, question, answer }`.
  - New Vision-only method `pass_question {threadId, questionId, decision}` appends `PassedOn` and leaves the question open for the person's answer in the channel. The UI answers through the existing `answer_question` with the person token.

- [ ] **Step 1:** Write the tests:
  - `answers_outside_the_level_are_refused_and_stay_open`: at `small`, a `scope` answer is refused, and under `custom` with `external: true` it is still refused.
  - `an_allowed_answer_is_recorded_with_its_decision`
  - `a_passed_question_is_answered_by_the_person_from_the_channel`
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run all Rust tests and clippy. Expected: PASS.
- [ ] **Step 5:** Commit with `feat: visions answer questions within autonomy`.

### Task 7: Daily budgets enforced by the server

**Files:**
- Create: `crates/desktop/src/state/visions/budget.rs`
- Modify: `crates/desktop/src/state/visions/wakes.rs`, `crates/desktop/src/server/agent.rs` (refuse driving sends when over), `crates/desktop/src/state/queue.rs:496`
- Test: `crates/desktop/src/state/visions/budget.rs`, `crates/desktop/tests/visions_cli_e2e.rs`

**Interfaces:**
- Produces:
  - `AppState::visions_spent_today(vision_id: Option<&str>) -> f64`. It sums `request_cost` over `usage.rows_since(local midnight)` whose `thread_id` is owned by that Vision (or by any Vision), following `memory_spent_today` (`memory_auto.rs:214`).
  - `AppState::visions_over_budget(vision_id) -> bool`, true when the Vision's own cap (profile override) or the overall `visions.daily_budget_usd` is reached.
- When over budget:
  - No wakes. The grant refuses `create_thread`, `send_message` and `enqueue_message` with `Reason::OverBudget`.
  - At each owned run's end (in `visions_after_run`), if over budget, every running owned chat of that Vision is stopped. That stop does not set `Hold`.
  - Exactly one `Notice BudgetReached` is posted per Vision per day.
  - Raising the budget in Settings or the profile resumes on the next wake.
- Unpriced requests count as 0 and are reported in the notice ("N requests had no price").

- [ ] **Step 1:** Write the tests: `spend_counts_only_owned_chats_since_midnight`, `reaching_the_cap_stops_runs_and_posts_once`, and `raising_the_cap_lets_the_next_wake_run`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run all Rust tests and clippy. Expected: PASS.
- [ ] **Step 5:** Commit with `feat: visions daily budget`.

### Task 8: Web bridge, state and the cloud's mood

**Files:**
- Create: `web/src/lib/visions.ts` (bridge + types), `web/src/state/visions.ts` (reducer slice + mood), `web/src/state/visions.test.ts`
- Modify: `web/src/state/reducer/model.ts:41,86-157,162-224` (`MainView` += `"visions"`; `visions` slice; actions), `web/src/state/store.tsx:205-354` (listen `visions-changed`), `web/src/lib/tauri.ts:439-500`, `web/src/lib/types.ts` (event name only; types go in `lib/visions.ts` because `types.ts` is at 962 lines)
- Test: `web/src/state/visions.test.ts`

**Interfaces:**
- Consumes: the Task 3 methods.
- Produces:
  - `VisionInfo`, `ChannelInfo`, `OwnerInfo` and `ChannelLine` (`{ seq, at_ms, entry }`) in `lib/visions.ts`. They mirror Task 2's Rust structs field for field in their serde form (snake_case), plus one bridge function per Task 3 method.
  - `type Mood = "needs" | "sorry" | "work" | "think" | "calm" | "sleep"`.
  - `visionMood(v: VisionInfo, chats: ThreadInfo[], waiting: number, runningSelf: boolean): Mood`. The priority is needs > sorry > work > think > calm. It is `sleep` when paused or over budget.
  - `railMood(moods: Mood[]): Mood`: the most urgent one, or `sleep` only if all are asleep.
  - `MOOD_LABEL: Record<Mood,string>`: "Needs you", "Sorry", "Working", "Thinking", "" (calm) and "Asleep".
  - `needsCount(state): number`: pending approvals on owned chats plus passed-on questions.
  - Actions `visions/loaded` and `visions/changed`.

- [ ] **Step 1:** Write the tests:
  - `mood_priority`, with a table of 8 cases.
  - `rail_is_asleep_only_when_every_vision_is`
  - `needs_count_adds_approvals_and_passed_questions`
  - `visions_changed_reloads_only_that_channel`
- [ ] **Step 2:** Run `npm --prefix web test -- visions`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the web tests and lint. Expected: PASS.
- [ ] **Step 5:** Commit with `feat: visions state and moods in the app`.

### Task 9: Visions view (rail cloud, channels bar, channel)

**Files:**
- Create: `web/src/screens/visions/VisionsButton.tsx`, `CloudIcon.tsx`, `ChannelsBar.tsx`, `ChannelView.tsx`, `ChannelEntry.tsx`, `ChatLink.tsx`, `NewChannelSheet.tsx`, `NewVisionSheet.tsx`, `Visions.css`, `VisionsView.test.tsx`, `VisionsButton.test.tsx`
- Modify: `web/src/screens/Rail.tsx` (`extra={<VisionsButton/>}` from `App.tsx`), `web/src/App.tsx:90,98-102`, `web/src/screens/Sidebar.tsx:131` (render `ChannelsBar` instead of the dock when `mainView === "visions"`)
- Test: as listed

**Interfaces:**
- Consumes: Task 8's selectors and the existing approval box component (from `ThreadView`).
- Produces:
  - `CloudIcon({ shape, color, mood, size, live })`, using the canvas paths (Cast board, `VisionsEntry.dc.html`). It animates only when `live`, and reduced motion turns animation off.
  - `VisionsButton`: the rail cloud. Its `aria-label` comes from `railMood`, for example "Visions, 2 need you". The badge is shown when `needsCount > 0`. The hop plays once per new needs-item.
  - `ChannelsBar`: project switcher, `# hq` (phase 1: an empty-state page with New channel / New Vision, no agent), channels, and Visions with live clouds and mood tags.
  - `ChannelView`: paged entries (`get_channel_page`), chat links with live status and Stop, Answered / PassedOn cards (the person answers inline), pending approvals of owned chats rendered with the existing approval box, and a composer with `@` mentions. "Mention a Vision" shows when the channel has more than one member and no mention.

- [ ] **Step 1:** Write the tests:
  - `the_cloud_says_what_needs_you`
  - `chats_returns_to_plain_chat_from_visions`
  - `channels_bar_replaces_the_chat_list`
  - `stop_on_a_chat_link_calls_stop_thread`
  - `a_passed_question_is_answered_in_the_channel`
  - `an_approval_from_an_owned_chat_shows_allow_and_deny`
  - `message_avatars_do_not_animate`
- [ ] **Step 2:** Run `npm --prefix web test -- visions`. Expected: FAIL.
- [ ] **Step 3:** Implement against the canvas boards. Keep each component ≤ 100 lines per function, and keep `Sidebar` from growing by putting the switch in a wrapper.
- [ ] **Step 4:** Run `npm --prefix web test && npm --prefix web run lint && npm --prefix web run build`. Expected: PASS.
- [ ] **Step 5:** Verify in the real app (`npx tauri dev`, isolated HOME): create a channel and a Vision, mention it, watch it start a chat, then Stop, take over and hand back. Take screenshots against the canvas boards.
- [ ] **Step 6:** Commit with `feat: visions view`.

### Task 10: Vision profile, channel details and Settings › Visions

**Files:**
- Create: `web/src/screens/visions/VisionProfile.tsx`, `ChannelDetails.tsx`, `web/src/screens/VisionsSettings.tsx`, `VisionsSettings.test.tsx`, `VisionProfile.test.tsx`
- Modify: `web/src/screens/SettingsNav.tsx:5` (add "Visions" after "Subagents"), `web/src/screens/SettingsGroup.tsx:22-37` (cloud icon), `web/src/screens/Settings.tsx:114,211-226`, `web/src/screens/Settings.test.tsx:142-165`
- Test: as listed

**Interfaces:**
- Consumes: `get_visions_settings` and `update_visions_settings` (Task 3), and `save_vision`, `pause_vision`, `save_channel`.
- Produces:
  - `VisionsSettingsSection({ onSaved })`, following `SubagentSettings.tsx`. It has the five-stop slider and a per-row list: read-only for presets and editable for Custom, with `external` and `spend` locked "Always you". It also has the daily budget, a "Pause all" switch (sets every Vision's `paused`) and the shared notes from the canvas.
  - Profile: Pause, Model, an Autonomy override ("Same as Settings (…)"), Daily budget, Soul (edits `soul`), Channels and Delete.
  - Channel details: Topic, Approval mode (person only), Members (Visions only) and Delete.

- [ ] **Step 1:** Write the tests:
  - `presets_are_read_only_and_custom_is_editable`
  - `locked_rows_stay_locked_in_custom`
  - `each_field_saves_alone_with_the_page_revision` (`update_visions_settings` is called with `({ autonomy: "most" }, "r1")`)
  - `profile_override_shows_the_inherited_level`
  - The `Settings.test.tsx` nav order includes "Visions".
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the web suite, lint and build. Expected: PASS. Check it in the real app against the VisionsSettings and VisionProfile boards.
- [ ] **Step 5:** Commit with `feat: visions settings and profiles`.

### Task 11: Live coverage, guides and diagnostics

**Files:**
- Modify: `docs/live-verification.md` (rows `view:visions`, `settings:visions`, `config:visions`, `feature:visions-take-over`, `feature:visions-budget`), `crates/desktop/tests/live_coverage.rs` (read `Rail.tsx` views if Task 3 of the reskin moved them), `docs/user-guide.md` (a Visions section), `docs/configuration.md` (shared-server methods and the `visions-changed` event near L1366)
- Test: `cargo test -p themis-desktop --test live_coverage`

- [ ] **Step 1:** Run `cargo test -p themis-desktop --test live_coverage`. Expected: FAIL, listing the missing rows.
- [ ] **Step 2:** Add the rows, each with its live-check steps (as Task 9 Step 5).
- [ ] **Step 3:** Run it again. Expected: PASS.
- [ ] **Step 4:** Commit with `docs: visions live checks and guides`.

### Task 12: `evals/visions` (benchmark audit)

**Files:**
- Create: `evals/visions/__main__.py`, `run.py`, `grade.py`, `scenarios.py`, `questions.jsonl` (the labelled impact set, 40 items, 4-5 per row), `test_harness.py`, `README.md`, `history.jsonl`
- Modify: `docs/agent-weaknesses.md` (a "Vision relays a false done" entry with this measure)
- Test: `python3 -m unittest evals.visions.test_harness` (offline, in CI)

**Interfaces:**
- Consumes: `evals/kit/app.py` (`app_server`, `call`, `Watch`), `evals/kit/spend.py` caps, `evals/verify/grade.py::claims_success`, and the `evals/questions` simulated-person pattern.
- Produces measures per run:
  - `person_messages` (fewer is better)
  - `false_done` (a channel claim of success while the hidden tests fail)
  - `above_level_attempts` (refused `answer_question` calls)
  - `restarts_after_stop`
  - `cost_usd`
- Scenarios: three Ledgerly tasks from `evals/verify/generate.py`, each run (a) through a Vision and (b) by the person in goal mode, with the same model.
- The impact set measures how often the Vision's chosen row matches the label.

- [ ] **Step 1:** Write `test_harness.py`:
  - `grade_counts_false_done_from_a_fixture_trace`
  - `restart_after_stop_is_detected`
  - `the_impact_set_has_every_row`
- [ ] **Step 2:** Run `python3 -m unittest evals.visions.test_harness`. Expected: FAIL.
- [ ] **Step 3:** Implement. Runs refuse to start above the `spend.py` caps.
- [ ] **Step 4:** Run the self-tests (PASS) and `ruff check evals/visions`.
- [ ] **Step 5:** Run one round on cheap models, label it a benchmark audit, and record the results and cost in `history.jsonl` and `docs/agent-weaknesses.md`. If the measured cost per scenario says the default budget of 1.0 is wrong, change `VisionsConfig` default in a separate commit with the numbers in the message.
- [ ] **Step 6:** Commit with `test(evals): visions audit` (stage the listed paths only).

## Done means

- Every task is merged, and these all pass: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web run lint && npm --prefix web test && npm --prefix web run build`, `python3 -m quality_dashboard.ratchets`, `python3 -m unittest quality_dashboard.test_dashboard quality_dashboard.test_ratchets`.
- The eval shows a Vision needing fewer person messages than goal mode, with `false_done == 0`, across the agreed number of runs. The owner sets that number before Task 12's round.
- The tier-1 live check passes on the freshly built app.
