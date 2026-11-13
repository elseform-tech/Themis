# Models Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One Settings › Models page and one `model_jobs` configuration block for every model choice, with fallback for the jobs Themis runs by itself.

**Architecture:** A new `model_jobs` block in core configuration replaces `subagents.model`, `subagents.models`, `memory.models`, `approval.auto_model` and `compaction.summary_model`. Core classifies a provider error as "can't serve" (or not); the desktop keeps an in-memory health record and runs each Themis-picks job through one fallback runner. The web gets one shared model picker and a rebuilt Settings › Models page.

**Tech Stack:** Rust (crates/core, crates/desktop), Tauri app server, React + TypeScript (web/src), Vitest, cargo test.

**Spec:** `docs/superpowers/specs/2026-11-13-models-settings-design.md` (owner-approved 2026-11-13). Board: Settings › Models, version 68, on https://claude.ai/artifact/JZgtvsAnv7iiDQUmE7zjqq.

## Global Constraints

- No backward compatibility: the five old fields are removed, not aliased; a file that still has one fails validation with a message naming its `model_jobs` replacement.
- `model_jobs` is user file only; only `model_jobs.fallback` may come from a run override. A project layer setting any of it is refused.
- A list holds 1–8 distinct entries; an entry is a model id (`<provider>/<id>` for a `providers` model) or `"chat"`.
- Fallback never moves on an ordinary request error (any 400 that is not Go's `server_error`, 401, 403, invalid request).
- The allowance mark is per provider and applies to that provider's paid models only (`ModelPrice::is_free` false or no price); free models are still tried.
- Status shows only on the Settings › Models provider card: never in pickers or job lists.
- Diagnostics are metadata only: `model_fallback` (job, from, to, reason class) and `model_health` (model or provider, state, until).
- Chat default stays the app setting (`default_provider`, `default_model`); no `model_jobs.chat`.
- Visions is out of scope (not on `main`): no Visions row or list here.
- AGENTS.md rules apply: model-facing text in `crates/core/builtins/prompts/*.md`; ratchets may only shrink; CLI e2e for backend flows; a `feature:` live-check row; docs updated in the same PR.

## Review Focus

1. A 401, 403 or ordinary 400 on the first model must fail the job as today, with no move and no `model_fallback` event (Task 4 test).
2. Every model in a list rate-limited at once: each is tried once, then the job fails with the last reason; a second job right after skips them all without a request (Task 4 test).
3. A list model the catalog no longer has: it is skipped as refused, the job uses the next one, and Settings shows the row's name unchanged (Task 4 and Task 8 tests).
4. Go allowance used up with a list of one paid and one free model: the paid one is skipped from the second job on, the free one answers (Task 3 test).
5. A `runtime.jsonc` that still has `approval.auto_model`: resolving fails with "`approval.auto_model` moved to `model_jobs.jobs.auto_reviewer`", and Settings › Permissions lists that problem (Task 1 test).

---

### Task 1: The `model_jobs` configuration block

**Files:**
- Create: `crates/core/src/configuration/model_jobs.rs`, `crates/core/src/configuration/model_jobs_tests.rs`
- Modify: `crates/core/src/configuration.rs` (field on `RuntimeConfig`), `crates/core/src/configuration/sections.rs` (remove `CompactionConfig::summary_model`, `MemoryConfig::models`, `SubagentsConfig::model`, `SubagentsConfig::models`), `crates/core/src/configuration/approval.rs` (remove `auto_model`), `crates/core/src/configuration/schema.rs`, `crates/core/src/configuration/resolve.rs`, `crates/core/src/configuration_reference.jsonc`, `docs/configuration.md`
- Test: `crates/core/src/configuration/model_jobs_tests.rs`, existing `settings_tests.rs` and `approval_tests.rs` updated

**Interfaces:**
- Produces:
  - `pub struct ModelJobsConfig { pub jobs: ModelJobs, pub fallback: bool }` (default `fallback: true`)
  - `pub struct ModelJobs { pub subagents: Vec<String>, pub auto_reviewer: Vec<String>, pub summaries: Option<Vec<String>>, pub memory: Vec<String> }`
  - `pub enum Job { Subagents, AutoReviewer, Summaries, Memory }` with `pub fn key(self) -> &'static str` (`"subagents"`, `"auto_reviewer"`, `"summaries"`, `"memory"`)
  - `pub const CHAT_ENTRY: &str = "chat";`
  - `impl ModelJobsConfig { pub fn list(&self, job: Job, chat_model: &str) -> Vec<String> }`: the job's entries with `"chat"` replaced by `chat_model`, duplicates after replacement removed, order kept; `Summaries` unset returns an empty vec (caller keeps automatic routing).
  - `RuntimeConfig::model_jobs: ModelJobsConfig`

- [ ] **Step 1: Write the failing tests** in `model_jobs_tests.rs`:
  - `defaults_match_todays_behaviour`: resolving an empty file gives `subagents == ["chat"]`, `auto_reviewer == memory == ["muse-spark-1.3-contributor", "claude-haiku-5-5", "deepseek-v4.1-flash"]`, `summaries == None`, `fallback == true`.
  - `chat_entry_resolves_to_the_chats_model`: `jobs.subagents = ["chat", "beam-free"]`, `list(Job::Subagents, "beam-free") == ["beam-free"]`.
  - `a_list_needs_one_to_eight_distinct_entries`: `[]`, nine ids and `["a", "a"]` each fail with the job key in the message.
  - `a_project_layer_cannot_set_model_jobs`: a project file with `model_jobs.jobs.memory` fails like `approval.auto_model` did.
  - `a_run_override_may_turn_fallback_off`: run override `{"model_jobs":{"fallback":false}}` resolves `fallback == false`; a run override with `jobs` is refused.
  - `old_fields_name_their_replacement`: for each of `approval.auto_model`, `compaction.summary_model`, `memory.models`, `subagents.model`, `subagents.models` a user file setting it fails with "`<old>` moved to `model_jobs.jobs.<new>`" (`subagents.model` and `subagents.models` both point to `model_jobs.jobs.subagents`).
  - `the_per_model_overrides_are_unchanged`: `models` (per-model overrides) still resolves from user and project layers.
- [ ] **Step 2: Run them.** `cargo test -p themis-core configuration::model_jobs` → FAIL (module missing).
- [ ] **Step 3: Implement** the block, its schema entry, layer rules and provenance like the `memory` block; remove the five fields from structs, schema and reference JSONC; give each removed key a validation error naming its replacement. Update `settings_tests.rs` and `approval_tests.rs` to the new fields. Update `docs/configuration.md`: a `model_jobs` table (fields, limits, layers, defaults), the five rows removed, and the Auto reviewer, memory, subagent and summary sections pointing to `model_jobs`.
- [ ] **Step 4: Run** `cargo test -p themis-core configuration` → PASS (the workspace will not build yet where consumers read removed fields; Tasks 4–6 fix them, so run only core here).
- [ ] **Step 5: Commit** `feat: model_jobs replaces the five scattered model settings`.

### Task 2: "Can't serve" error classes

**Files:**
- Create: `crates/core/src/providers/serve.rs`, `crates/core/src/providers/tests/serve.rs`
- Modify: `crates/core/src/providers.rs` (module, re-export)

**Interfaces:**
- Produces:
  - `pub enum CannotServe { RateLimited { retry_after: Option<Duration> }, AllowanceUsed { resets_in: Option<Duration> }, ContentBlocked, Outage, Refused }` with `pub fn class(&self) -> &'static str` (`"rate_limited"`, `"allowance_used"`, `"content_blocked"`, `"outage"`, `"refused"`)
  - `pub fn cannot_serve(error: &anyhow::Error) -> Option<CannotServe>`: searches the error chain for `LLMError`; `None` means an ordinary failure that must not fall back.

- [ ] **Step 1: Write the failing table test** `classifies_real_provider_errors` with these inputs (built as `LLMError` values, bodies copied from the 2026-11-13 live check):
  - 429 `rate_limit_error`, `retry_after` 40654 s → `RateLimited { retry_after: Some(40654 s) }`
  - 429 body `{"type":"GoUsageLimitError","message":"OpenCode Go usage limit reached (monthly, resets in 7d 20h)…"}` → `AllowanceUsed { resets_in: Some(7d 20h) }`
  - 400 body `{"error":{"type":"server_error","message":"Upstream request failed: [content_safety_violation] Response content blocked by label 'Jailbreak'."}}` → `ContentBlocked`
  - 400 body `{"error":{"type":"server_error","code":"server_error","message":"Upstream request failed: [server_error] Upstream response was not valid JSON"}}` → `Outage`
  - 500, 502, 499 → `Outage`; 402, 404, 410 → `Refused`
  - 400 `invalid_request_error`, 401, 403, `InvalidRequest` → `None`
- [ ] **Step 2: Run** `cargo test -p themis-core providers::tests::serve` → FAIL.
- [ ] **Step 3: Implement** `cannot_serve`. Parse "resets in Xd Yh Zm" from the message; unparseable → `None` duration.
- [ ] **Step 4: Run** → PASS. Add a proptest `never_panics_on_any_body` over arbitrary status and body strings (untrusted provider JSON, per AGENTS.md).
- [ ] **Step 5: Commit** `feat: classify provider errors a model can't serve`.

### Task 3: Model health record

**Files:**
- Create: `crates/desktop/src/state/model_health.rs`, `crates/desktop/src/state/model_health_tests.rs`
- Modify: `crates/desktop/src/state.rs` (field on the shared state)

**Interfaces:**
- Consumes: `CannotServe` (Task 2), `ModelPrice::is_free` (catalog).
- Produces:
  - `pub struct ModelHealth` with `pub fn record(&self, provider: &str, model: &str, paid: bool, why: &CannotServe, now: SystemTime)`, `pub fn ready(&self, provider: &str, model: &str, paid: bool, now: SystemTime) -> bool`, `pub fn provider_state(&self, provider: &str, now: SystemTime) -> Option<ProviderState>`
  - `pub enum ProviderState { AllowanceUsed { until: SystemTime } }` (serializes to `{"state":"allowance_used","until":<ms>}`)
  - Caps: rate limit `retry_after` capped at 24 h, missing → 60 s; allowance `resets_in` missing → 24 h, capped at 35 days; `Refused` → 1 h; `ContentBlocked` and `Outage` record nothing (the next request may pass).
  - Every `record` writes one `model_health` diagnostics event (metadata only).

- [ ] **Step 1: Write failing tests:** `a_rate_limit_rests_one_model_until_retry_after`, `allowance_used_up_rests_only_the_providers_paid_models` (Review Focus 4), `retry_after_is_capped_at_a_day`, `a_rested_model_is_ready_again_after_its_time`, `content_blocks_and_outages_are_not_remembered`, `the_record_stays_small` (records 10,000 distinct models; the map holds at most 256 entries, oldest dropped).
- [ ] **Step 2: Run** `cargo test -p themis-desktop --lib model_health` → FAIL.
- [ ] **Step 3: Implement** with a `Mutex<HashMap>` bounded at 256 entries.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat: remember which models can't serve yet`.

### Task 4: The fallback runner

**Files:**
- Create: `crates/desktop/src/state/model_jobs.rs`, `crates/desktop/src/state/model_jobs_tests.rs`

**Interfaces:**
- Consumes: `ModelJobsConfig::list`, `Job` (Task 1), `cannot_serve` (Task 2), `ModelHealth` (Task 3).
- Produces: `pub(crate) async fn on_job_models<T, F, Fut>(&self, job: Job, models: &[String], fallback: bool, attempt: F) -> Result<(String, T), anyhow::Error> where F: FnMut(String) -> Fut, Fut: Future<Output = anyhow::Result<T>>`: tries `models` in order, skipping ones `ModelHealth::ready` says are not ready; on an error, `cannot_serve` `Some` records health and (with `fallback`) moves to the next one with one `model_fallback` event; `None` returns the error at once. Returns the model that answered. All failed → the last error. `fallback == false` → only the first model, as today.

- [ ] **Step 1: Write failing tests** with a fake `attempt`: `an_ordinary_error_never_falls_back` (Review Focus 1), `a_rate_limit_moves_to_the_next_model_once` (one `model_fallback` event with from, to and `rate_limited`), `all_failing_returns_the_last_error_and_rests_them` (Review Focus 2: a second call makes zero attempts), `a_missing_model_is_skipped_as_refused` (Review Focus 3), `fallback_off_tries_only_the_first`.
- [ ] **Step 2: Run** `cargo test -p themis-desktop --lib state::model_jobs` → FAIL.
- [ ] **Step 3: Implement** `on_job_models`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat: run Themis-picks jobs down their model list`.

### Task 5: Auto reviewer and memory use their lists

**Files:**
- Modify: `crates/desktop/src/state/auto_review.rs` (both places that read `approval.auto_model` / `memory.models`), `crates/desktop/src/state/memory_auto.rs`, `crates/desktop/src/state/memory_auto/request.rs`, `crates/core/src/memory/jobs.rs` (`on_models` stays the memory loop; it reads `model_jobs` instead of `memory.models`)
- Test: `crates/desktop/tests/model_jobs_cli_e2e.rs` (new), existing `memory_cli_e2e.rs`

**Interfaces:**
- Consumes: `on_job_models` (Task 4), `ModelJobsConfig::list(Job::AutoReviewer | Job::Memory, chat_model)`.

- [ ] **Step 1: Write the failing CLI e2e** `the_reviewer_falls_back_when_its_first_model_is_out_of_allowance` in `model_jobs_cli_e2e.rs`: a scripted provider answers model A with the 429 `GoUsageLimitError` body and model B with an allow verdict; `model_jobs.jobs.auto_reviewer = [A, B]`; an Auto run's write is allowed (no question box), and `logs/` has one `model_fallback` line `{job: auto_reviewer, from: A, to: B, reason: allowance_used}`. Second test `memory_jobs_use_model_jobs_memory`: the existing memory e2e passes with `model_jobs.jobs.memory` in place of `memory.models`.
- [ ] **Step 2: Run** `cargo test -p themis-desktop --test model_jobs_cli_e2e` → FAIL.
- [ ] **Step 3: Implement:** the reviewer calls `on_job_models(Job::AutoReviewer, …)`; memory jobs read `list(Job::Memory, …)` and keep their own loop, daily budget and failure counting, also recording health through `ModelHealth`.
- [ ] **Step 4: Run** the two e2e files and `cargo test -p themis-desktop --lib auto_review memory_auto` → PASS.
- [ ] **Step 5: Commit** `feat: the Auto reviewer and memory jobs use model_jobs`.

### Task 6: Subagents and summaries use their lists

**Files:**
- Modify: `crates/desktop/src/state/subagents/access.rs` (`default_model`, `other_models`, `candidates`), `crates/desktop/src/state/subagents.rs`, `crates/desktop/src/state/subagent_tools.rs`, `crates/desktop/src/state/run.rs`, `crates/desktop/src/state/providers.rs`, `crates/core/src/runtime/summary_job.rs`, `crates/core/src/runtime/documents.rs`, any prompt file under `crates/core/builtins/prompts/` that names `subagents.models` (grep), `crates/desktop/src/types/threads.rs`
- Test: `crates/desktop/tests/model_jobs_cli_e2e.rs`

**Interfaces:**
- Consumes: `ModelJobsConfig::list(Job::Subagents | Job::Summaries, chat_model)`, `on_job_models`.
- Behaviour: the agent may give one subagent any model in the subagents list; any other is refused with the existing message, naming the list. A subagent tries its chosen model, then the rest of the list, then the chat's model last (today's last resort), through `on_job_models` when fallback is on. Summaries: an empty `list` keeps today's automatic routing unchanged; a set list replaces it (first model, fallback down the list) and the run still fails before it starts when a listed model is not in the captured catalogs.

- Chat (no fallback): a chat run whose model can't serve records it in `ModelHealth` and fails with the reason first, from `CannotServe`: "<model> is rate limited until HH:MM", "<provider>'s allowance is used up until <date>", "<model> refused this request", "<model> is unavailable right now" (UI text, not model-facing).

- [ ] **Step 1: Write failing e2e tests:** `a_subagent_falls_back_down_the_subagents_list` (first model 429, second answers, the subagent card's model is the second), `a_subagent_model_outside_the_list_is_refused`, `summaries_without_a_list_keep_automatic_routing` (request goes to `longcat-2.5-preview-free` as today), `a_summaries_list_replaces_routing_and_falls_back`, `a_chat_says_why_its_model_could_not_answer` (429 with `retry_after` → the `failed` event's error starts "beam-free is rate limited until"; no other model is called).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** Keep the subagent card's model field the model that answered.
- [ ] **Step 4: Run** `cargo test --workspace` → PASS (first point where the whole workspace builds again).
- [ ] **Step 5: Commit** `feat: subagents and summaries use model_jobs`.

### Task 7: Server methods and the in-app writer

**Files:**
- Modify: `crates/desktop/src/state/configuration.rs` (add `user_model_jobs_view`, `patch_user_model_jobs`; remove `model`/`models` from the subagents `EDITABLE` list; `set_user_memory_settings` loses `models`), `crates/desktop/src/server/commands.rs` (methods), `AGENTS.md` (Desktop UI exception: Settings › Models also writes the `model_jobs` block), `docs/configuration.md`
- Test: `crates/desktop/tests/model_jobs_cli_e2e.rs`

**Interfaces:**
- Produces server methods:
  - `get_model_jobs` → `{ "model_jobs": ModelJobsConfig, "revision": String, "providers": [{ "id": String, "name": String, "state": ProviderState | null }] }`
  - `update_model_jobs { changes: { "jobs.<key>"?: string[] | null, "fallback"?: bool }, revision }` → same shape; stale revision refused with the existing stale message style; `null` clears a job to its default.
  - `update_subagent_settings` no longer accepts `model`/`models`; `set_memory_settings` no longer accepts `models`.

- [ ] **Step 1: Write failing CLI e2e:** `model_jobs_round_trip_with_revisions` (write `jobs.subagents`, read back, stale revision refused, file text keeps the person's comments outside the block), `a_bad_list_is_refused_and_nothing_is_written`, `the_provider_state_reports_allowance_used` (after a `GoUsageLimitError` the `providers` entry has `allowance_used` and `until`).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** like `patch_user_subagents_config`.
- [ ] **Step 4: Run** `cargo test -p themis-desktop --test model_jobs_cli_e2e --test settings_changes_e2e` → PASS.
- [ ] **Step 5: Commit** `feat: Settings › Models writes model_jobs in place`.

### Task 8: The model picker

**Files:**
- Create: `web/src/components/ModelPicker.tsx`, `web/src/components/ModelPicker.css`, `web/src/components/ModelPicker.test.tsx`, `web/src/components/ProviderMark.tsx`
- Modify: `web/src/lib/models.ts` (keep `groupByProvider`; add `providerName(model: GoModel): string`), `web/src/screens/ThreadHeader.tsx` (the header's model control opens `ModelPicker`)

**Interfaces:**
- Produces:
  - `ModelPicker({ models, selected, onPick, chosen, offerChat, onClose }: { models: GoModel[]; selected?: string; onPick: (id: string) => void; chosen?: string[]; offerChat?: boolean; onClose: () => void })`: a popover with a search field (matches model id and provider name, case-insensitive), provider chips ("All" plus one per provider present in `models`), and every model alphabetically, each row a `ProviderMark` and the id; `selected` gets a check; `chosen` rows are greyed with a check and not pickable; `offerChat` adds "Same as chat" first (picks `"chat"`). No status text, no "Default".
  - `ProviderMark({ provider }: { provider: string | null })`: a 16 px rounded square with the provider's initials (OpenCode Go → "OC"); no third-party artwork.

- [ ] **Step 1: Write failing tests:** `searches_model_and_provider_names`, `provider_chips_filter_the_list`, `checks_the_selected_model`, `greys_models_already_in_the_list`, `offers_same_as_chat_only_when_asked`, `shows_no_status_text` (no "Paused", "Rate limited", "Default"), and in `ThreadHeader.test.tsx` `the_header_shows_only_the_model_name`.
- [ ] **Step 2: Run** `npm --prefix web test -- ModelPicker ThreadHeader` → FAIL.
- [ ] **Step 3: Implement** to DESIGN.md tokens; keyboard: arrows move, Enter picks, Esc closes.
- [ ] **Step 4: Run** → PASS; `npm --prefix web run lint`.
- [ ] **Step 5: Commit** `feat: one searchable model picker with provider chips`.

### Task 9: Settings › Models page

**Files:**
- Modify: `web/src/screens/ModelsSettings.tsx` (providers card, You pick, Themis picks, fallback switch), `web/src/screens/SubagentSettings.tsx` (remove Default model and Allowed models), `web/src/lib/tauri.ts`, `web/src/lib/types.ts`
- Delete: `web/src/screens/MemoryModels.tsx`, `web/src/screens/MemoryModels.test.tsx`
- Create: `web/src/screens/ModelJobs.tsx`, `web/src/screens/ModelJobs.test.tsx`
- Test: `web/src/screens/ModelsSettings.test.tsx`

**Interfaces:**
- Consumes: `getModelJobs()`, `updateModelJobs(changes, revision)` (Task 7, added to `tauri.ts`), `ModelPicker` (Task 8).
- Layout (board version 68): provider cards first (mark, name, state text such as "Allowance used up · back 21 Nov" in the warning colour when `state` is set); "You pick" with Chat (the existing default-model setting, via `ModelPicker`); "Themis picks" with Subagents, Auto reviewer, Summaries, Memory, each a row showing its list comma-separated ("Automatic" for Summaries unset) that opens to a draggable list (handle, position, id, provider label, remove) and **Add** (opens `ModelPicker` with `chosen` = the list and `offerChat`); then "Fall back to the next model" switch. No status in rows.

- [ ] **Step 1: Write failing tests:** `lists_each_themis_picks_job_in_order`, `reordering_writes_the_job_list`, `add_appends_and_greys_added_models`, `removing_the_last_model_is_not_allowed`, `the_switch_writes_fallback`, `the_provider_card_shows_allowance_used`, `a_model_missing_from_the_catalog_keeps_its_name` (Review Focus 3), `subagent_settings_has_no_model_controls`.
- [ ] **Step 2: Run** `npm --prefix web test -- ModelsSettings ModelJobs SubagentSettings` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `npm --prefix web test`, `npm --prefix web run lint`, `npm --prefix web run build` → PASS.
- [ ] **Step 5: Commit** `feat: Settings › Models shows providers and every job's models`.

### Task 10: Evals, guides and the live check

**Files:**
- Modify: eval harnesses under `evals/` that start runs (grep for run overrides), `docs/user-guide.md`, `docs/live-verification.md` (row `feature:models-settings`), `docs/agent-weaknesses.md` only if an eval measure changes
- Test: the eval self-tests (`python3 -m unittest` for each touched harness), `cargo test -p themis-desktop --test live_coverage`

- [ ] **Step 1: Write failing self-tests:** each touched harness's run override contains `{"model_jobs":{"fallback":false}}` and records it in the result.
- [ ] **Step 2: Run** them → FAIL.
- [ ] **Step 3: Implement**; add the live-check row: in Settings › Models, put a paid model first in Auto reviewer while the Go allowance is used up (or with a scripted provider), run an Auto step: it is reviewed by the next model, `logs/` has `model_fallback`, the provider card shows the allowance state; open Add, search "flash", add a model, drag it first; the chat header's picker searches and filters by provider and shows no status.
- [ ] **Step 4: Run** the full checks from AGENTS.md (fmt, clippy, `cargo test --workspace`, web lint/test/build, ratchets, dashboard tests) and the real app on a free model per the live row → PASS, screenshots of each sky.
- [ ] **Step 5: Commit** `docs: models settings guides and live check`.
