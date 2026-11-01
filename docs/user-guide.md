# Themis user guide

Themis is a desktop coding agent. You open a project, start a thread, describe
the change, and the agent works in the selected project checkout. Threads share
that checkout, including existing uncommitted files. Inspect changes with your
Git client; open a separate worktree as a project when you want isolation.

## Install

1. Get the macOS `.dmg` (built from this repo — see
   [maintainer-distribution](maintainer-distribution.md)) and open it.
2. Drag **Themis** into Applications and launch it.

Themis keeps its data outside your projects: settings in `settings.json` and
conversation history in `sessions.sqlite3` under the Tauri app-data directory.
Entered API keys are stored in macOS Keychain; Go can also read `OPENCODE_KEY`
from its environment.

## Getting started

Themis opens directly into the workspace. Choose **Create project**, enter a
name, and start chatting in the new thread. All new projects live under the fixed
`~/ThemisOS/Projects` root. Existing folders are never overwritten.

New projects get a Git repository and initial empty commit so each thread can
work in isolation. Open an existing folder through the project actions menu.

## Secrets

Don't paste keys into a message: it goes to the model provider and stays in the chat. If you do, a line above the composer says so before you send, and the agent reminds you to revoke and rotate it. Keep them here instead, or in an environment variable. In Custom, the agent asks before reading or writing a project's `.env` or a private key, and before staging one in Git; it also keeps `.gitignore` covering them ([details and limits](sandbox.md#sensitive-project-files)).

API keys and tokens are **secrets**, kept in **Settings › Secrets** by name. OpenCode Go
reads the secret chosen under Settings → Models → Providers → **Secret** (`OPENCODE_KEY`
unless you pick another); the choice is saved and applies every time Themis opens. Set
its value there (**Set OPENCODE_KEY**) or in Settings › Secrets, or launch Themis with an
environment variable of that name. MCP servers read secrets as `${NAME}`, so
`${OPENCODE_KEY}` works there too. A key saved by an earlier version is copied to
`OPENCODE_KEY` the first time it is read.
Entered secrets remain in macOS Keychain across app restarts. They are not saved
in the conversation database, settings files, or browser storage.
Forgetting a saved key does not unset an environment key. A Finder launch
may not inherit environment variables exported by your shell.

**Key available** reports presence only. Settings loads the Go model catalog
automatically and lets you select a default model; **Refresh models** reloads it.
New Go threads use `muse-spark-1.3-contributor` by default. The model pill at
the top of each thread lists models available to the connected Go account. Its
effort slider uses the selected model's catalog levels and stays visible when no
levels are listed.

## Projects, threads, runs

- **Project actions → Open existing folder** opens an existing project. In Custom mode, non-Git
  folders disable sending; YOLO allows runs (see [sandbox](sandbox.md)).
- The **+** beside a project starts a thread with your default provider and model.
  Threads share the selected project checkout; they do not automatically create
  separate branches or worktrees.
- Type in the thread view and send. The run streams assistant text and a tool
  trace; the selected permission policy governs tool approvals (see below).
- **⌘/Ctrl+K** opens the command palette: jump to threads and projects, or run
  actions (new thread, open project, toggle theme, open settings). **Esc** closes the topmost layer.
- The sidebar shows the chosen project's chats only. Switch projects, create
  or open one, or rename any project with the pencil icon on its row in the project switcher at the top.
  Hover or focus a chat and open its ⋮ menu to rename or remove it.
  Open the model pill at the top of the thread to choose a Go model and, when
  that model offers effort levels, the effort slider. A new chat shows a greeting,
  a centred composer and starter suggestions. The composer's **+** menu attaches
  files or inserts `/` (skill) and `@` (plugin). The agent writes one short line when it
  starts a new step or finds something that matters, never one per tool call; those lines
  stay in the conversation, styled like its answers. The tool calls between two of them
  fold into one row that says what they did, such as **Read 4 files · Ran python3
  check.py**, open while one of them runs; expand it, then a tool call, to inspect its
  input and output. Every turn that worked with tools ends with a message saying what it
  did: when the agent stops on a short progress line instead ("Continuing through the
  files."), Themis asks it once to continue or sum up. A reply that only announces its
  next step ("Let me check the console next.") or writes a tool call out as text also
  gets one more try before the turn ends; an offer or a question to you ends it.
  Completed replies offer
  **Copy** beside the model and the run's duration. Public text streams as it
  arrives; the model's private reasoning is never shown.
  Thinking/using-tool status and elapsed time remain visible during gaps. Scrolled up to read, the status shrinks to "…" so it covers nothing.
- **Stop** requests cancellation at the next safe boundary. A request or tool
  already executing finishes first; completed edits remain available to review.
  Pressing **Esc twice** does the same: after the first press, "Press Esc again to stop"
  shows beside the stop button for a second and a half. An Esc that closes a menu,
  panel or suggestion list only closes it.
- **While a run is going** you can keep typing: **Send** queues the message
  above the composer, and queued messages go out by themselves, oldest first,
  when the run ends. Each one has three icons: **Steer** sends it into the
  running run now (the agent reads it before its next step, without stopping;
  a message with attached files waits for the next turn instead), **Edit**
  changes it while it waits (it is skipped while you edit), and **Remove**
  takes it out of the queue. A steered message shows in the chat as soon as you
  steer it, and the agent says in one short sentence how it will handle it before
  carrying on; when you steer in two or more, the agent is reminded
  of all of them before it finishes. If the agent is waiting on an approval when
  you steer, you don't need to answer it first: the box goes away, the action is
  denied this once so the agent reads your message, and it can ask again later.
  Your chat permissions don't change, and a message left in the queue leaves the
  box waiting. A steer the run never got to read leaves the chat, returns to the
  front of the queue and is sent as the next message. A message that cannot be sent (a bare `/goal`, a broken
  project configuration) stays in the queue marked **Needs an edit**, with the
  reason as its tooltip and in the chat; edit it (or save it unchanged) to send
  it again. Queued messages survive a restart. A chat holds at most 50 queued
  messages of up to 100,000 characters each.
- **Changing the model or effort during a run** applies from the next turn:
  the running turn finishes on the model it started with.
  Asking in the chat to stop or pause the goal pauses it; the agent can't pause a
  goal nobody asked to stop.

### Questions from the agent

When the agent needs your input, it asks without stopping its work. The
question shows on the composer with 2–4 options, one per line, its recommended one
marked **Recommended**, or, for an open question such as a name, its
**Suggested** answer. Each option has one short line on its tradeoff, and the
recommended one says why. The agent carries on with that answer until you click
another option or write your own.

- **One box at a time.** The composer shows one approval or question at a time,
  approvals first; what waits behind it is a count ("1 question next"). Once you
  answer, the question leaves the composer and stays in its turn, where it was
  asked, as one line with your answer. **Skip** (or five minutes without an
  answer) also moves it back into its turn as an **Open question** line; click
  that line to put it on the composer again and answer whenever you like.
- **Fold to icons** (the chevron) turns the goal, the approval and the questions
  into one row of small pills: the goal, the waiting
  command, the number of questions. Click any pill to unfold. The fold is
  remembered.
- Your answer reaches the agent right away when it is working (read before its
  next step), or goes out as the next message when it is not. If the answer
  differs from what it assumed, it fixes that work first.
- Only a question the agent cannot recommend any answer to, and that blocks all
  further work (the name of your own key, say), ends its turn; it never shrinks, and in
  goal mode the goal waits for that answer only.
- A goal can be reached with questions still open: the agent's message says
  which ones and what it recommended. Questions stay with the chat across restarts;
  from the CLI, `themis call list_questions` and `answer_question` do the same.

### Subagents

For work that splits into independent parts, such as reviewing several areas or
searching a large codebase, the agent can start subagents. Each one works on its
own brief while the agent carries on, and its result reaches the agent before its
next step. The agent's turn ends only after every subagent it started has
reported back.

- **Checked, not trusted.** The agent treats what a subagent reports as claims: before relying on a finding it reads the code itself (you see "Read …" right after the subagents), and its reply opens with what it checked and what it confirmed or rejected.
- **Read-only.** Subagents read, list and search files and run shell commands, but
  do not change the project. Where the OS sandbox is available their commands may
  write only temporary folders; without it nothing confines them beyond your
  approval rules.
- **Approvals.** A subagent's approval names it. **Allow in this chat** on its
  request covers only that subagent; your chat's existing grants and deny rules
  apply to it as usual.
- **Stop.** Stop on the chat stops its subagents too. The agent can stop one
  subagent; that subagent's partial result still reaches it.
- **Choosing models.** Name them in your request: "use glm-5 for the review and haiku for the docs". The agent may use the **Default model** (the chat's model unless you set one) and up to three **Allowed models** you list in Settings › Subagents; it matches your words to them and says so when you name one that isn't listed. If a subagent's model can't start, or its first request fails (a used-up quota, say), it falls back to the Default model, then the chat's model; Workspace › Tasks shows "claude-haiku → glm-5".
- **Limits.** At most three run at once by default (`subagents.max_parallel`);
  more wait their turn. Each has 40 model requests (`subagents.max_turns`).
  `subagents.model` picks a different model for them, and `subagents.enabled:
  false` turns them off; see [configuration](configuration.md).
- **Watching them.** A reply's subagents show in the chat as one small stack of lettered circles, each ringed while it works. Hover one for its name, or click it to open its steps in Workspace › Tasks, where **Stop** ends just that subagent; the chat overview shows how many run and whether one waits for your approval. Settings › Subagents turns them off, sets how many run at once, and sets their Default and Allowed models.
- Each subagent's steps are saved beside the chat and removed with it. From the
  CLI, `themis call list_subagents '{"threadId": "…"}'` lists a chat's subagents.

### Goals

A goal is one sentence the agent keeps working toward, turn after turn, until it
is reached; for example `/goal optimize this app to speed up p95`.

- **Start one** with `/goal <sentence>` (the first entry of the `/` menu, at the
  start of a message). The command shows as a **Goal** sticker in the composer
  and in the sent message. It starts right away. When you ask for an ongoing outcome in
  plain words, the agent may ask once whether to make it a goal; if you agree, it
  sets the goal itself.
- **The goal row** at the top of the composer shows the sentence (click it to
  read it in full) and the goal's total elapsed
  time across all its turns (time spent paused, waiting or with Themis closed does
  not count). Its icons are
  **Edit** (applies from the next turn; the current turn keeps running),
  **Pause** (after this turn) or **Resume**, and **More**, which holds **Remove**
  (the current turn finishes; no further turns or checks). A message sent while a goal is paused, or after it ended, gets an ordinary answer: the agent is
  told not to pick the goal's work back up unless you ask. You can also ask in the
  chat to pause or remove the goal, and the agent does it; it can't do either unless
  you asked. In a narrow chat the sentence sits on its own line below the icons.
- **After each turn** (a goal turn, or the run of a message you sent while the
  goal is open) a separate check (on the chat's model, or the one
  `goal.check_model` names in runtime.jsonc), with a fresh context, reads the goal, the
  turn's answer and the commands it ran, and decides: reached or not yet (the next
  turn starts by itself, told why). With a number in
  the goal, that number is the bar; without one, the check decides when further
  gains are not worth it and says why.
  The check reads the commands of the turn it judges (long outputs keep their
  start and end), never the agent's claims, and keeps a list of what the goal asks
  for and what is proven, so proof carries from turn to turn; an item goes back to
  open when later work could have broken it. The overview box counts them ("2 of 5
  complete"), and Workspace › Goal lists them as Pending and Complete, with what showed each. A turn's
  replies dim when the check rejects them, so a claim the check didn't accept
  doesn't read as a result. A browser script that changes the page instead of using it (setting a value, clicking from code) rules out reaching the goal until the page is reloaded and the whole thing shown again with real input. The check also says whether the turn moved the work forward (a turn
  that changed files always did); after three turns that only re-check, the next
  one is told to step back and try a different approach.
- **What you add joins the goal.** A request you send while the goal is open
  (steered in, queued or typed) is kept with the goal, and the check reaches the
  goal only when those requests are done too. Answers to the agent's questions are
  not requests. Up to 20 are kept (the oldest drop off); `/goal` again starts a
  fresh list. `themis call get_goal` shows them under `added`.
- **The agent says where it stands.** There is no status box after a turn: each
  goal turn opens with the agent saying, in its own words, where the goal stands
  and what it will do next. Once the check finds the goal reached, one short
  turn has the agent tell you so and what shows it, unless the answer it checked
  already said so (and no question of the agent's is still open).
- **It keeps going and never gives up.** There is no time, turn, cost or stall
  limit. Your messages are handled first, then the goal continues. Failed turns are
  retried with growing waits (up to 15 minutes) however often they fail; **Stop**
  ends only the current turn; a closed app resumes running goals when it opens
  again. A goal pauses only when it is reached, waiting for your answer (a blocking
  question: the agent can do nothing more toward the goal without it; answering
  it resumes the goal, see **Questions from the agent**), or out of model
  quota (it retries every 15 minutes; switching the model resumes it at once). A
  goal saved as stuck by an earlier version waits for Resume.
- **Notifications** (macOS): while Themis is in the background, a desktop
  notification names the chat when its goal is reached, waits for your
  answer, or runs out of model quota (once per quota pause, not at every retry).
  Pausing a goal yourself never notifies. Clicking one brings Themis forward and
  opens the chat. Turn them off with **Notify when a goal finishes or pauses** in
  Settings › General › Notifications. macOS asks for permission the first time one
  is needed or when you turn the switch on; if notifications are off for Themis in
  System Settings › Notifications, the switch says so.
- A goal never grants permissions: approvals and the sandbox apply as usual, and
  while an approval waits the goal waits too. A reached goal leaves the composer;
  the agent's message saying so stays in the conversation. Sending a new message after a goal
  is reached clears it; `/goal` again replaces the current goal. When the agent
  rewords an open goal (for example to fold in what you added), the goal goes on:
  its elapsed time, turn count and added requests carry over.
- **⌘/Ctrl+B** collapses or pins the sidebar. Hovering at the left edge reveals
  an overlay after 300 ms. It closes 700 ms after leaving, unless focus is inside.
  Hover reveal can be disabled in Appearance. A visible button always reopens it.
- Threads appear under their project. Rename long titles from the thread's ⋮ menu.
  Drafts, selection and scroll positions restore locally. Conversation turns and
  completed tool events restore from SQLite.
- Enter or ⌘/Ctrl+Enter sends; Shift+Enter inserts a newline.
  ⌘/Ctrl+Shift+O or **New chat** starts a new thread.
- The ring beside send is the context meter: estimated use of the model's usable
  context. Click it for the breakdown (tool activity, messages, attachments,
  checkpoint and task ledger, instructions), the reserves held outside it and the
  tidy-up and auto-compact thresholds. Near auto-compact during a run it turns
  warm and a one-time heads-up appears; afterwards a **Context compacted** divider
  shows the before → after usage. Nothing is deleted: originals stay on disk.
- **The overview box** sits over the top right of a chat while Workspace is closed:
  a live preview of the chat's browser (the page keeps its size; only the preview is
  small), one row each for changed files (+/− lines), tasks and
  queued messages, and the latest artifact, as icons with short
  values (hover a row for its name). Click a row to open Workspace on that tab,
  or the Workspace icon to open it as you left it; closing Workspace brings the
  box back. The chevron folds it to icons only, as does a narrow chat. Rows with
  nothing to show are left out, and a chat with nothing to show has no box.
- **Workspace** in a thread's header opens a monitor of the project:
  **Changes** (readable changed files and their wrapped, highlighted diff,
  updating live while the agent works; binary and generated files are hidden; a
  change too large to show whole, such as a downloaded dataset, shows its first
  2,000 lines with a note), **Files**, **Branches**, **Pull requests** (needs
  the GitHub CLI signed in) and **Tasks**. Tasks lists this chat's running work and
  the automations that continue it and programs the agent started in the
  background (dev servers, watchers), with **Stop** (the current run, or one
  background program, with its latest output and a read-only **Full log**) and
  **Pause** (turn an automation off); stopping never undoes work already done.
  Background programs end when you delete the chat or quit Themis (the app server
  it started quits with it);
  stopping a run does not end them, but it does end a command the run was waiting for. A small
  stack of icons beside the Workspace button shows the same tasks at a glance and
  opens that tab. Drag its edge to resize. It never changes files or
  runs Git actions. The composer shows the project folder and current branch.

Recent projects are remembered (max 10) and reopened automatically on launch;
threads that were still running at shutdown are marked **recovered**. The last
saved events remain visible, but the agent does not automatically rerun an
interrupted tool or request. Review any changes before sending a follow-up.

## Thread changes and removal

Agent edits land in the shared project checkout. The chat has no Merge control;
use your Git client to inspect and commit changes. Removing a thread deletes its
conversation records, not the project files or completed edits.

## Approvals

The composer shield selects **Custom** or **YOLO** for the thread. Custom is
initially selected and uses ordered approval rules from advanced file configuration.
Its default policy allows reads and asks before other tools. A matching deny
rule rejects the action; a matching allow rule proceeds. Projects can add
restrictions but cannot grant access.

When a rule asks, an **Approval needed** box appears on that chat's composer,
before any question (see **Questions from the agent**). Only that action waits:
the chat stays readable. Once you decide, the box goes away and leaves nothing in
the chat (the call itself still shows in its step list). Another chat that waits for
you shows **Needs approval** in the sidebar. The box shows the tool, summary,
and risk, with the choices in one row:

- **Allow once (1)** — allow this call only.
- **Allow in this chat (2)** — allow the same command family (for example `cargo test`
  with any further arguments, every tool of one MCP server, or reading pages from one
  site) now and in every later run of this chat, including its automations. Other chats
  still ask.
- **Deny (3)** — refuse; the agent sees the denial and adapts or stops.

Destructive actions (force pushes, discarding changes, deleting files and similar) always
ask: their dialog shows only **Allow once (1)** and **Deny (3)**. Deny rules always win over
chat grants. Grants live in the chat's own file, `chat_grants/<chat id>.json` in the app data
folder; to undo one, remove its entry (or delete the file) or start a new chat. Enabling an MCP
connection is the permission to start it, so no start prompt appears.

Approval timeouts deny pending requests and remove the box with a short note. Reopening the app restores requests still pending in the running backend. With no explicit approval
configuration, **Ask before read tools** still controls reads. **YOLO** means no
approval prompts and no sandbox: it bypasses harness approval and tool
restrictions, including the filesystem root and Git allowlist, and its commands
may write anywhere, read secret folders and use the network. Only Themis' own
settings (its app data and the project's `.themis/` folder) stay read-only, and
the agent is told to avoid destructive actions you did not ask for. Blocks you
wrote still hold: a `deny` rule, `sandbox.network: "block"` and
`allow_keychain: false`; the shield's panel lists them. The shield
shows **YOLO** in the warning colour. Mode and
configuration changes apply to the next run; active runs keep their captured
policy. See [configuration](configuration.md) for examples and
[sandbox](sandbox.md) for the enforcement boundary.

### Web search and pages

Agents read web pages with `fetch_url` and search with `web_search`. Search uses a
SearXNG instance you run and name in the user `runtime.jsonc`
(`"web": {"search_url": "http://127.0.0.1:8888"}`); without it only page reading is
offered. Both ask per site in Custom, and **Allow in this chat** covers that site.
Page text and results are treated as untrusted. See [web tools](web-tools.md).

Agents can also drive a built-in browser (its own headless Chromium engine, downloaded
the first time it is used, or a Chrome, Chromium or Edge you choose in Settings › Browser,
with its own profile per project). While the agent browses, the chat's overview box
shows a live preview of the page with what the agent is doing; click it to open
**Workspace → Browser**, where the line above the page says the same. (To have
Workspace open by itself instead, turn on **Open Workspace when the agent starts
browsing** in Settings › Browser.) You can click, type or enter an
address at the same time; the agent keeps working. The speaker button turns the
page's sound on (it starts off). **DevTools** opens Chrome's
DevTools for the page. Browser actions ask per site, like page reads. Start a
message with **/browser-use** to have the agent do a task in the browser (it
follows built-in guidance: act on snapshot refs, check console errors, never enter
secrets you did not give it, ask before anything irreversible, and attach a
screenshot or recording when useful).
**Capture** adds a screenshot of the page to your message and **Record** a video
(a red line and a red header pill show while anything records). Agents can take
screenshots and recordings too and show them, or other images, videos and PDFs from
the project, under their reply; click one to open the viewer.

### The sandbox

Commands the agent runs (shell commands, background tasks, the git tool and
automation checks) run inside an operating-system sandbox in Custom (in YOLO it
only keeps Themis' own settings read-only): Seatbelt on macOS, bubblewrap or
Landlock on Linux; Windows has none yet.
They can write only inside the project, temporary folders and any folders you
list in `sandbox.writable_paths`; they cannot read secret folders such as
`~/.ssh` or `~/.aws`, and they can never change Themis' own settings, chat grants
or the project's `.themis/` folder. Network access is allowed unless you set
`"network": "block"`.

When the sandbox blocks a command in **Custom**, a dialog asks **Run outside the
sandbox?** with **Allow once (1)**, **Allow in this chat (2)** and **Deny (3)**;
allowing runs the same command again without the sandbox. In **YOLO** nothing is
asked and the agent is told the sandbox blocked it. The sandbox is configured
only in the `sandbox` block of your `runtime.jsonc` (projects can only tighten
it). **Settings → Permissions** shows whether that file was read (Loaded, Not
created, or Invalid with its line) and opens it with the editor you pick under
**Open with** (the system text editor, or VS Code, Cursor, Windsurf, Antigravity,
Zed, Sublime Text, BBEdit or Xcode when installed), creating a commented-out
starter the first time; `themis permissions`
prints the full effective view. See [sandbox](sandbox.md) for details and limits.

## Skills

The **Skills** tab holds reusable instruction bundles: a name, description,
instructions injected into runs, an optional tool allowlist, and helper
scripts. Attach skills to a thread to scope what it may use. The list includes plugin-bundled skills and skills discovered from local folders. Browse new packages in **Plugins → Discover**; Skills has no separate discovery catalog. Standalone imports remain available through chat or the CLI.

Skill instructions can contain up to 256 KiB of UTF-8 text without truncation. File reads are paged at 8,000 content characters; the agent must follow `next_offset` to read longer instructions. Large tool results are saved to disk and represented by a small receipt, so the agent can extract the relevant facts. An oversized tool result normally continues the run. Storage failures or repeated recovery can stop the current run; the chat remains available. Oversized tool arguments are rejected before execution, without partial writes. See [context limits and recovery](context-and-attachments.md).

**Recalling older details.** After a chat is compacted, details that only appear in earlier messages, long tool output or attached files are not in the summary. The agent has a read-only `recall` tool that searches this chat's saved originals (never other chats) and returns short excerpts; it needs no approval. Answers tell you what was said in plain words ("you mentioned earlier…"), never internal archive paths or record numbers. Excerpts are treated as untrusted data, not instructions. If an answer needs a detail it could not find, ask for it again or attach the file. Details are in [context limits and recovery](context-and-attachments.md#the-recall-tool).

- An empty allowlist means "all tools".
- Unknown tool names are rejected. The valid names are exactly:
  `list_dir`, `read_file`, `write_file`, `copy_file`, `move_file`,
  `delete_file`, `create_dir`, `search_file`, `shell`, `apply_patch`, `git`,
  `fetch_url`, `web_search` and `browser` (every `browser_*` tool).

The file tools accept an absolute path when it lies inside the project (it is
used as the matching project-relative path); paths outside the project stay
refused. `read_file` on an ordinary file ignores the archive-only
`jsonl_record`/`json_pointer` options and says so. `shell` repairs the two
command shapes models often send (the program repeated as the first argument,
or an empty `command` with the program in `args`) before approval, so the
approval dialog shows exactly what runs. When an action is refused, the agent is
told not to reach the same result another way and to ask you instead.

## Automations and the review queue

The **Automations** tab schedules recurring work with a name, interval in
minutes (≥ 1), task prompt, and enabled flag. Choose where each run goes:

- **Continue an existing thread**: select a thread. Each run uses that thread's
  current model, reasoning effort, and skills, and adds to its history.
- **Create a new thread each run**: select an existing project or create one,
  then choose a Go catalog model, its available reasoning effort, and skills.
  Older automations keep this behavior.

- **Watches.** Ask the chat agent in plain words ("watch status.txt every 2
  minutes and tell me when it says DONE, then stop watching"). It creates a short
  automation on that chat without asking, checks cheaply (a command that costs no
  model call) and uses the model only when something changed. Watches expire after
  2 hours by default (at most 24 hours), stop after 60 runs by default and after
  three failures in a row, and the agent ends them when the goal is met. The chat's
  Tasks tab shows the interval, goal and expiry, and why a finished one ended.
  Stop or Pause them there at any time. See [automations](automations.md).
- The scheduler only runs **while the app is open**, and only when
  **automations_enabled** is on in Settings (the global kill-switch).
- **Run now** fires an automation immediately regardless of the kill-switch.
- Each completed run lands in the **Review queue**. Open an item to inspect it,
  then **Continue** (opens the run's thread so you can follow up) or
  **Dismiss**. Items stay `pending` until you act.

## Providers

OpenCode Go is built in. You can add OpenAI-compatible providers (any service or
local server that speaks Chat Completions) under `"providers"` in the global
`runtime.jsonc`, which Settings › Permissions opens in your editor:

```jsonc
"providers": [
  { "id": "local", "name": "Local models", "base_url": "http://127.0.0.1:8080/v1",
    "secret": "${LOCAL_KEY}" }
]
```

Put the key in Settings › Secrets under the name the provider's `secret` gives
(never in the file). Without `"models"`, Themis reads the provider's
`<base_url>/models` list; list them yourself to add context and output limits and
prices. Settings › Models › Providers shows each provider with its secret and
status (Connected, Key set, Key rejected, Couldn't reach it, Not set), and the
default-model picker groups the models of every connected provider by provider.
A chat on one of these models is named `<id>/<model>` and needs no OpenCode Go
key. Only your own `runtime.jsonc` can add providers; a project's
`.themis/config.jsonc` cannot. See [configuration](configuration.md#model-providers-and-per-model-overrides)
for every field and limit, and for the `"models"` block that caps a model's
context or output and sets its default effort or thinking.

Set the default model in Settings; choose a model per thread in the composer.

Model resolution order for a run:

1. The thread's explicit model (thread header or `default_model` setting).
2. The Go environment fallback: `THEMIS_GO_MODEL`.
3. The built-in default: `muse-spark-1.3-contributor`.

Go uses the OpenCode Go API at `https://opencode.ai/inference/go/`, which serves
each model family over its own protocol: Chat Completions and Responses under
`openai/v1`, Messages under `anthropic/v1` and Gemini under `google/v1beta`.
Themis reads each model's protocol and endpoint from the public models.dev
catalog, so a model Go adds later is called correctly without an app update.
Settings, the composer, and the automation editor select from current Go model
IDs. Their effort controls follow the selected model's catalog levels. A model
whose protocol Themis does not support yet shows as "(unavailable)" with the
reason as its tooltip and cannot be chosen. If your saved default model is no
longer in Go's catalog, new chats use `muse-spark-1.3-contributor`, or the first
usable model when that one is gone too.

## Settings reference

Settings has General, Appearance, Models, Usage, Secrets, Permissions, Browser, Web, and
Advanced sections. While Settings is open they take over the whole left sidebar (the icon
rail is hidden) and **Back to app** at the top returns to the chat. The sidebar toggle in
the header (⌘B) folds the list to icons, as it does the rail.
**Usage** shows the tokens Themis used for the last 5 hours, this week (from Monday) and
this month: input with its cached share, output with its reasoning, requests and, when
the public models.dev catalog lists prices, an estimated "≈ List price" (a "+" means some
requests had no published price). Below are the last 30 days as daily bars by model
(hover or focus a bar for its numbers; free models are marked Free) and the chats that
used most. Only requests this Themis made are counted, so the provider's own dashboard
can show more, and the estimate is not your bill: OpenCode Go plans charge a flat fee with
their own usage limits, which Themis does not show. The context panel in the composer adds
one line with the open chat's total so far. Each request is kept for 35 days, then as daily
totals for 400 days, in `usage.sqlite3` in the app data folder (numbers and ids only).
`themis usage` prints the same totals.
**Browser** sets what the agent may do in its built-in browser (Allow, Ask or Block for
browsing, downloads and debugging, with per-site exceptions), Developer mode, the downloads
folder, and the browser's history and data; see [web tools](web-tools.md#permissions-settings--browser).
It saves to the `browser` block of your `runtime.jsonc`.
**General › Notifications** holds **Sound when a reply finishes**, **Haptics** (macOS) and
**Notify when a goal finishes or pauses** (macOS; see [Goals](#goals)).
Ordinary preferences save on change; text inputs save when focus leaves the field.
After each save the pane heading shows **Saved** for a few seconds; in Models, Permissions,
Browser, Web and Advanced it adds *applies from each chat's next message*, because a reply
already running keeps the settings it started with. Nothing needs a restart.
Errors appear inline, and rejected changes leave the previous preferences intact.
Permissions sets the mode new chats start in (Custom or YOLO; each chat can still switch in
its composer) and the approval timeout, lists your Custom rules from `runtime.jsonc`
read-only (first match wins, then the default for anything else), and shows whether that
file was read, with **Open with** to edit it.

**Your `runtime.jsonc`.** A new file is short: common settings grouped by purpose, each a
whole line starting with `// "` under a plain note. Delete the `//` to turn one on and edit
its value; plain `//` notes are never settings. Every option, its allowed values and its
default are explained in `runtime.reference.jsonc` beside it, which Themis keeps current
and never reads. A key may appear only once (two `"sandbox"` lines are refused with the
line number; put both settings in one block). There is nothing to restart: each chat reads
the file when its next message starts, and a reply already running keeps the settings it
began with. Settings › Permissions shows whether the file was read and any problem in it.

Preferences persisted to `settings.json`:

| Key | Meaning | Default |
| --- | --- | --- |
| `theme` | Dark only; legacy values normalize to `dark` | `dark` |
| `default_provider` | Provider for new threads | `go` |
| `default_model` | Model for new threads (`""` = provider default) | `""` |
| `max_total_turns` | Steps per reply (1–2,000; Settings → Advanced → Steps per reply) | `200` |
| `context_token_cap` | Working context: `0` = Auto (the model's window, up to 200,000), or a cap of 200,000–1,000,000 tokens (Settings → Advanced → Working memory; a model with a smaller window uses its whole window) | `0` |
| `approval_timeout_seconds` | Seconds before an unanswered approval is denied (30–600; Settings → Permissions) | `300` |
| `default_approval_mode` | Mode a new chat starts in: `custom` or `yolo` | `custom` |
| `confirm_reads` | Require approval for read tools | `false` |
| `recent_roots` | Recently opened projects, most recent first (max 10, managed automatically) | `[]` |
| `concurrency_limit` | Max parallel runs across all threads (1–16); sends past the limit are rejected | `3` |
| `automations_enabled` | Global automation kill-switch | `true` |
| `completion_sound` | Play a click when a reply you started finishes | `false` |
| `completion_haptic` | Trackpad feedback when a reply you started finishes (macOS) | `false` |
| `goal_notifications` | Desktop notification when a goal is reached or pauses while Themis is in the background (macOS) | `true` |
| `projects_directory` | Fixed parent folder for new projects | `~/ThemisOS/Projects` |
| `text_size` | Conversation text size (12–18 px) | `13` |
| `sidebar_hover` | Reveal collapsed navigation on edge hover | `true` |
| `check_sources_for_updates` | Look for plugin updates in sources not checked today when the app opens | `true` |
| `config_editor` | Editor Settings → Permissions opens `runtime.jsonc` with (`""` = system text editor; or `vscode`, `cursor`, `windsurf`, `antigravity`, `zed`, `sublime`, `bbedit`, `xcode`) | `""` |

Appearance and model defaults each have their own reset action.

## Updates

Settings → General → **Updates & diagnostics** → **Check for updates**. Checks are manual only —
Themis never checks on boot or in the background. The status line reports one
of:

- **up to date** — you're current.
- **update available** — shows the version and release notes.
- **disabled** — no update endpoint is configured in this build; the message
  says why.
- **check failed** — the message carries the error (e.g. no network).

## Diagnostics

The top right of the window shows Themis's own resource use: CPU (its share
of all cores), memory and the window's frame rate. A value turns amber when CPU
passes half the machine, when memory has kept growing for half an hour, or when
the window draws under 30 frames a second. Click it for the split between the
app, the built-in browser and tools such as dev servers.

The desktop keeps configuration files and logs out of normal user flows.
Advanced users can inspect local rotating JSONL logs and configure User or
Project runtime JSONC through files and the shared CLI/API. Global integration
definitions use the revisioned app-data registry; project imports use
`.themis/plugins/`. See the [configuration guide](configuration.md) for paths,
schema, limits, and CLI examples.

## FAQ

**What does Themis cost?**
Themis itself makes no paid calls. OpenCode Go billing applies under your own
key. Keep Steps per reply low when testing long tasks.

**What are the limits?**
Steps per reply caps the steps one reply may take; Working memory caps the context the model keeps in view (Auto follows the model); Chats at once (`concurrency_limit`) caps how many chats work at the same time;
approval dialogs deny after the configured timeout (default 5 minutes); each shell invocation captures at most
32 KiB of output; provider HTTP calls time out after 120 s.

**Where are my keys?**
Entered keys stay in macOS Keychain until you forget them. Go also reads
`OPENCODE_KEY` from the environment. Keys are not written to SQLite or settings.

**Why is my project read-only?**
Custom mode disables the desktop composer for non-Git folders. The shield
remains available; selecting YOLO enables sending (with no approvals and no
sandbox walls; only Themis' own settings stay protected).
See [sandbox](sandbox.md) for backend policy and the limits of tool-level
restrictions.

**How do I integrate changes?**
Changes already affect the selected project checkout. Use your Git client to
inspect and commit them. The chat has no Merge control.

**A run failed with "send rejected / runs already active"?**
You hit `concurrency_limit`. Wait for a run to finish or raise the limit in
Settings.

**The app forgot my projects / acts strangely after an upgrade?**
Settings load from `settings.json`; a corrupt file falls back to defaults
(this can look like forgotten recents). Reopen projects from the sidebar and
inspect settings with `themis call get_settings` when troubleshooting.

## Formatted responses

Replies support Markdown headings, lists, tables and fenced code blocks. Code
blocks show a language label and a Copy button. Python, Bash and other scripts
are displayed without execution. Mermaid fences render diagrams with expandable
source; invalid diagrams show their source instead. Diagram configuration
directives and image resources fall back to source for safety.

The expandable sidebar shows a glinting Themis wordmark and respects reduced motion.


### Workspace and appearance

The icon rail stays visible when the project dock is collapsed. Utilities open
in the main view; New chat, projects, and threads stay in the project dock.
Hover or focus a utility icon for its label. Long thread titles reveal their
full text in one leftward pass on hover or keyboard focus. With reduced motion,
titles wrap instead.

The default **Themis** project lives at `~/ThemisOS/Projects/Themis` and remains
in the sidebar without a lock. Every project, including Themis, supports
display-name changes. Settings shows only the shared projects root. Every new
project is created under `~/ThemisOS/Projects`, with no folder override. Renaming leaves
folders and conversation history in place. Existing projects are not moved.

Appearance is dark only, including when old settings request light or system
mode. **Appearance** offers 17 presets (the original Themis scheme plus 16
alternatives), including GitHub-inspired Dark and Dimmed, and 12 local font
choices with fallback families. Theme, font, and size preview across the app;
**Apply** saves them, **Revert** restores saved choices, and leaving Appearance
cancels an unapplied preview. Code always uses monospace typography. Other
settings continue to save automatically.

Thread editing changes only the name. Select models in the thread's model pill between runs.
Each completed response shows the model used; later switches leave those labels
unchanged. Older responses say “Model not recorded” when metadata is missing.
See [UI rules](ui-rules.md) for the maintained interface contract.

The workspace uses continuous surface layers: lighter utility/header frame, dark
project sidebar, darker canvas. Appearance swatches show these three levels in
the same order for every theme, and the app itself previews a theme until you
Apply or Revert.

### Integration discovery and recovery

Discover cards show purpose, origin and compatibility. Install directly from a tile; compatibility is checked automatically. Partial packages list omitted components and offer Install supported parts. Unsupported-only packages have a disabled Install action and are hidden from the checked catalog unless Unsupported or All statuses is selected. Not checked and Connection not checked are explicit unknowns, not errors or verification badges. New catalog installs are global, with a spinner during installation and ✓ Installed afterwards. Discover shows every marketplace together, grouped by purpose (Coding, Cloud & deploy, Data & databases, Monitoring, Security, Work apps, Design, Other) with a Source filter for one marketplace; search also matches the publisher. Fully working plugins carry no badge; partly working ones show "Some parts skipped". Existing project imports retain their scope. Colored labels retain readable text; installed items filter by Enabled or Disabled. MCP setup and testing remain available through chat or the CLI.

Automation editors keep destination, timezone and enabled/paused status visible above Advanced. Custom permissions explain their next-run scope; approval dialogs label reusable grants Allow in this chat. The companion updates its mood without re-showing its native window. Keyboard focus includes editable instructions.

After a backend event gap or reconnect, Themis reloads completed histories and pending approvals. Active streams keep their current live text and reconcile durable history when the run finishes; missed streaming fragments can remain absent until then. Failed recovery displays a warning and retries. Recovery preserves the selected conversation and local drafts.

The collapsible sidebar lists the chosen project's **Chats** (most recent first; older chats behind **Show more**). A small icon marks a chat that is working, waiting for your approval, failed, or finished in the background since you last looked. Projects with live work are marked in the switcher. Drag the sidebar’s right edge to resize it between 240 and 560 px (double-click resets it); the width is remembered. Opening **Workspace** on a smaller window folds the sidebar to its icons until you close Workspace. Focus the edge and use Left/Right arrows or Home/End to resize with the keyboard. Ordering uses saved conversation events and survives app restarts; opening a chat alone does not change its position, and empty chats remain in their project until they have activity.

When a turn ends, its reply lists the files that turn edited; select one to open its diff in **Workspace**.

To delete a project, open the project switcher and click the trash icon on that project's row; you do not need to switch to it first. After you confirm, Themis moves the project folder to the macOS Trash (recover it from there if needed) and forgets its chats, its automations and its place in recent projects. The built-in **Themis** project, folders outside the projects folder, and projects with a running chat cannot be deleted.

Integrations keeps separate search and filter selections for Installed and Discover. Reset filters affects only the current view. Marketplace checks show the companion and real completed-check percentage, then reveal the checked catalog. Collapsed warnings expand to show compatibility limitations; unavailable local skills remain visible with their validation error.

MCP shows the full saved command and arguments, with status, toggle and uninstall aligned together. Integration errors and installation confirmations appear in the timed notification box at the side. Enabled MCP connections start automatically in the background on app entry and project selection. One failure does not block the app or other connections. Status updates to Connected or Failed; use Refresh to retry a failed connection. A server that needs a secret shows **Secret needed**: press **Add secret**, paste it in Settings › Secrets and save, and it connects. Servers read keys as `${NAME}` (the placeholder their documentation shows), so a configuration copied from a server's README works as is once a secret of that name is saved; an environment variable of that name also works. A remote server that signs in shows **Sign in needed** with **Sign in**. **Edit as mcp.json** starts with commented-out examples of each kind; remove the `//` in front of one to add it.

Open an MCP connection’s details to see its exposed tool count, names and descriptions. This reads cached discovery metadata; it does not execute a tool. Tools appear after the connection completes.

A message sent to a chat from the CLI (`themis thread send`) or another window appears in an open window right away, once, followed by the run as it happens.

For large shared-server requests, use `themis call METHOD - < arguments.json`. The CLI reads bounded UTF-8 JSON from stdin (8 MiB maximum), avoiding operating-system command-line length limits. The complete request, including its method and envelope, must fit the shared server’s 8 MiB frame limit. Secret entry still requires the existing secure provider setup flow.

Conversation history can grow beyond that transport limit. The app and
`themis thread history ID` retrieve it in bounded pages and reconstruct the exact
records, including individual entries larger than one frame. This preserves the
existing full-history result; client/UI memory still grows with that result.
Advanced observers can call `get_thread_history_page` directly to process pages
incrementally. Its JSONL text may split a record across pages: concatenate chunks
and decode only complete lines. Keep the returned `through_seq` snapshot and
`next_cursor` for subsequent requests; a new snapshot includes later appends.
See [history paging](configuration.md#history-paging) for arguments and bounds.

For long coding tasks, checkpoints retain recent tool findings, completed edits and test observations alongside the working summary and original-source references. Summary size adapts to leave room for further work. An unchanged reread may carry a reminder to reuse prior findings; three consecutive previously seen native observations stop the run even when alternating files. Changed contents return a fresh result, and actions clear native read history. Shell retains its consecutive identical-call check because it can change files silently. For large edits, approved Python/bash can operate on local files and return selected verification facts. `apply_patch` requires numbered unified diffs and reports malformed formats before changing files. These controls reduce repeated work; summaries remain lossy and do not guarantee completion or prevent every loop.

If required compaction fails after saving originals and pending inputs, a later
ordinary request also receives selected historical tool observations and their
original-source locations, including after restart. This helps avoid repeating
completed actions; it does not automatically resume work or prove tool success.
Your latest request chooses the task. Stop retains the existing archive recovery
behavior. The bounded record is partial, and originals remain available.

For factual and technical work, the agent is instructed to cite inspected sources,
including current file lines or line ranges for code, and to explain what changed
or was found, why it addresses the task, and relevant tradeoffs. Test reports
should give actual commands, outcomes and captured evidence when available;
unverified claims remain explicit. These instructions apply before and after
compaction, preserve requested formats such as JSON, and do not require citations
for creative prose. They guide model responses; they do not automatically certify
that a citation supports a claim or that reported code locations remain current
after subsequent edits.
