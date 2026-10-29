# Subagents: delegated agents you can watch

Date: 2026-10-29 · Branch: `codex/subagents-design` · Design only; nothing built yet.

Mockups: canvas https://claude.ai/artifact/Em6B9YcQir5EFWhwUN7PEJ, row "Subagents",
boards 25–28 (`AgentsChat`, `AgentsDetail`, `AgentsStates`, `AgentsSettings`).

The UI calls them **subagents** everywhere (chat card, Tasks, approvals, Settings). UI text is
short and has no hint lines.

## Why

- Themis has no way for the agent to hand part of a task to another agent. Reviews, audits and
  investigations that touch several areas run one after another in one context, and fill it.
- 33 of the 315 official catalog plugins ship Claude subagents (`agents/`). Themis skips them
  today (catalog check of 2026-10-29), and 10 of those plugins are blocked by nothing else.
- The person needs to see what each subagent is doing without the chat turning into several
  interleaved tool streams.

## Goals

1. **The main agent and its subagents work at the same time.** Starting subagents returns
   immediately. The main agent keeps working, gets each result as it arrives, and can steer,
   stop or wait for its subagents.
2. **One card in the chat** per batch of subagents, with one live row per subagent: its status,
   its job, a short status line, its tool calls and its time.
3. **Workspace › Tasks shows one subagent in full:** its brief, a live transcript, its model,
   permissions and cost, and Stop.
4. **Subagents are read-only by default.** They may write only to the folders and files the main
   agent gives them, and no two subagents own the same path.
5. **No subagent widens permissions.** Approvals appear on the main composer, named after the
   subagent.

## Non-goals

- Subagents that start subagents (depth is 1).
- Subagents talking to each other. Results flow only to the main agent.
- A separate subagent chat or subagent editor in the UI. Definitions come from plugins and from
  the main agent's brief.
- Changing how goal checks work. A goal check runs after the main turn ends, as today. A main
  turn ends only after its subagents have ended (see Lifecycle).

## How it works

### Tools for the main agent

| Tool | What it does |
|---|---|
| `spawn_subagents` | Starts one or more subagents and returns their ids at once |
| `wait_subagents` | Waits until the listed subagents (or any one of them) end, with a timeout of at most 10 min; returns their results |
| `steer_subagent` | Sends a note into one running subagent |
| `stop_subagent` | Stops one running subagent; its partial result comes back as usual |

```json
{"subagents": [
  {"name": "Bug hunt", "brief": "Find correctness bugs in …. Do not edit files.",
   "profile": "code-reviewer"},
  {"name": "Test gaps", "brief": "…", "write_paths": ["crates/core/tests/"]}
]}
```

- `profile` is optional. It names a subagent definition installed from a plugin's `agents/`
  (see Plugins).
- `write_paths` are project-relative folders (ending in `/`) or files, never globs.
  - They are rejected if they overlap another running subagent's paths, escape the project, or
    reach the app data or the project's `.themis/`.
  - Because they are plain prefixes, checking for overlap is a prefix comparison, and the paths
    map directly to sandbox allow-lists.
- **Limits per call:** at most 6 subagents, and at most `subagents.max_parallel` running at once.
  Extra subagents wait their turn.
- **Results:**
  - As each subagent ends, its result is delivered to the main agent's steer inbox as a labelled
    note, read before the main agent's next model request.
  - The result is bounded at 8 KiB: the final answer, or what it had so far plus why it ended
    (stopped, failed, denied, turn or cost limit).
  - The full answer stays in the subagent's transcript.

### Lifecycle

- **A subagent is a child run of the current main turn.** It uses the main turn's captured
  configuration and approval mode snapshot; configuration is still captured once per run.
- **Its starting context** is the subagent system prompt, its profile and its brief. It doesn't
  see the main chat history.
- **Each subagent has its own steer inbox.** The current inbox is task-local
  (`runtime.rs` `STEER_INBOX`), so each child run is started in its own scope. Steers meant for
  the main agent never reach a subagent, and the reverse holds too.
- **When the main agent has nothing left to do** while subagents still run, the runtime waits for
  them instead of ending the turn. Each result then wakes the main agent for another model
  request.
  - The turn ends only once the main agent answers and no subagents are running.
  - So a turn's goal check, `RunEnd` hooks and `files_changed` always come after its subagents.
- **Stopping:**
  - Stop on the main turn stops all its subagents.
  - Stop on a subagent (Tasks or `stop_subagent`) ends that subagent only.
- **Restart:** if the app restarts mid-turn, running subagents are recorded as "lost after
  restart" in their transcripts. The main agent's resumed turn (goal mode) gets that as their
  result. Nothing restarts on its own.
- **Limits per subagent:**
  - `subagents.max_turns` model requests, default 40.
  - Its own context compaction, using the same checkpoint rules as the main agent.
  - Hitting a limit ends it with its partial result.
- **Cost:** subagent usage counts toward the chat's cost row and Settings › Usage, attributed to
  the chat. The model is `subagents.model`, or the chat's model when unset. It is validated with
  the same provider-prefix check as `goal.check_model`.
- **Storage:**
  - Transcripts are kept at `subagents/<subagent-id>.jsonl` in the chat's app data.
  - Deleting the chat deletes them.
  - The main agent's context holds only the results.

### Tools and permissions for a subagent

- **Default tools:**
  - Read, search and list.
  - The browser's read actions, on its own browser tab.
  - Shell.
  - No background tasks, no `shell_unsandboxed`, and no subagent tools.
- **With `write_paths`:** it also gets the write and edit tools, which the harness limits to
  those paths. This check lives in the tool layer, so it holds on every system.
- **Shell and the sandbox:**
  - *With an enforcing sandbox* (macOS or Linux, sandbox on): the subagent's shell writes are
    confined to its `write_paths` (none when it's read-only). This is a harness wall: YOLO keeps
    it, the same way it keeps written deny rules. AGENTS.md's list of walls YOLO keeps gains
    "subagent write paths".
  - *With no enforcing sandbox* (Windows, or the sandbox turned off): shell runs as it does for
    the main agent on that system, following the approval mode. A subagent's shell command can
    then write outside its paths, and subagents still run in parallel. That is the person's
    choice and responsibility. The subagent detail shows "Shell not confined" in its permissions
    line.
- **Approvals:**
  - Approvals go through the main approval hook with the subagent's name attached.
  - Requests from parallel subagents queue up on the composer, one box at a time, oldest first.
- **Grants:**
  - "Allow in this chat" given from a subagent's request covers that subagent only. The grant key
    gains the subagent id.
  - Grants given to the main agent cover its subagents, since the main agent chose to delegate.
  - Deny rules always apply.

### The person's controls

- **Watching:** the chat card and Workspace › Tasks (below).
- **Stop:** on a subagent's Tasks row or in its detail view. Tasks stays limited to Stop and Pause.
- **Steering a subagent:**
  - The person steers through the main agent. Each subagent row in the chat card has a Steer
    icon, which adds an "About Bug hunt" chip to the main composer. The steer box no longer sits
    in Workspace.
  - The message is steered into the main agent, tagged with that subagent. Because the main
    agent is free while subagents run, it reads the note before its next request. It then
    decides whether to pass it on (`steer_subagent`), stop the subagent or start another one.

### Events and UI

- **Events:**
  - Subagent events are a separate stream keyed by `subagent_id` and the parent run id:
    `SubagentStarted {name, brief, model, write_paths, profile}`, `SubagentStatus {text}` and
    `SubagentEnded {status, summary, tool_calls, duration, cost}`.
  - The desktop saves them with the chat's run events.
  - Each subagent's own tool events go to its transcript, not to the chat.
- **The status line:**
  - Subagents' tool schemas gain an optional `status` field: one line of at most 80 characters,
    such as "Checking the CLI path for the same bug".
  - The prompt asks for it when the subagent starts a new piece of work.
  - It rides along on a tool call that happens anyway, so it costs no extra request.
  - The row shows the latest status. Until there is one, or while the running tool call is newer,
    it shows that tool call's summary ("Reading crates/core/src/runtime.rs").
- **Who changed which file:**
  - Workspace › Changes can filter by author.
  - The author comes from `write_paths` ownership: a changed file under a subagent's paths
    belongs to it, any other file to the main agent.
  - Changes outside any owned path made while subagents run without a sandbox can't be
    attributed reliably. They are labelled "Main or subagent".
- **Chat card (board 25):**
  - The header shows counts and elapsed time.
  - Each row has the status line, a Steer icon, and a link to its detail.
  - When all subagents have ended, the card folds to one result line per subagent.
- **Overview box:** a Subagents row ("1 running · 1 approval"), with a gold dot when one needs
  the person.
- **Tasks (board 26):**
  - Subagents are listed as Running or Needs attention rows.
  - The detail view shows the brief, a live transcript in the chat's style (progress sentences
    and folded tool calls), the model, a permissions line ("Read only", or "Writes
    crates/core/tests/", plus "Shell not confined" when that applies), tool calls, time and cost,
    and Stop.
- **Approvals (board 27):** the composer approval box reads "Test gaps (subagent) wants to run a
  command".

## Configuration and visibility

Captured runtime configuration, new `subagents` block (`crates/core/src/configuration.rs`):

| Field | Default | Meaning |
|---|---|---|
| `subagents.enabled` | `true` | Offer the subagent tools to the main agent |
| `subagents.max_parallel` | `3` | Subagents running at once per chat (1–6) |
| `subagents.max_turns` | `40` | Model requests per subagent |
| `subagents.model` | `null` | Model for subagents; `null` uses the chat's model |
| `subagents.allow_writes` | `true` | `false` keeps every subagent read-only, whatever `write_paths` the main agent asks for |

The project config may lower `max_parallel` or `max_turns`, turn subagents off or turn writes
off. It cannot add write permissions.

**Settings › Subagents (board 28)** sits after Web in the Settings list.

- It's the third in-app settings editor, after Settings › Browser and Settings › Web, and is built
  the same way: it edits only the `subagents` block of the user `runtime.jsonc`, field by field,
  keeping the rest of the file and its comments.
- It has four rows, with no hint text:
  - **Allow subagents:** `enabled`.
  - **Maximum subagents:** 1–6 (`max_parallel`).
  - **Subagent model:** "Chat model" or a model from the Models list (`model`).
  - **Allow file writes:** `allow_writes`.
- `max_turns` stays file-only.

**Rule changes when built:**
- AGENTS.md's Settings exception gains Subagents.
- AGENTS.md's list of walls YOLO keeps gains subagent write paths.
- `docs/configuration.md`, `docs/sandbox.md` and `docs/ui-rules.md` describe the section, the
  wall and the no-sandbox behaviour.

**Diagnostics (metadata only):**
- Subagent start and end: status, duration, tool-call count, write-path count, profile name and
  whether a sandbox enforced it.
- Approvals queued and answered.
- Never briefs, transcripts or results.

## Prompts

New files in `crates/core/builtins/prompts/`, each added to the inventory test:

- `subagent_system.md`
- `subagent_read_only_note.md`
- `subagent_write_paths_note.md`
- `subagent_status_field.md`: when to fill `status`.
- `spawn_subagents_tool.md`: when to delegate (independent parts, read-heavy work, not for
  single-file edits), plus how to use wait, steer and stop.
- `subagent_result_note.md`: the labelled result note delivered to the main agent, with its end
  reasons.
- `subagent_steer_note.md`: the person's steer about a subagent ("About Bug hunt: …").
- `subagent_denied_note.md`: what a subagent is told when a request is denied.

## Hooks

- **Tool hooks cover subagents:**
  - `BeforeTool` and `AfterTool` fire for subagents' tool calls as well as the main agent's, so a
    guard can't be bypassed by delegating.
  - The payload gains `agent: {id, name}`, absent for the main agent.
  - A blocking `BeforeTool` hook blocks a subagent's call the same way.
- **Two new canonical events, `SubagentStart` and `SubagentEnd`:**
  - They map Claude's `SubagentStart` and `SubagentStop`, which are imported instead of skipped.
  - The payload carries the subagent's id, name and profile; `SubagentEnd` adds its status.
  - Briefs and results stay out of these payloads to keep them small. Tool hooks still receive
    tool arguments, as today.
- **Main-run hooks:**
  - `RunStart` and `RunEnd` stay tied to the main run.
  - `BeforeCompaction` is emitted inside the runtime and would fire for a subagent's own
    compaction too, so its payload gains the same `agent` field. Imported hooks without
    subagent support skip calls that carry it.
- **Hooks still never grant permissions.**
- **UI:** Integrations › Installed › Hooks (mockup board 24) lists the two new events in run
  order, between "After a tool runs" and "Before context is compacted". Tool-event hook rows say
  "Main and subagents".
- `docs/plugins.md` lists the new events.

## Plugins

- **Importing `agents/*.md`** creates subagent profiles in the plugin's existing registry entry.
  The profile body becomes the instructions, placed before the brief.
- **Tool lists from frontmatter:**
  - Claude tool names are mapped to Themis tools (`Read`, `Grep` and `Glob` to read and search;
    `Bash` to shell; `Edit` and `Write` to write and edit).
  - Unmapped names are dropped, and the profile is flagged partial.
  - A tool list can only narrow tools, MCP tools included: an explicit list leaves out MCP tools
    it doesn't name.
  - It never grants permissions.
- **Writes:** a profile whose tools include write or edit still needs `write_paths` from the
  main agent before it can write. Without them it runs read-only.
- **`model` frontmatter** maps to a Models list entry when one matches. Otherwise
  `subagents.model` applies.
- **Catalog report:** "Claude subagents" is no longer a skip reason.
- **`commands/`** are separate work: they become skills in plugin PR P3.

## Order of work

1. **Read-only subagents:**
   - The four tools, async child runs with their own inboxes, the turn waiting for subagents.
   - Limits, cost accounting, transcripts and restart handling.
   - Events and the status field.
   - Tool hooks and the new hook events.
   - The chat card with Stop, the Tasks detail, the approval queue and grant scoping.
   - The overview row, configuration, Settings › Subagents (without the writes row) and
     diagnostics.
   - This is large enough to split: backend and CLI first, then the desktop UI.
2. **`write_paths`:** path validation, harness write limits, the sandbox wall, the author filter
   in Changes, and the writes row in Settings.
3. **Steering:** the Steer icon and composer chip, and `steer_subagent` delivery.
4. **Plugin `agents/` profiles.**

## Verification

- **Core:**
  - Tests for `write_paths` validation: overlap, escape, app data and `.themis/`.
  - A property test on the path validator.
  - Inbox isolation: a steer to the main agent never reaches a subagent.
  - Grant scoping.
  - The turn waiting for subagents.
- **Hooks:** a `BeforeTool` guard blocks a subagent's matching call, and `SubagentStart` and
  `SubagentEnd` fire once per subagent (shared server/CLI test).
- **Shared server/CLI end-to-end:**
  - A chat that spawns two read-only subagents and one with write paths while the main agent
    keeps working.
  - Two approvals queued from parallel subagents, one denied.
  - Stop on one subagent, a steer passed on through the main agent, and a restart mid-turn.
  - On a system with no sandbox: parallel subagents with the "Shell not confined" line.
- **Web and app:** tests for the chat card, detail, overview row and author filter, then a run
  in the real desktop app.
- **Evals:** a soak comparing main alone versus main with subagents on review-shaped tasks.
  Measure redone work, tool calls, cost and pass rate. Ship only if it is no worse on redone work
  and cost at equal quality.

## Open questions

- Whether subagents should default to a cheaper model when the chat's model is expensive. The
  Model row lets the person choose either way.
