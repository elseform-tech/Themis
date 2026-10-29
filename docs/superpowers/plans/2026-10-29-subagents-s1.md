# Subagents S1: read-only subagents, implementation plan

Spec: `docs/superpowers/specs/2026-10-29-subagents-design.md` (PR #138). S1 ships in two PRs:
**S1a** (this plan): backend and CLI. **S1b**: the desktop UI (boards 25–28).

S1 keeps subagents **read-only**. Write paths are S2. Steering from the composer chip is S3, but
the `steer_subagent` tool ships here. Plugin `agents/` profiles are S4.

## Constraints from the runtime map

- Each run's context lives in many task-locals (steer inbox, compaction config, catalog limits,
  usage recorder, hooks, diagnostics). They're set up in the desktop at `run.rs:672-739`.
- A terminal `RunEvent` on a run's channel ends the chat turn (RunEnd hooks, `finish_run`,
  `files_changed`). Child events must never reach the parent channel.
- Stop, steering, approvals and browsers are keyed by `thread_id` only.
- `ApprovalHook::approve` is synchronous and blocks. `ToolAction` and `ApprovalRequest` carry no
  agent identity.
- Background tasks only notify the UI. The steer inbox is the one path that delivers text into a
  running agent.

## Tasks

Each task ends with its tests passing, then a commit.

1. **Configuration.** Add a `subagents` block to `configuration.rs` with `enabled` (true),
   `max_parallel` (3, range 1–6), `max_turns` (40), `model` (null) and `allow_writes` (true; unused
   until S2).
   - Validation, provenance and the project-may-only-lower rule.
   - `model` gets the same provider-prefix check as `goal.check_model`.
   - Update `configuration_reference.jsonc` and `docs/configuration.md`.
   - Tests: resolver precedence, the lower-only rule, a bad model, out-of-range values.
2. **Prompts.** Add `subagent-system`, `subagent-read-only-note`, `subagent-status-field`,
   `spawn-subagents-tool`, `subagent-result-note` and `subagent-denied-note`, registered in the
   inventory test.
   - `initial_messages` gets an optional system-head override (a `RunPolicy` field), so a
     subagent's system message is `SUBAGENT_SYSTEM` instead of the main one.
3. **Runtime: waiting and status.**
   - **Waiting:** a task-local `SUBAGENT_WAIT` handle, with pending count and notify.
     - When the model answers without tool calls while subagents are still pending, the run waits
       for the next result note instead of finishing.
     - The note arrives through the steer inbox, tagged so the steers check doesn't count it as a
       person's steer.
     - Stop interrupts the wait.
   - **Status field:** for runs with `status_field` on, every tool schema gains an optional
     `status` string (at most 80 characters). It's removed from the arguments before the tool
     runs, and emitted as `RunEvent::Status`.
   - Tests in `runtime_e2e` with a scripted provider.
4. **Run scope helper (desktop).** Move the task-local stack from `spawn_run` into
   `fn scoped(…) -> impl Future`, used by the main run and by children. This is a refactor only:
   the full suite must stay green.
5. **Subagent manager (desktop).** Add `AppStateInner.subagents: thread_id → Vec<Subagent>`. Each
   subagent holds an id, name, brief, status, stop flag, steer inbox, start time, tool-call count
   and its transcript path.
   - **Child run:**
     - its own mpsc channel and `run_id`;
     - the read-only tool set: list/read/search, plus shell without background and without
       escalation;
     - a read-only sandbox plan (a new `PlanInputs` flag: no project write, temp only);
     - the main snapshot's mode and rules;
     - the model from `subagents.model`, else the chat's;
     - `max_turns`;
     - usage recorded under the parent thread.
   - **Limits:** at most 6 per call, and at most `max_parallel` running per chat. The rest queue.
   - **Transcripts:** the child's events go to `chat_data/subagents/<id>.jsonl` (bounded), which is
     deleted with the chat. At startup, any entry still marked running is rewritten as "lost after
     restart".
6. **Tools (desktop):** `spawn_subagents`, `wait_subagents`, `stop_subagent` and `steer_subagent`,
   built like `QuestionTool` with AppState, sink, ids and the parent inbox.
   - The result note is at most 8 KiB: the final answer, or partial work plus the reason it ended.
   - Children never get the subagent tools.
7. **Approvals.**
   - A `SubagentApprovals` wrapper tags requests with the subagent's id and name. A new optional
     `subagent` field on `ApprovalRequest` and `PendingApproval` carries them.
   - Stop on a subagent denies only its pending approvals. Stop on the chat denies all of them.
   - "Allow in this chat" on a subagent's request is kept in that child's `CachingApprovals` only.
     It never goes to the chat grants file, so it covers only that subagent and leaves the file
     format unchanged.
8. **Hooks.**
   - The child runs under `hooks::ACTIVE`, so its `BeforeTool` and `AfterTool` hooks fire, and the
     payload gains `agent: {id, name}`.
   - New canonical events `SubagentStart` and `SubagentEnd`; Claude's `SubagentStart` and
     `SubagentStop` are mapped to them.
   - `RunStart` and `RunEnd` stay on the main run.
9. **Events and persistence.**
   - New `ThreadEvent` variants: `SubagentStarted`, `SubagentStatus` and `SubagentEnded`, saved to
     the chat history like other events. The chat card in S1b reads them.
   - Diagnostics (metadata only): start and end, status, duration, tool-call count, profile, and
     whether a sandbox enforced the run.
10. **CLI and end-to-end tests.**
    - A scripted provider runs the main agent, which spawns two subagents, keeps working, then
      waits. Checks:
      - the results arrive as notes;
      - the turn ends only after the subagents;
      - the events are saved;
      - usage rows carry the parent thread.
    - Stop on one subagent; a denied approval; a restart in the middle of a spawn; `enabled: false`
      hides the tools.
    - A hook test: a `BeforeTool` guard blocks a subagent's call.
11. **Docs:** `docs/configuration.md`, `docs/plugins.md` (hooks), `docs/user-guide.md`
    (subagents), and AGENTS.md (the YOLO walls entry waits for S2's write paths).
12. **Live check:** one real run on `beam-free` through the CLI, a review task split across two
    read-only subagents, labelled as a benchmark audit.

## Out of S1a

- The UI: the chat card, Tasks detail, overview row, Settings › Subagents and approval labels.
  These are S1b.
- Write paths (S2), the composer steer chip (S3), plugin profiles (S4), and giving subagents
  their own browser tab (a later step; in S1 subagents get no browser tools).
