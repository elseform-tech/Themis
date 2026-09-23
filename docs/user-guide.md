# Themis user guide

Themis is a desktop coding agent. You open a project, start a thread, describe
the change, and the agent works in an isolated worktree. Changes reach your
checkout; integrate changes with your Git client when ready.

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
name, and start chatting in the new thread. The default location is
`~/ThemisOS/Projects`; change it under Settings → General, or expand
**Change location** when creating a project. Existing folders are never overwritten.

New projects get a Git repository and initial empty commit so each thread can
work in isolation. Open an existing folder through the project actions menu.

## Keys

Use `OPENCODE_KEY` in the environment when launching Themis, or enter a key in
Settings → Models & connections → Manage key → **Save key**.
Entered keys remain in macOS Keychain across app restarts. They are not saved
in the conversation database, settings files, or browser storage.
Forgetting a saved key does not unset an environment key. A Finder launch
may not inherit environment variables exported by your shell.

**Key available** reports presence only. **Test connection & load models**
checks the Go model catalog and provides a model selector. New Go threads use
`minimax-m2.5` if no default model has been selected. Other providers use their
configured defaults. Ollama needs no key; custom endpoints still require
backend configuration.

## Projects, threads, runs

- **Project actions → Open existing folder** opens an existing project. Other
  folders open read-only (see [sandbox](sandbox.md)).
- The **+** beside a project starts a thread with your default provider and model.
  Each thread on a git project gets its own worktree and branch
  (`themis/<short-id>`).
- Type in the thread view and send. The run streams assistant text and a tool
  trace; non-read tool calls raise an approval dialog (see Approvals below).
- **⌘/Ctrl+K** opens the command palette: jump to threads and projects, or run
  actions (new thread, open project, toggle theme, open settings). **Esc** closes the topmost layer.
- Hover or focus a sidebar thread to edit its name/provider/model/skills or remove it.
  Model and reasoning effort selectors sit together inside the composer. Effort overrides currently support Go/OpenAI GPT-5 and o3/o4
  models; other models use their provider default. Tool calls appear inline
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

Agent edits remain in the isolated worktree. The chat has no diff sidebar or
Merge control. Use your Git client to inspect and integrate changes when needed.
The sidebar remove button asks for confirmation before deleting a thread and
its worktree, including unmerged changes.

## Approvals

Every non-read tool call pauses for an approval dialog showing the tool, a
summary, and the risk level (`read`, `write`, `execute`, `network`,
`destructive`). Decide:

- **Once** — allow this call only.
- **Always** — allow this tool for the rest of the run (cached per tool).
- **Deny** — refuse; the agent sees the denial and works around it or stops.

Reads are auto-allowed unless **Ask before read tools** is enabled in Permissions.
If you ignore a dialog until the configured timeout, it denies
automatically — hanging approvals never resolve to "yes". See
[sandbox](sandbox.md) for the full policy.

## Skills

The **Skills** tab holds reusable instruction bundles: a name, description,
instructions injected into runs, an optional tool allowlist, and helper
scripts. Attach skills to a thread to scope what it may use.

- An empty allowlist means "all tools".
- Unknown tool names are rejected. The valid names are exactly:
  `list_dir`, `read_file`, `write_file`, `copy_file`, `move_file`,
  `delete_file`, `create_dir`, `search_file`, `shell`, `apply_patch`, `git`.

## Automations and the review queue

The **Automations** tab schedules recurring work: name, project, provider,
model, skills, interval in minutes (≥ 1), task prompt, and an enabled flag.

- The scheduler only runs **while the app is open**, and only when
  **automations_enabled** is on in Settings (the global kill-switch).
- **Run now** fires an automation immediately regardless of the kill-switch.
- Each completed run lands in the **Review queue**. Open an item to inspect it,
  then **Continue** (opens the run's thread so you can follow up) or
  **Dismiss**. Items stay `pending` until you act.

## Providers

Supported providers: `go` (OpenCode Go, the default), `openai`, `anthropic`,
`ollama`, `custom`. Set thread defaults in Settings; override per thread.

Model resolution order for a run:

1. The thread's explicit model (thread header or `default_model` setting).
2. The provider's environment fallback: `THEMIS_GO_MODEL`,
   `THEMIS_OPENAI_MODEL`, `THEMIS_ANTHROPIC_MODEL`, `THEMIS_OLLAMA_MODEL`,
   `THEMIS_CUSTOM_MODEL`.
3. The built-in default. For Go this is the last-resort `go-default` — treat
   it as a fallback, not a recommendation; set a real model.

Go speaks the OpenAI chat-completions wire format at
`https://opencode.ai/zen/go/v1` (override per-thread only via a custom
endpoint). Use **Test connection & load models** in Settings to retrieve current model IDs.
The thread model field also accepts an explicit ID.

## Settings reference

Settings has General, Appearance, Models & connections, Permissions, and Advanced sections.
Ordinary preferences save on change; text inputs save when focus leaves the field.
Errors appear inline, and rejected changes leave the previous preferences intact.
Permissions lets you require approval for read tools and set the approval
timeout. Writes and commands still require approval.

Preferences persisted to `settings.json`:

| Key | Meaning | Default |
| --- | --- | --- |
| `theme` | `dark`, `light`, or `system` | `system` |
| `default_provider` | Provider for new threads | `go` |
| `default_model` | Model for new threads (`""` = provider default) | `""` |
| `max_turns` | Max agent turns per run (backend validates 1–200) | `20` |
| `context_messages` | Prior user/final assistant messages sent to the model (1–100) | `20` |
| `approval_timeout_seconds` | Seconds before an unanswered approval is denied (30–600) | `300` |
| `confirm_reads` | Require approval for read tools | `false` |
| `recent_roots` | Recently opened projects, most recent first (max 10, managed automatically) | `[]` |
| `concurrency_limit` | Max parallel runs across all threads (1–16); sends past the limit are rejected | `3` |
| `automations_enabled` | Global automation kill-switch | `true` |
| `projects_directory` | Parent folder for new projects | `~/ThemisOS/Projects` |
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

Settings → **Diagnostics** → **Copy diagnostics** copies a pretty-printed JSON
snapshot (app version, OS, settings, recent errors) to the clipboard for bug
reports. The button also shows the version and OS inline. **Secret values are
never included** — only the settings snapshot and error strings.

## FAQ

**What does Themis cost?**
Themis itself makes no paid calls. You pay your model provider under your own
key (BYOK). Cost per run ≈ model price × tokens; keep `max_turns` low and
prefer cheaper models for triage. Local `ollama` models cost nothing but
electricity.

**What are the limits?**
`max_turns` caps turns per run; `concurrency_limit` caps parallel runs;
approval dialogs deny after the configured timeout (default 5 minutes); each shell invocation captures at most
32 KiB of output; provider HTTP calls time out after 120 s.

**Where are my keys?**
Entered keys stay in macOS Keychain until you forget them. Go also reads
`OPENCODE_KEY` from the environment. Keys are not written to SQLite or settings.

**Why is my project read-only?**
Only git checkouts get worktrees. Folders without git open read-only: the
agent can read and answer, but every write/execute action is denied. See
[sandbox](sandbox.md).

**How do I integrate changes?**
Use your Git client to inspect the thread worktree and merge its branch into
your project. The chat has no Merge control.

**A run failed with "send rejected / runs already active"?**
You hit `concurrency_limit`. Wait for a run to finish or raise the limit in
Settings.

**The app forgot my projects / acts strangely after an upgrade?**
Settings load from `settings.json`; a corrupt file falls back to defaults
(this can look like forgotten recents). Reopen projects from the sidebar and
check Settings → Diagnostics to confirm the loaded snapshot.

## Formatted responses

Replies support Markdown headings, lists, tables and fenced code blocks. Code
blocks show a language label and a Copy button. Python, Bash and other scripts
are displayed without execution. Mermaid fences render diagrams with expandable
source; invalid diagrams show their source instead. Diagram configuration
directives and image resources fall back to source for safety.

The sidebar keeps its galaxy brand header in both themes. Its scale animates
gently and respects the system reduced-motion preference.
