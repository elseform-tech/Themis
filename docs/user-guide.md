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
New Go threads use `muse-spark-1.3-contributor` by default. The composer lists
models available to the connected Go account. Its effort control uses the
selected model's catalog levels and stays visible when no levels are listed.

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
- Hover or focus a sidebar thread to edit its name or remove it.
  Select a Go model in the composer and use the horizontal effort slider when
  that model offers effort levels. Tool calls appear inline
  in the conversation with a chevron, expandable input/output and completion status.
  Public text streams as it arrives. Related updates and tool calls sit inside
  milestone groups; the current group opens while completed groups collapse.
  Expand a group, then a tool call, to inspect its input and output.
  Thinking/using-tool status and elapsed time remain visible during gaps.
- **Stop** requests cancellation at the next safe boundary. A request or tool
  already executing finishes first; completed edits remain available to review.
- **⌘/Ctrl+B** collapses or pins the sidebar. Hovering at the left edge reveals
  an overlay after 300 ms. It closes 700 ms after leaving, unless focus is inside.
  Hover reveal can be disabled in Appearance. A visible button always reopens it.
- Threads appear under their project. Rename long titles with the pencil beside the sidebar thread.
  Drafts, selection and scroll positions restore locally. Conversation turns and
  completed tool events restore from SQLite.
- Enter or ⌘/Ctrl+Enter sends; Shift+Enter inserts a newline.
  ⌘/Ctrl+Shift+O or the composer + starts a new thread.

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

- **Once** — allow this call only.
- **Allow for this run** — allow the same tool and risk for the rest of this run.
- **Deny** — refuse; the agent sees the denial and adapts or stops.

Approval timeouts deny pending requests. Reopening the app restores requests still pending in the running backend. With no explicit approval
configuration, **Ask before read tools** still controls reads. **YOLO** bypasses
harness approval and tool restrictions, including the filesystem root and Git
allowlist. Mode and configuration changes apply to the next run; active runs
keep their captured policy. See [configuration](configuration.md) for examples
and [sandbox](sandbox.md) for the enforcement boundary.

## Skills

The **Skills** tab holds reusable instruction bundles: a name, description,
instructions injected into runs, an optional tool allowlist, and helper
scripts. Attach skills to a thread to scope what it may use. The list includes plugin-bundled skills and skills discovered from local folders. Browse new packages in **Plugins → Discover**; Skills has no separate discovery catalog. Standalone imports remain available through chat or the CLI.

Skill instructions can contain up to 256 KiB of UTF-8 text without truncation.

- An empty allowlist means "all tools".
- Unknown tool names are rejected. The valid names are exactly:
  `list_dir`, `read_file`, `write_file`, `copy_file`, `move_file`,
  `delete_file`, `create_dir`, `search_file`, `shell`, `apply_patch`, `git`.

## Automations and the review queue

The **Automations** tab schedules recurring work with a name, interval in
minutes (≥ 1), task prompt, and enabled flag. Choose where each run goes:

- **Continue an existing thread**: select a thread. Each run uses that thread's
  current model, reasoning effort, and skills, and adds to its history.
- **Create a new thread each run**: select an existing project or create one,
  then choose a Go catalog model, its available reasoning effort, and skills.
  Older automations keep this behavior.

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

Go uses the OpenCode Go API at `https://opencode.ai/zen/go/v1`.
Settings, the composer, and the automation editor select from current Go model
IDs. Their effort controls follow the selected model's catalog levels.

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
remains available; selecting YOLO enables sending. See [sandbox](sandbox.md)
for backend policy and the limits of tool-level restrictions.

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

The expandable sidebar shows a shimmering Themis wordmark and respects reduced motion.


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

Thread editing changes only the name. Select models in the composer between runs.
Each completed response shows the model used; later switches leave those labels
unchanged. Older responses say “Model not recorded” when metadata is missing.
See [UI rules](ui-rules.md) for the maintained interface contract.

The workspace uses continuous surface layers: lighter utility/header frame, dark
project sidebar, darker canvas. Appearance swatches and the Surface preview show
these three levels in the same order for every theme.

### Integration discovery and recovery

Discover cards show purpose, origin and compatibility. Install directly from a tile; compatibility is checked automatically. Partial packages list omitted components and offer Install supported parts. Unsupported-only packages have a disabled Install action and are hidden from the checked catalog unless Unsupported or All statuses is selected. Not checked and Connection not checked are explicit unknowns, not errors or verification badges. New catalog installs are global, with a spinner during installation and ✓ Installed afterwards. Marketplace, status and Personal/Public source dropdowns share one toolbar. Existing project imports retain their scope. Colored labels retain readable text; installed items filter by Enabled or Disabled. MCP setup and testing remain available through chat or the CLI.

Automation editors keep destination, timezone and enabled/paused status visible above Advanced. Custom permissions explain their next-run scope; approval dialogs label reusable grants Allow for this run. The companion updates its mood without re-showing its native window. Keyboard focus includes editable instructions.

After a backend event gap or reconnect, Themis reloads completed histories and pending approvals. Active streams keep their current live text and reconcile durable history when the run finishes; missed streaming fragments can remain absent until then. Failed recovery displays a warning and retries. Recovery preserves the selected conversation and local drafts.

The collapsible sidebar includes **Recent**, showing the eight most recently active chats across open projects. It sits below Projects, with one-line chat titles, current status icons, and full titles/project names on hover. Drag the sidebar’s right edge to resize it between 240 and 560 px; the width is remembered. Focus the edge and use Left/Right arrows or Home/End to resize with the keyboard. Ordering uses saved conversation events and survives app restarts; opening a chat alone does not change its position, and empty chats remain in their project until they have activity.

Integrations keeps separate search and filter selections for Installed and Discover. Reset filters affects only the current view. Marketplace checks show the companion and real completed-check percentage, then reveal the checked catalog. Collapsed warnings expand to show compatibility limitations; unavailable local skills remain visible with their validation error.

MCP shows the full saved command and arguments, with status, toggle and uninstall aligned together. Integration errors and installation confirmations appear in the timed notification box at the side. Enabled MCP connections start automatically in the background on app entry and project selection. One failure does not block the app or other connections. Status updates to Connected or Failed; use Refresh to retry a failed connection.

Open an MCP connection’s details to see its exposed tool count, names and descriptions. This reads cached discovery metadata; it does not execute a tool. Tools appear after the connection completes.

For large shared-server requests, use `themis call METHOD - < arguments.json`. The CLI reads bounded UTF-8 JSON from stdin (8 MiB maximum), avoiding operating-system command-line length limits. The complete request, including its method and envelope, must fit the shared server’s 8 MiB frame limit. Secret entry still requires the existing secure provider setup flow.
