# Prompt queue with steering, and goal mode

Date: 2026-10-18 · Branch: `codex/goal-mode` · Two parts, shipped in order:
part 1 (prompt queue and steering) first, part 2 (goal mode) built on it.

Mockups: canvas https://claude.ai/artifact/Em6B9YcQir5EFWhwUN7PEJ, row "Goal
mode", boards 16–19 (`GoalAuto`, `GoalRunning`, `GoalEdit`, `GoalStates`).

## Why

Today the composer locks while a run is going ("Run in progress…",
`web/src/screens/ThreadComposer.tsx`). You cannot add a thought, correct the
agent mid-run, or line up the next request. You wait, or you press Stop and
lose the turn.

Long outcome-shaped work ("optimize this app to speed up p95") needs many
rounds. Today each round needs you to type "continue", and agents stop as
soon as their own tests pass (see the spec-check evidence in
`long-horizon-strategy`). The person wants to hand over one sentence and
have the agent keep going, for hours if needed, until the goal is reached.
They step in only to steer, edit the goal, or pause.

## Goals

1. **Prompt queue (all chats, with or without a goal).** While a run is
   going, you can type and send. Sent messages wait in a queue above the
   composer and go out automatically, in order, when the run finishes.
2. **Per queued message, three icon buttons with tooltips:**
   - **Steer:** deliver it into the running run now, read before the agent's
     next step, without stopping the run.
   - **Edit:** change the text while it waits.
   - **Undo:** remove it from the queue before it is sent.
3. **Goal mode.** A goal is one editable sentence. The agent keeps starting
   new turns until the goal is reached. There is no limit on time, turns or
   cost.
4. **Starting a goal, two ways:**
   - `/goal <sentence>` starts immediately.
   - A plain prompt that clearly asks for an ongoing outcome makes the agent
     ask once, in one line, whether to make it a goal. Your reply in your own
     words decides.
5. **The goal bar on the composer** shows the goal sentence and its elapsed
   time, with icon buttons: Edit, Pause/Resume, Show/Hide. Collapsed, it is
   one line (sentence truncated). Expanded, it shows the full sentence. There
   is nothing else on the bar.
6. **The goal is persistent.** It survives app restarts and resumes by
   itself when Themis opens. Your messages are handled first, then the goal
   continues. It pauses only when:
   - it is reached (it ends);
   - it is genuinely stuck;
   - the model quota is used up.

## Non-goals

- No checklist of "done when" items, no limits form, no Start button, no
  Stop button for the goal. The only manual goal actions are edit, pause and
  resume.
- Goals do not appear in the Workspace Tasks tab or the header task
  indicator.
- No undo of sent messages or of the agent's changes. "Undo" only removes a
  queued message.
- No new permissions. A goal, a queued message, or a steer never grants or
  widens approvals. Hooks and external instructions still never grant
  permissions.
- Goals are not built on automations. Automations wake on a clock (2 min
  minimum), carry expiry and run limits, and show in Tasks. None of that fits.

## Anti-goals (failure even if it "works")

- A goal that silently stops: a crash, a provider error or a restart leaves
  it idle without saying why.
- A goal that loops forever repeating the same failed attempt. "No limit"
  must not mean "no stuck detection".
- The agent grading itself: "reached" decided only from the agent's own
  claim.
- Steering that drops or reorders your words, or delivers a message twice.
- A cluttered composer. Every element must earn its place (DESIGN.md, "Only
  what earns its place").
- Prompts, goal text or tool output written to diagnostics logs (AGENTS.md:
  metadata and correlation IDs only).

## Part 1 — Prompt queue and steering

### Behavior

- **Queuing a message.** While a run is going, Send queues the message
  instead of being disabled. Queued messages show as a small stack directly
  above the composer: one row each, text truncated to one line, oldest first.
- **When a run finishes** (finished, failed, or stopped by you), the oldest
  queued message is sent as a normal message, which starts the next run.
  Others stay queued until their turn.
- **Steer** moves that message out of the queue and delivers it to the
  running run. The agent sees it before its next model call. The row
  disappears from the queue. In the transcript it appears as your message at
  the point it was delivered, so the record shows when the agent saw it.
  - If the run ends before the steer is picked up, the message is not lost:
    it is sent as the next message instead.
  - With no run going, Steer is not shown; Send works as today.
- **Edit** turns the row into an inline text field. Saving keeps its place in
  the queue; Escape cancels. While a row is being edited, it is held, even if
  it is next in line: the next row is sent instead.
- **Undo** removes the row. There is no confirmation; the action is a single
  removal of text you just wrote. **My call:** a short "Removed · Restore"
  toast is not added; keep it minimal.
- **Attachments, skills and plugins** chosen in the composer travel with the
  queued message.
- **Stop** behaves as today and does not clear the queue. The queue then
  drains as described.

### Where the queue lives

- **The queue is per chat and lives in the app server**, not only in the web
  state. Queued messages then survive a window reload and reach the CLI. It
  is persisted in app data as `queues/<chat id>.json`, in the same
  folder-per-concern pattern as `chat_grants/`, with the same safe-id check
  and the same "damaged file is ignored and logged with a fixed reason"
  rule.
- **App server API:** enqueue, edit, remove, steer, and list for a chat. The
  web UI and `themis` CLI both use it.

### Runtime change for steering

- `run_task` in `crates/core/src/runtime.rs` already checks a shared stop
  flag before each model call (`check_stopped`). Add a per-run **steer inbox**
  beside it: a shared, ordered list the server pushes into and the run loop
  drains before each model call.
- Each drained message is appended as a user message, marked so the
  transcript and the history record it as yours and delivered mid-run.
- It is included in compaction like any user message, and the request text
  is still carried verbatim.
- A steer that arrives while a tool is executing waits for that tool to
  finish. Tools are never interrupted by steering.
- A steer that arrives while the run waits on an approval is delivered after
  the approval is decided. Steering never answers an approval prompt.

## Part 2 — Goal mode

### Starting a goal

- **`/goal`** is listed first under a **Commands** heading in the existing `/`
  picker (`web/src/components/PromptInput.tsx`), above Skills.
  - It looks like a skill but is a built-in command, because a goal needs the
    runtime to keep starting runs, which a skill (instructions only) cannot
    do.
  - Sending `/goal <sentence>` creates or replaces the chat's goal and starts
    it.
  - `/goal` with no sentence asks for one, inline.
- **Natural start.** The agent gets a `set_goal` tool (own-chat only, no
  approval, like own-chat automation lifecycle).
  - The system prompt tells it: when a request clearly needs many rounds of
    work toward a measurable or judgeable outcome, ask once in one line
    ("Want me to make this a goal and keep working until it's reached?").
  - On your agreement, it calls `set_goal` with the sentence you agreed on,
    including any conditions you added in your reply.
  - It never asks for questions or one-step changes, and never asks twice
    about the same request.
- **One goal per chat.** Setting a new goal replaces the current one.
- **The bare word "goal"** in a sentence does not trigger anything.

### Goal state

Stored per chat in app data as `goals/<chat id>.json`, same pattern as
`chat_grants/`. Fields:

- `text`: the sentence.
- `status`: `running`, `paused_by_you`, `waiting` (for you, or for an
  approval), `stuck`, `quota`, or `reached`.
- `reason`: a short sentence for stuck/quota/reached.
- `started_at`.
- `active_ms`: accumulated running time.
- `running_since`: set while running.
- `turns`.
- Recent check verdicts, used for stall detection.

**Elapsed time** is `active_ms` plus time since `running_since`. It counts
only while the goal is running: not while paused, waiting, out of quota, or
while the app is closed or the machine asleep.

### The loop (goal supervisor)

The app server already knows when a run ends: the terminal-event pump and
`finish_run` in `crates/desktop/src/state/run.rs`. Add one step after a run
ends in a chat whose goal is `running`:

1. **Queued messages go first.** If the chat's queue is not empty, the
   queue's next message is sent as usual. The goal waits for that run, then
   continues. Your requests are always handled before the goal resumes.
2. **Otherwise, run the goal check** (below), but only after a goal turn.
   - If the run that just ended answered one of your messages, skip the check
     and start the next goal turn directly. Its continuation message says
     your request was handled in between.
3. **Act on the verdict:**
   - **Reached:** set status `reached`, post the verdict's reason in the
     chat, stop.
   - **Not yet:** start the next goal turn immediately through the same path
     automations use to start a run in a chat (`send_message_with_options`).
     The message is marked as coming from the runtime, not from you:
     `[Goal — runtime, not from the user] Goal: <text>. Last check: <reason>. Continue toward the goal.`
   - **Can't be reached as written:** set status `stuck`, post why, wait for
     an edit.

**One goal turn is one ordinary run.** It gets compaction, the task ledger,
`files_changed`, approvals, the spec check, Stop and history unchanged. Each
run keeps its existing per-run turn cap (`max_total_turns`), so a single run
cannot loop forever. The goal itself has no cap.

**Several chats can each have a running goal.** They share the existing
concurrency limit (`running_count`). A goal turn that cannot start because
the limit is reached waits for a free slot. It is not an error and never
pauses the goal.

**The CLI** gets the same controls: `/goal <sentence>` in a message, plus
commands to show, edit, pause and resume a chat's goal. Exact names are the
builder's call.

**Edits apply at the next turn.** Edit saves the new text to the goal file.
The next continuation message reads whatever text is saved. Nothing polls
for edits. This matches "configuration is captured once per run".

### The goal check

- A separate model call with a fresh context, using the chat's model. It
  sees:
  - the goal text;
  - the run's final answer;
  - the commands that run executed, with their bounded output (from the
    ledger/evidence);
  - how the agent said it measures the goal.

  It never sees the agent's chat history beyond that, and treats all of it as
  data, not instructions.
- **Returns structured JSON:** `{verdict: reached | not_yet | unreachable | needs_user, reason, progress_signal}`.
  - `needs_user` means the run's final answer asks you something only you
    can decide. It sets status `waiting` (see below).
  - `progress_signal` is one short measured fact (for example `p95 238 ms`),
    used only for stall detection and the chat note, never shown on the goal
    bar.
- **"Reached" is the model's judgement.** The person's decision: good metrics
  come from the person or the agent, so the model decides.
  - On the first goal turn, the agent states in the chat what it will measure
    and how, and takes a baseline.
  - If the goal has a number, the number is the bar.
  - Without one, the check may call it reached when further gains are not
    worth it, and must say why.
- **A malformed or failed check** is retried once. If it still fails, the
  supervisor continues with "not yet" and a note that the check failed. A
  goal never stops because the checker failed.

### When a goal pauses (and only then)

- **Reached:** ends.
- **Genuinely stuck:**
  - Three consecutive turns with no progress: no `files_changed` and the same
    or worse `progress_signal`. Status `stuck`, with reason.
  - The check says the goal can't be reached as written. Status `stuck`,
    waiting for an edit.
  - The agent needs an answer only you can give. It asks in the chat and the
    status becomes `waiting`. **Your reply resumes the goal** after that run.
- **Model quota used up.** Status `quota`. The supervisor retries every 15
  minutes (**my call**) and resumes by itself when a request succeeds.
  - Distinguishing "quota used up" from a short rate limit needs the provider
    response. Both arrive as HTTP 429 `RateLimitError`
    (`crates/core/src/providers.rs`).
  - Treat it as quota when the body or provider code says quota/credits/usage
    limit, or when `retry_after` exceeds 10 minutes (**my call**).
  - Otherwise it is a rate limit: wait `retry_after` (or back off) and carry
    on, without pausing.

### What does not pause a goal

- **Network and provider errors:** timeouts, 5xx, short rate limits. These
  are retried with growing waits (30 s, 1 min, 2 min … capped at 15 min,
  **my call**).
- **A failed or crashed run:** the next goal turn starts with a runtime note
  that the previous turn failed and why. The automations rule "stop after
  three failures" does not apply.
- **Waiting for an approval** in Custom mode: the goal waits inside that run;
  the clock stops while waiting.
- **Stop:** ends the current run only. The goal continues with the next
  turn. **My call:** Stop on the composer stops the turn, Pause on the goal
  bar stops the goal; this keeps "only edit, pause, resume" for goal
  controls.
- **Closing the app or restarting.** On start, the server finds goals whose
  status is `running`, `quota`, or `waiting` on a failed run, and resumes
  them.
  - The interrupted turn cannot be continued mid-step. The next turn starts
    with a runtime note: "the previous turn was interrupted; re-check what
    was in progress before continuing."
  - A goal you paused stays paused.

### Changing the model mid-goal (applies to every chat)

The person decided: allowed, and it waits for the current turn.

- **What exists today.** `set_provider` refuses to switch while a run is
  active (`crates/desktop/src/state/threads.rs`, "cannot switch provider
  mid-run"). That would lock a goal to its first model for hours.
- **New behavior.** Picking a model while a run is going saves it as the
  chat's **next model**. The header pill shows it right away, with a tooltip
  "Applies from the next turn". The current run finishes on its snapshot's
  model, and every later run (goal turns, queued messages, steers that fall
  through to the next run) uses the new one. Nothing is stopped or restarted.
  This replaces the busy error in all chats, not only goal chats.
- **Reasoning effort** resets on a switch, as today. A new effort chosen
  during a run also applies from the next turn.
- **The goal check** uses the chat's current model, so it follows the switch.
- **Quota pause:** when the goal is paused for quota, switching to a
  different model resumes it immediately instead of waiting for the next
  15-minute retry.
- **Context size:** each run already sizes its budget from its model's
  capacity. If the new model's window is smaller than the retained history,
  the existing compaction and overflow recovery shrink it before the first
  call.
- **Reasoning replay:** the previous model's reasoning must not be replayed
  to a different model. Switching between runs already exists, so this is
  likely handled already. Add a regression test either way: one model's
  turn, a switch, the next turn's provider request carries no foreign
  reasoning.

### Your controls

- **Edit:** the bar becomes a text field with Save and Discard icons. Saving
  applies from the next turn; the current turn keeps running.
- **Pause:** finish the current turn, then don't start another. Status
  `paused_by_you`.
- **Resume:** status `running`. If no run is going, start a goal turn now.
  - On `stuck`, Resume starts a turn with your edited goal or guidance.
  - On `quota`, Resume tries immediately.
- **Show/Hide:** expand or collapse the bar. A per-viewer preference only,
  remembered locally.

### UI notes

- **The goal bar** sits on top of the composer, inside the same rounded
  container, as in boards 16–18.
  - It shows the target icon, the sentence and the elapsed time (`H:MM:SS`,
    tabular numerals).
  - Icons, each with a tooltip and `aria-label`: Edit goal, Pause after this
    turn / Resume, Show/Hide goal.
  - No progress numbers or turn counts on the bar; progress is reported in
    the chat.
- **The queue stack** sits between the goal bar (if any) and the input row.
  Each row: the text, then the Steer, Edit and Undo icons with tooltips
  ("Steer — send now", "Edit", "Remove from queue").
- **The composer placeholder** while running changes from "Run in progress…"
  to "Queue a message — or steer it in".
- **Goal turn separators** in the chat ("Turn 24"), and the check's short
  "Not reached yet — <reason>" note, as in board 17.
- **Endings** as in board 19, plus one card for "Paused · quota used up". The
  bar keeps only Edit and Resume when paused, and nothing when reached.

## Constraints (from AGENTS.md and the codebase)

- **Branch:** `codex/goal-mode`. Small commits per logical step, each with
  its tests and docs.
- **Persistence:** new app-data files (`queues/`, `goals/`) follow the
  `chat_grants/` pattern: safe chat ids only, a damaged file is ignored with a
  fixed diagnostic reason, deleted with the chat. Do not put them in
  `runtime.jsonc`.
- **Diagnostics:** log ids, status changes, verdict kinds, retry counts and
  timings. Never log goal text, queued text, check prompts or outputs.
- **Untrusted input:** the goal check's JSON output (model output) and the
  queue and goal files gets a property test. The goal-check parser gets a
  fuzz target (`docs/fuzzing.md`).
- **Tests:**
  - Shared-server and CLI regression tests through the local app server and
    `themis` for queue, steer, goal loop, pause/resume, restart resume,
    stuck, and quota.
  - Vitest for the composer stack, goal bar and `/goal` picker entry.
  - Browser or native checks of the journeys.
- **Docs:** update the user guide, `docs/configuration.md` if any field is
  added, and the "Run in progress" behavior.
- **Evaluation** (`optimization-goal`): before shipping goal mode as the
  default, A/B on `evals.hard` the long tasks, run as one `/goal`, against
  the same prompts without it. Measure:
  - completion;
  - redone work;
  - calls per finished task.

  Report it as a benchmark audit.

## Left to the builder

- Exact names of API methods, files and message markers.
- How the server tracks "the commands a run executed" for the check: reuse
  the ledger or evidence directory rather than adding new storage.
- The exact system-prompt wording for the natural "make this a goal?"
  question, kept short.
- Toast and animation details, within DESIGN.md.
- Whether the queue stack shows at most N rows with "+k more".

## Calls made in this document (please check)

- **Retry and backoff:**
  - quota retry every 15 minutes;
  - quota = provider says so, or `retry_after` > 10 min;
  - error backoff capped at 15 min.
- **Stall:** 3 turns with no file changes and no better progress signal.
- **Undo** has no confirmation and no restore toast.
- **Stop** stops the turn; the goal continues. Pause stops the goal.
- **A message being edited** is skipped by the queue until saved.
- **The goal check** uses the chat's model.
- **Show/Hide** is a local per-viewer preference.
- **A failed goal check** continues as "not yet" instead of pausing.
- **No check after a run that answered your message.** The goal turn starts
  directly after it.
- **The check detects "needs you"** (`needs_user`) rather than the agent
  setting it.
- **Goals in different chats** run side by side within the existing
  concurrency limit.
- **A model switch during a quota pause** resumes the goal immediately.
- **The pill shows the next model at once** (with a tooltip), rather than
  keeping the old model's name until the run ends.
