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

## Keys

Use `OPENCODE_KEY` in the environment when launching Themis, or enter a key in
Settings → Models → Manage key → **Save key**.
Entered keys remain in macOS Keychain across app restarts. They are not saved
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
  files or inserts `/` (skill) and `@` (plugin). Run activity collapses into
  **Show thinking** chips; completed replies offer **Copy**. Tool calls appear inline
  in the conversation with a chevron, expandable input/output and completion status.
  Public text streams as it arrives. Related updates and tool calls sit inside
  milestone groups; the current group opens while completed groups collapse.
  Expand a group, then a tool call, to inspect its input and output.
  Thinking/using-tool status and elapsed time remain visible during gaps.
- **Stop** requests cancellation at the next safe boundary. A request or tool
  already executing finishes first; completed edits remain available to review.
- **While a run is going** you can keep typing: **Send** queues the message
  above the composer, and queued messages go out by themselves, oldest first,
  when the run ends. Each one has three icons: **Steer** sends it into the
  running run now (the agent reads it before its next step, without stopping;
  a message with attached files waits for the next turn instead), **Edit**
  changes it while it waits (it is skipped while you edit), and **Remove**
  takes it out of the queue. A steered message shows in the chat as
  *Steered in during the run*. A steer the run never got to read is sent as
  the next message. A message that cannot be sent (a bare `/goal`, a broken
  project configuration) stays in the queue marked **Needs an edit**, with the
  reason as its tooltip and in the chat; edit it (or save it unchanged) to send
  it again. Queued messages survive a restart.
- **Changing the model or effort during a run** applies from the next turn:
  the running turn finishes on the model it started with.

### Goals

A goal is one sentence the agent keeps working toward, turn after turn, until it
is reached; for example `/goal optimize this app to speed up p95`.

- **Start one** with `/goal <sentence>` (the first entry of the `/` menu, at the
  start of a message). The command shows as a **Goal** sticker in the composer
  and in the sent message. It starts right away. When you ask for an ongoing outcome in
  plain words, the agent may ask once whether to make it a goal; if you agree, it
  sets the goal itself.
- **The goal bar** on the composer shows the sentence and the goal's total elapsed
  time across all its turns (time spent paused, waiting or with Themis closed does
  not count). Its icons are
  **Edit** (applies from the next turn; the current turn keeps running),
  **Pause** (after this turn) or **Resume**, **Remove** (the current turn finishes;
  no further turns or checks), and **Show/Hide** for the full sentence. In a narrow
  chat the sentence sits on its own line below the icons.
- **After each turn** a separate check, with a fresh context, reads the goal, the
  turn's answer and the commands it ran, and decides: reached, not yet (the next
  turn starts by itself, told why), or not reachable as written. With a number in
  the goal, that number is the bar; without one, the check decides when further
  gains are not worth it and says why.
  The check reads only the commands of the turn it judges (long outputs keep their
  start and end), never earlier turns or the agent's claims, so the agent ends a
  turn it thinks finishes the goal with one compact verification of every
  requirement. The check also says whether the turn moved the work forward; turns
  that only re-check count toward being stuck.
- **It keeps going.** There is no time, turn or cost limit. Your messages are
  handled first, then the goal continues. Errors are retried with growing waits;
  **Stop** ends only the current turn; a closed app resumes running goals when it
  opens again. A goal pauses only when it is reached, genuinely stuck (no progress
  over three turns, not reachable as written, or three turns in a row failing with
  an error that retrying will not fix, such as a rejected request; fix it or edit
  the goal, then Resume), waiting
  for your answer (your reply resumes it), or out of model quota (it retries every
  15 minutes; switching the model resumes it at once).
- A goal never grants permissions: approvals and the sandbox apply as usual, and
  while an approval waits the goal waits too. A reached goal leaves the composer;
  its note stays in the conversation. Sending a new message after a goal
  is reached clears it; `/goal` again replaces the current goal.
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
- **Workspace** in a thread's header opens a read-only monitor of the project:
  **Changes** (readable changed files and their wrapped, highlighted diff,
  updating live while the agent works; binary and generated files are hidden; a
  change too large to show whole, such as a downloaded dataset, shows its first
  2,000 lines with a note), **Files**, **Branches**, **Pull requests** (needs
  the GitHub CLI signed in) and **Tasks**. Tasks lists this chat's running work and
  the automations that continue it and programs the agent started in the
  background (dev servers, watchers), with **Stop** (the current run, or one
  background program, with its latest output and a read-only **Full log**) and
  **Pause** (turn an automation off); stopping never undoes work already done.
  Background programs end when you delete the chat or quit the Themis server;
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

When a rule asks, the dialog shows the tool, summary, and risk:

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

Approval timeouts deny pending requests and remove the dialog with a short note. Reopening the app restores requests still pending in the running backend. With no explicit approval
configuration, **Ask before read tools** still controls reads. **YOLO** means no
approval prompts: it bypasses harness approval and tool restrictions, including
the filesystem root and Git allowlist, but the sandbox still applies. Mode and
configuration changes apply to the next run; active runs keep their captured
policy. See [configuration](configuration.md) for examples and
[sandbox](sandbox.md) for the enforcement boundary.

### Web search and pages

Agents read web pages with `fetch_url` and search with `web_search`. Search uses a
SearXNG instance you run and name in the user `runtime.jsonc`
(`"web": {"search_url": "http://127.0.0.1:8888"}`); without it only page reading is
offered. Both ask per site in Custom, and **Allow in this chat** covers that site.
Page text and results are treated as untrusted. See [web tools](web-tools.md).

Agents can also drive a built-in browser (an installed Chrome, Chromium or Edge,
with its own profile per project). **Workspace → Browser** opens by itself when the
agent starts using it in a run, so you can watch it: the
line above the page says what the agent is doing. You can click, type or enter an
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
automation checks) run inside an operating-system sandbox in both Custom and
YOLO: Seatbelt on macOS, bubblewrap or Landlock on Linux; Windows has none yet.
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
it). **Settings → Permissions** opens that file (creating a commented-out
starter the first time) and shows any problem with it; `themis permissions`
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

OpenCode Go is the supported provider. Set the default Go model in Settings;
choose a model per thread in the composer.

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

Settings has General, Appearance, Models, Permissions, and Advanced sections.
Ordinary preferences save on change; text inputs save when focus leaves the field.
Errors appear inline, and rejected changes leave the previous preferences intact.
Permissions lets you require approval for read tools and set the approval
timeout. Writes and commands still require approval.

Preferences persisted to `settings.json`:

| Key | Meaning | Default |
| --- | --- | --- |
| `theme` | Dark only; legacy values normalize to `dark` | `dark` |
| `default_provider` | Provider for new threads | `go` |
| `default_model` | Model for new threads (`""` = provider default) | `""` |
| `max_turns` | Max agent turns per run (backend validates 1–200) | `20` |
| `context_messages` | Prior user/final assistant messages sent to the model (1–100) | `20` |
| `approval_timeout_seconds` | Seconds before an unanswered approval is denied (30–600) | `300` |
| `confirm_reads` | Require approval for read tools | `false` |
| `recent_roots` | Recently opened projects, most recent first (max 10, managed automatically) | `[]` |
| `concurrency_limit` | Max parallel runs across all threads (1–16); sends past the limit are rejected | `3` |
| `automations_enabled` | Global automation kill-switch | `true` |
| `projects_directory` | Fixed parent folder for new projects | `~/ThemisOS/Projects` |
| `text_size` | Conversation text size (12–18 px) | `13` |
| `sidebar_hover` | Reveal collapsed navigation on edge hover | `true` |

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
key. Keep `max_turns` low when testing long tasks.

**What are the limits?**
`max_turns` caps turns per run; `concurrency_limit` caps parallel runs;
approval dialogs deny after the configured timeout (default 5 minutes); each shell invocation captures at most
32 KiB of output; provider HTTP calls time out after 120 s.

**Where are my keys?**
Entered keys stay in macOS Keychain until you forget them. Go also reads
`OPENCODE_KEY` from the environment. Keys are not written to SQLite or settings.

**Why is my project read-only?**
Custom mode disables the desktop composer for non-Git folders. The shield
remains available; selecting YOLO enables sending (the sandbox still applies).
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

Discover cards show purpose, origin and compatibility. Install directly from a tile; compatibility is checked automatically. Partial packages list omitted components and offer Install supported parts. Unsupported-only packages have a disabled Install action and are hidden from the checked catalog unless Unsupported or All statuses is selected. Not checked and Connection not checked are explicit unknowns, not errors or verification badges. New catalog installs are global, with a spinner during installation and ✓ Installed afterwards. Marketplace, status and Personal/Public source dropdowns share one toolbar. Existing project imports retain their scope. Colored labels retain readable text; installed items filter by Enabled or Disabled. MCP setup and testing remain available through chat or the CLI.

Automation editors keep destination, timezone and enabled/paused status visible above Advanced. Custom permissions explain their next-run scope; approval dialogs label reusable grants Allow in this chat. The companion updates its mood without re-showing its native window. Keyboard focus includes editable instructions.

After a backend event gap or reconnect, Themis reloads completed histories and pending approvals. Active streams keep their current live text and reconcile durable history when the run finishes; missed streaming fragments can remain absent until then. Failed recovery displays a warning and retries. Recovery preserves the selected conversation and local drafts.

The collapsible sidebar lists the chosen project's **Chats** (most recent first; older chats behind **Show more**). A small icon marks a chat that is working, waiting for your approval, failed, or finished in the background since you last looked. Projects with live work are marked in the switcher. Drag the sidebar’s right edge to resize it between 240 and 560 px (double-click resets it); the width is remembered. Opening **Workspace** on a smaller window folds the sidebar to its icons until you close Workspace. Focus the edge and use Left/Right arrows or Home/End to resize with the keyboard. Ordering uses saved conversation events and survives app restarts; opening a chat alone does not change its position, and empty chats remain in their project until they have activity.

When a turn ends, its reply lists the files that turn edited; select one to open its diff in **Workspace**.

To delete a project, open the project switcher and click the trash icon on that project's row; you do not need to switch to it first. After you confirm, Themis moves the project folder to the macOS Trash (recover it from there if needed) and forgets its chats, its automations and its place in recent projects. The built-in **Themis** project, folders outside the projects folder, and projects with a running chat cannot be deleted.

Integrations keeps separate search and filter selections for Installed and Discover. Reset filters affects only the current view. Marketplace checks show the companion and real completed-check percentage, then reveal the checked catalog. Collapsed warnings expand to show compatibility limitations; unavailable local skills remain visible with their validation error.

MCP shows the full saved command and arguments, with status, toggle and uninstall aligned together. Integration errors and installation confirmations appear in the timed notification box at the side. Enabled MCP connections start automatically in the background on app entry and project selection. One failure does not block the app or other connections. Status updates to Connected or Failed; use Refresh to retry a failed connection.

Open an MCP connection’s details to see its exposed tool count, names and descriptions. This reads cached discovery metadata; it does not execute a tool. Tools appear after the connection completes.

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
