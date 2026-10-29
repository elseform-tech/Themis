# Subagents: delegated agents you can watch

Date: 2026-10-29 · Branch: `codex/subagents-design` · Design only; nothing built yet.

Mockups: canvas https://claude.ai/artifact/Em6B9YcQir5EFWhwUN7PEJ, row "Subagents",
boards 25–28 (`AgentsChat`, `AgentsDetail`, `AgentsStates`, `AgentsSettings`).

The UI calls them **subagents** everywhere (chat card, Tasks, approvals, Settings). UI text is
short and has no hint lines.

## Why

- Themis has no way for the agent to hand part of a task to another agent. Reviews, audits and
  investigations that touch several areas run one after another in one context, and fill it.
- 33 of the 315 official catalog plugins ship Claude subagents (`agents/`), which we skip today
  (catalog check of 2026-10-29). Real agents would let those plugins work as written.
- The person needs to see what each agent is doing without the chat turning into four
  interleaved tool streams.

## Goals

1. The main agent can start agents with a written brief and get each agent's final answer back.
2. In the chat: one card per batch of agents, with one live row per agent (status, its job, its
   latest progress sentence, tool calls, time).
3. In Workspace › Tasks: the full detail of one agent (its brief, live transcript, model,
   permissions, cost) with Stop and a steer box.
4. Agents are read-only by default. They may write only to paths the main agent gives them, and
   no two agents ever own the same path.
5. No agent widens permissions. Approvals appear on the main composer, named after the agent.

## Non-goals

- Agents that start agents (depth is 1).
- Agents talking to each other. Results flow only to the main agent.
- A separate agent chat or agent editor in the UI. Agent definitions come from plugins and from
  the main agent's brief.
- Changing how goal checks work: a goal check runs after the main turn, as today.

## How it works

### The tool

`spawn_subagents` takes a list of agents and returns when all of them have ended:

```json
{"agents": [
  {"name": "Bug hunt", "brief": "Find correctness bugs in …. Do not edit files.",
   "profile": "code-reviewer", "write_paths": []},
  {"name": "Test gaps", "brief": "…", "write_paths": ["crates/core/tests/**"]}
]}
```

- `profile` is optional. It names an agent definition installed from a plugin's `agents/`; its
  instructions go before the brief, and its tool list can only narrow the agent's tools.
- `write_paths` are globs inside the project. They are rejected when they overlap between agents,
  escape the project, or reach the app data or the project's `.themis/`.
- The result is one bounded entry per agent: its final answer, or what it had so far plus why it
  ended (stopped by you, failed, denied).
- Agents run in parallel, up to `subagents.max_parallel`. Extra agents wait their turn.

### Each agent's run

- It is a child run of the current turn, using the same captured configuration and approval mode
  snapshot as its parent (configuration is captured once per run, as today).
- It starts from its system prompt, profile and brief only. It doesn't see the main chat history.
- **Tools:**
  - Read-only by default: read, search, the browser's read actions, and shell commands that the
    sandbox runs with no writes.
  - With `write_paths`, it can also write and edit files under those paths. The OS sandbox write
    allow-list for that run is exactly those paths.
- Approvals go through the parent's approval hook with the agent's name attached. "Allow in this
  chat" grants and deny rules apply as usual. YOLO keeps its written walls.
- Its transcript is saved under the chat's app data as `subagents/<subagent-id>.jsonl`. The main
  agent's context gets only the result. Agents don't add to the main context or its compaction.

### Steering

- The detail view's steer box sends the text to the main agent as a steered message tagged with
  the agent ("about Bug hunt"). The main agent decides whether to pass it on (a steer into the
  agent's run), stop the agent, or start another one.
- Stop on an agent ends that agent only. Its partial result goes back to the main agent.
- Stopping the main turn stops all its agents.

### Events and UI

- New run events carry `agent_id` and the parent run id: `AgentStarted {name, brief, model,
  write_paths}`, `AgentStatus {text}` (see the status line below), and
  `AgentEnded {status, summary, tool_calls, duration, cost}`. Each agent's tool events stream to
  its own transcript, not to the chat.
- **The live status line** is a dedicated short status from the agent, not its last sentence.
  - Agents get one extra tool, `subagent_status`, that takes a single line of at most 80 characters
    and returns nothing to the model.
  - The prompt asks the agent to call it when it starts something new (for example "Checking
    whether the CLI path has the same bug"), not on every step.
  - Until the first status, and while the latest one is older than the current tool call, the
    row shows the running tool call's summary ("Reading crates/core/src/runtime.rs").
  - A status counts as one short tool call; the soak eval checks it doesn't add noticeable cost.
- `files_changed` gains an `author` field (`main` or an agent id), so Workspace › Changes can
  filter by who wrote each file.
- **Chat card (board 25):**
  - The header shows counts and elapsed time.
  - Each row links to the agent's detail.
  - When all agents have ended, it folds to one result line per agent.
- **Overview box:** an Agents row ("2 running · 1 waits"), with a gold dot when an agent needs you.
- **Tasks (board 26):** agents are listed as Running or Needs attention rows. The detail view
  shows the brief, a live transcript in the chat's style (progress sentences, folded tool calls),
  the model, permissions, tool calls, time and cost, plus Stop and the steer box.
- **Approvals (board 27):** the composer approval box reads "Test gaps (agent) wants to run a
  command".

## Configuration and visibility

Captured runtime configuration, new `subagents` block (`crates/core/src/configuration.rs`):

| Field | Default | Meaning |
|---|---|---|
| `subagents.enabled` | `true` | Offer `spawn_subagents` to the main agent |
| `subagents.max_parallel` | `3` | Agents running at once per chat |
| `subagents.model` | `null` | Model for agents; `null` uses the chat's model |
| `subagents.allow_writes` | `true` | `false` keeps every agent read-only, whatever `write_paths` the main agent asks for |

The project config may lower `max_parallel`, turn agents off or turn writes off. It cannot add
write permissions.

**Settings › Subagents (board 28)** sits after Web in the Settings list. It's the third in-app
editor after Settings › Browser and Settings › Web, built the same way: it edits only the
`subagents` block of the user `runtime.jsonc` with field-level writes that keep the rest of the file
and its comments. It has four rows:

- **Allow subagents:** `enabled`.
- **Maximum subagents:** 1–6 (`max_parallel`).
- **Subagent model:** "Chat model" or a model from the Models list (`model`).
- **Allow file writes:** `allow_writes`.

The section has no hint text.

This changes a rule the person set: when it's built, the Settings exception in AGENTS.md
("except Settings → Browser and Settings → Web…") gains Subagents, and `docs/configuration.md` and
`docs/ui-rules.md` describe the section.

Diagnostics (metadata only) cover agent start and end: status, duration, tool-call count, the
write-path count and the profile name. They never include briefs, transcripts or results.

## Prompts

New files in `crates/core/builtins/prompts/`, each added to the inventory test:

- `subagent_system.md`
- `subagent_read_only_note.md`
- `subagent_write_paths_note.md`
- `subagent_status_tool.md` (when to report a status: at the start of each new piece of work, in
  one plain line)
- `spawn_subagents_tool.md` (when to delegate: independent parts, read-heavy work, and not for
  single-file edits)

## Hooks

- `BeforeTool` and `AfterTool` fire for subagents' tool calls as well as the main agent's, so a
  guard hook can't be bypassed by delegating. The payload gains `agent: {id, name}` (absent for
  the main agent). A blocking `BeforeTool` hook blocks the subagent's call the same way, and the
  subagent sees the denial like any other.
- Two new canonical events, `SubagentStart` and `SubagentEnd`. They map Claude's `SubagentStart`
  and `SubagentStop`, which are imported instead of skipped. The payload carries the subagent's
  id, name and profile, and for `SubagentEnd` its status. It never carries the brief or the
  result, in line with the rule against logging prompts and tool payloads.
- `RunStart`, `RunEnd` and `BeforeCompaction` stay tied to the main run, so existing hooks don't
  start firing once per subagent.
- Hooks still never grant permissions.
- Integrations › Installed › Hooks (mockup board 24) lists the two new events in run order between
  "After a tool runs" and "Before context is compacted". Tool-event hook rows say "Main and
  subagents".
- `docs/plugins.md` lists the new events.

## Plugins

- Importing `agents/*.md` creates agent profiles in the plugin's existing registry entry. The
  profile's instructions are the body. Claude tool names in its frontmatter can only narrow
  tools, and they never grant permissions.
- The compatibility report drops "Claude subagents" as a skip reason.
- `commands/` and `agents/` are separate work: commands become skills (catalog Step 2).

## Order of work

1. Read-only agents: the tool, child runs, events and the status line, tool hooks firing for
   subagents plus the two new hook events, the chat card, the Tasks
   detail with Stop, named approvals, the overview row, the configuration, Settings › Subagents
   (without the writes row) and diagnostics.
2. `write_paths`: overlap checks, the sandbox allow-list, the Changes author filter, and the
   writes row in Settings › Subagents.
3. The steer box through the main agent.
4. Plugin `agents/` profiles.

## Verification

- Core: tests for `write_paths` overlap, escape, app-data and `.themis` rejection, plus a property
  test on the glob validator.
- Hooks: a `BeforeTool` guard blocks a subagent's matching call, and `SubagentStart` and
  `SubagentEnd` fire once per subagent with metadata-only payloads (shared server/CLI test).
- Shared server/CLI end-to-end: a chat that spawns two read-only agents, one write-path agent,
  a denied approval, Stop on one agent, and a steer about an agent.
- Web: tests for the chat card, the detail view, the overview row and the author filter. Then a
  run in the real desktop app.
- Evals: a soak comparing main versus agents on review-shaped tasks. Measure redone work, tool
  calls, cost and pass rate. Ship only if it is no worse on redone work and cost at equal quality.

## Open questions

- Whether agents should default to a cheaper model when the chat's model is expensive.
