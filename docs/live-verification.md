# Live verification after a merge

Automated tests use mock providers. After every merge to `main`, a live check confirms that the
freshly built app works with real models. It is a **benchmark audit**, reported separately from
the automated test inventory, and it comes in two tiers:

- **Tier 1, every merge (about 15–20 minutes, watched):** the smoke check (step 2) and the
  coverage rows the merged PR touched (step 3), driven in the real app while the person watches,
  plus the goal soak and verification eval running unwatched (steps 5 and 6).
- **Tier 2, the full tour (about 60–90 minutes, watched):** every row of the coverage table
  (step 4). Run it weekly, before a release, or when asked.

Never use real app data or the real Keychain: every step below runs with its own HOME and data
folder, and the key comes from `OPENCODE_KEY` (the app falls back to it when no Keychain entry is
found). Use free or cheap models (for example `beam-free` or `iceman-contributor-free`, else
`muse-spark-1.3-contributor`, `mimo-v2.6-flash`, `qwen3.8-flash`, `glm-5.3-flash`,
`deepseek-v4.1-flash`), at high effort where the model offers it.

## 1. Sync and build from `main`

```sh
git checkout main && git pull --ff-only
npm --prefix web run build
cargo build -p themis-desktop --features tauri/custom-protocol   # desktop app with the built UI
cargo build -p themis-desktop --bin themis                       # CLI and app server
```

## 2. Smoke check (every merge, about 5 minutes)

Wrap the fresh `target/debug/themis-desktop` in a scratch `.app` with a test bundle identifier
(for example `ai.themis.desktop.livecheck`) and launch it with a scratch HOME:

```sh
HOME=$(mktemp -d) ThemisLive.app/Contents/MacOS/themis-desktop
```

1. In a new chat on a cheap thinking model, ask for a file to be written and read back. Check the
   approval box appears once on the composer, disappears after Allow once, and the answer is
   right and ends with a short summary.
2. Ask a follow-up that depends on the first answer without reading files; it keeps the context.
3. Open Settings (every section loads) and Workspace (every tab loads), then quit the app.

## 3. What the merge touched (every merge)

Pick the rows of the coverage table below whose **Code** column matches a file the merged PR
changed, and run their steps in the same scratch app. A PR that changes shared machinery
(`crates/core/src/runtime.rs`, `providers.rs`, `crates/desktop/src/state/run.rs`, the composer
or the thread reducer) also runs the goal backbone (step 4A). Report each row as passed, failed
or skipped with the reason.

## 4. The full tour (weekly, before a release, or on request)

### A. One goal that uses every tool

In a new project, set the scratch `runtime.jsonc` to `"context_token_budget": 32000` (it applies
to the next run, which also checks configuration reload), start Custom mode, and send:

> /goal Build a tiny `tour` Python package in this project and prove each step: create
> `tour/slug.py` with `slug(text)` and a failing test in `tests/`, run the tests, fix it with a
> patch, and commit on a new branch `tour`. Copy, move and delete a scratch file and make a
> directory. Search the files for `slug`. Look up Python's `re.sub` documentation on the web and
> quote one sentence with its URL. Serve the project with a background `python3 -m http.server`,
> open it in the built-in browser, click into `tour/`, screenshot it and attach the screenshot.
> Ask me which separator to use (`-` or `_`) before finishing. After any checkpoint, recall the
> exact first failing test output. List my plugins, save a one-line skill named `tour-note`, and
> create a daily automation for this chat that you then pause. Stop the server at the end.

While it runs, do the **During the goal** rows: queue a message, steer one in, answer the
question box, use an "Allow in this chat" grant, edit and pause then resume the goal, and finally
stop one run with Esc twice. The goal must reach its end with every step proved, a progress line
at each step and a summary at the end of every turn.

### B. Every screen and setting

Walk the remaining rows in table order: every Settings section (change a value, confirm it takes
effect, put it back), Integrations, Automations, Review queue, Search, Help, the companion, and
project and chat management.

## Coverage table

Every user-facing feature has a row. **ID** names it: `tool:` an agent tool, `settings:` a
Settings section, `workspace:` a Workspace tab, `view:` a main view, `config:` a `runtime.jsonc`
group, `feature:` anything else. The `live_coverage` test in `crates/desktop/tests` fails when a
tool, Settings section, Workspace tab, main view or configuration group exists without a row, or
a row names one that no longer exists, so **a PR that adds a feature adds its row**. **Tier**
`smoke` rows run on every merge; `tour` rows run when the merge touches their **Code** or in the
full tour.

| ID | Tier | Code | Check |
| --- | --- | --- | --- |
| `tool:list_dir` | tour | `crates/core/src/tools.rs` | Goal lists the project. |
| `tool:read_file` | smoke | `crates/core/src/tools.rs` | Smoke step 1 reads the file back. |
| `tool:write_file` | smoke | `crates/core/src/tools.rs` | Smoke step 1 writes with one approval. |
| `tool:copy_file` | tour | `crates/core/src/tools.rs` | Goal copies the scratch file. |
| `tool:move_file` | tour | `crates/core/src/tools.rs` | Goal moves it. |
| `tool:delete_file` | tour | `crates/core/src/tools.rs` | Goal deletes it; the approval names the path. |
| `tool:create_dir` | tour | `crates/core/src/tools.rs` | Goal makes the directory. |
| `tool:search_file` | tour | `crates/core/src/tools.rs` | Goal finds `slug`. |
| `tool:shell` | tour | `crates/core/src/tools.rs`, `crates/core/src/sandbox.rs` | Goal runs the tests; the approval shows the argv. |
| `tool:background_task` | tour | `crates/core/src/tools/background.rs` | The server runs in the background, shows in Workspace › Tasks and stops at the end. |
| `tool:apply_patch` | tour | `crates/core/src/tools/patch.rs` | Goal fixes the test with a patch; Changes shows the diff. |
| `tool:git` | tour | `crates/core/src/tools.rs` | Goal commits on `tour`; Branches lists it. |
| `tool:recall` | tour | `crates/core/src/tools/recall.rs` | After a checkpoint, the first test output comes back exact. |
| `tool:fetch_url` | tour | `crates/core/src/tools/web.rs` | Goal reads the `re.sub` page. |
| `tool:web_search` | tour | `crates/core/src/tools/web.rs` | Goal finds the docs page through search. |
| `tool:browser_open` | tour | `crates/core/src/browser` | The page opens; the overview box shows it live. |
| `tool:browser_navigate` | tour | `crates/core/src/browser` | Goal navigates to the served page. |
| `tool:browser_snapshot` | tour | `crates/core/src/browser` | Goal reads the page. |
| `tool:browser_click` | tour | `crates/core/src/browser` | Goal clicks into `tour/`. |
| `tool:browser_type` | tour | `crates/core/src/browser` | Ask the agent to type into the address or a search field. |
| `tool:browser_select` | tour | `crates/core/src/browser` | Ask the agent to pick an option on a page with a select box. |
| `tool:browser_press` | tour | `crates/core/src/browser` | Ask the agent to press Enter or Escape on a page. |
| `tool:browser_move` | tour | `crates/core/src/browser` | Ask the agent to aim in a first-person page: `dx` turns the view past the page's edge, and a click shoots without turning it. |
| `tool:browser_scroll` | tour | `crates/core/src/browser` | Ask the agent to scroll a long page. |
| `tool:browser_tabs` | tour | `crates/core/src/browser` | Ask the agent to list and switch tabs. |
| `tool:browser_screenshot` | tour | `crates/core/src/browser` | Goal screenshots the page. |
| `tool:browser_record` | tour | `crates/core/src/browser` | Ask for a short recording; it plays in the media viewer. |
| `tool:browser_devtools` | tour | `crates/core/src/browser/tools/devtools.rs` | Only with Developer mode on: read the console. Off: the tool is not offered. |
| `tool:set_goal` | tour | `crates/desktop/src/state/goal_tool.rs` | `/goal` sets the goal; the goal bar shows it. More › Remove goal removes it; asking in the chat to pause or remove it does the same. |
| `feature:goal-proof` | tour | `crates/desktop/src/state/goal.rs` | Run a goal over several turns: a rejected turn's replies dim, and the next turn doesn't re-prove what is listed. A passing "Checking…" shows during a check, then a "Goal check" bubble saying what is wrong, what to change and what is pending, and the agent answers it. |
| `tool:ask_user` | tour | `crates/desktop/src/state/question_tool.rs` | The separator question appears on the composer after any approval, answered inline. |
| `tool:spawn_subagents` | tour | `crates/desktop/src/state/subagent_tools.rs` | Ask for a review of two separate areas: two read-only subagents start, both results reach the reply, and the turn ends only after both. |
| `tool:attach_file` | tour | `crates/desktop/src/state/media_tool.rs` | The screenshot is attached to the reply. |
| `tool:list_plugins` | tour | `crates/desktop/src/state/integration_tools.rs` | Goal lists plugins. |
| `tool:save_skill` | tour | `crates/desktop/src/state/integration_tools.rs` | `tour-note` appears in Integrations › Skills. |
| `feature:prompt-hooks` | tour | `crates/core/src/plugins/hooks.rs`, `web/src/screens/HooksByEvent.tsx` | Install a local plugin with a `UserPromptSubmit` hook that prints a ticket number and a `PreToolUse` `Bash` guard: Hooks lists them under Prompt submitted and Before a tool runs (`shell, background_task`); the agent's reply uses the ticket number; a guard that blocks stops the shell call. |
| `feature:browser-idle` | tour | `crates/core/src/browser/session.rs` | Let the agent open a page with a WebGL loop: CPU drops to near idle within seconds of the run ending, and the page draws again on the next action or when the Browser tab opens. A first-person page can lock the pointer after a click. |
| `feature:chat-width` | tour | `web/src/screens/ThreadView.css`, `web/src/screens/OverviewBox.css` | With a long reply open, collapse and open the sidebar, open Workspace and drag its divider: the text and composer reflow to the room left each time, and stay clear of the overview box during a run. |
| `feature:live-changes` | tour | `web/src/lib/useLiveDiff.ts`, `web/src/screens/WorkspacePanel.tsx` | With a chat open and no run going, edit a tracked file in another editor: the header count and Workspace › Changes update within a few seconds, without any click. During a long shell command that writes files, they update while it runs, also with Workspace closed. |
| `feature:branches` | tour | `crates/desktop/src/state/workspace.rs`, `web/src/screens/WorkspacePanel.tsx`, `web/src/screens/ThreadComposer.tsx` | In a project with a second branch: Workspace › Branches, click it: its files and diff show. The composer shows the current branch; clicking it lists the others and switching updates the chip and Changes. During a run the chip is plain text. A switch that would overwrite uncommitted changes shows Git's one-line reason in a toast. |
| `feature:stop-hook-loop` | tour | `crates/core/src/plugins/hooks.rs`, `crates/core/src/runtime.rs` | Install `ralph-loop`, turn on its hook and run `/ralph-loop "Add one line to notes.md" --max-iterations 3`: the agent goes three rounds and ends on its own. With no limit, Esc twice stops it. |
| `feature:hook-inputs` | tour | `crates/core/src/plugins/hooks.rs` | With a local plugin hook that saves its input: `transcript_path` points at a file holding the chat, `permission_mode` follows the shield, and `logs/` doesn't contain the prompt. A `PreToolUse` `Edit` guard refusing `secret.txt` refuses a patch that changes `fine.txt` and `secret.txt`, and neither file changes. A `SessionStart` hook of a plugin `kit` writing `export KIT_X=1` and `export PATH=/x` to `$CLAUDE_ENV_FILE`: a later hook of `kit` sees `KIT_X` and `THEMIS_ENV_REFUSED=PATH`, and the agent's `echo $KIT_X` prints nothing. |
| `feature:skill-hooks` | tour | `crates/core/src/plugins/hooks.rs`, `crates/core/src/runtime.rs` | With a local plugin holding a skill and a `PreToolUse` `Skill` hook that saves its input: ask for a task the skill fits and, after the agent reads it, the input has `tool_name` `Skill` and `tool_input.skill`. Attach it with `/`: the hook runs before the reply. A hook that exits 2 stops the turn. |
| `feature:mcp-sign-in-presets` | tour | `crates/core/src/plugins/oauth.rs`, `crates/core/src/plugins/marketplace.rs`, `crates/desktop/src/state/mcp.rs` | Install `render` and turn its connection on: Sign in needed. Sign in opens Render's own sign-in page in the browser with `client_id=claude`, and Cancel stops the wait. Don't sign in unless the account is a throwaway. Install `wingspan`: the page is Wingspan's, with a client Themis registered. |
| `feature:plugin-literal-secrets` | tour | `crates/core/src/plugins/marketplace.rs`, `crates/core/src/plugins/keys.rs` | Install `cloudinary`: no "Some parts skipped". Turn on `cloudinary-mediaflows`: Secret needed lists `CLOUDINARY_CLD_API_KEY`, `CLOUDINARY_CLD_CLOUD_NAME` and `CLOUDINARY_CLD_SECRET`, and Add secret opens Settings › Secrets on that name. Install a local plugin whose `.mcp.json` holds a made-up key: searching the data folder for the key finds nothing. |
| `feature:plugin-secret-consent` | tour | `crates/desktop/src/state/mcp_consent.rs`, `web/src/screens/Plugins.tsx` | Install a local plugin whose `.mcp.json` sends `X-Test: ${OPENCODE_KEY}` to a local listener, with that secret saved, and turn its connection on: Needs your OK says "Sends OPENCODE_KEY to" the host, and the listener has seen nothing. Allow: it connects. Change the plugin's URL and update it: it is off again, and asks again. |
| `feature:hook-sessions` | tour | `crates/core/src/plugins/hooks.rs`, `crates/desktop/src/state/hook_sessions.rs`, `crates/desktop/src/server.rs` | A local plugin whose `SessionStart` hook appends its input to a file and prints a ticket number, and whose `SessionEnd` hook does the same: three turns, one start, and the agent still knows the number on the third; no end. Delete the chat: one end with `reason` `other`. In a new chat, quit the app: one end with `reason` `prompt_input_exit`, and quitting takes no more than a few seconds with a hook that sleeps. With `"hooks": {"session_idle_minutes": 1}`, leave a chat for two minutes: an end, and writing again starts with `source` `resume`. |
| `feature:hook-moments` | tour | `crates/core/src/plugins/hooks.rs`, `crates/core/src/plugins/marketplace.rs`, `crates/core/src/runtime.rs` | A local plugin logging each of `UserPromptExpansion`, `PostToolBatch`, `PostCompact`, `PermissionRequest`, `PermissionDenied`, `Notification`, `ConfigChange` and `InstructionsLoaded`: attach a skill with `/`, use two tools in a step, deny an approval in Custom and change `.themis/config.jsonc` between turns, and each logs once with the input the docs give. A `PermissionRequest` hook answering allow: the person is still asked. One answering deny: nobody is, and the agent says why. Hooks lists each under its own event. |
| `feature:hook-questions` | tour | `crates/core/src/plugins/hooks.rs`, `crates/core/builtins/prompts/hook-prompt-system.md` | A local plugin with a `PreToolUse` `Write` prompt hook, "Refuse writes to notes.md": on a cheap model, a write to notes.md is refused with the reason and another file's goes through. Stop the provider: the write is refused rather than let through. `"hooks": {"prompt_hook_calls": 0}` in `runtime.jsonc`: no check and no refusal. |
| `feature:mcp-sse` | tour | `crates/core/src/plugins/connections.rs`, `crates/core/src/plugins/marketplace.rs` | Install `paypal` and enable its server: it shows **Secret needed**, and with a sandbox token it connects and lists tools. Install `windsor-ai`: **Sign in needed**. |
| `feature:hook-reports` | tour | `crates/core/src/plugins/hooks.rs`, `crates/desktop/src/state/hook_wakes.rs`, `web/src/screens/ThreadConversation.tsx` | In Custom, a local plugin whose `Stop` hook has `asyncRewake`, sleeps 5 seconds, prints a finding on stderr and exits 2: Allow it, and after the turn ends the chat shows a Plugin hook row and the agent says what it made of it, up to three times and then it stops by itself. Stop during one: no more until you write. |
| `feature:plugin-odds-and-ends` | tour | `crates/core/src/plugins/marketplace.rs`, `crates/core/src/plugins/hooks.rs`, `crates/core/src/plugins/connections.rs` | Install `monday-crm`: it arrives switched off. Install `dominodatalab`: `/style-domino-mlops` changes how the next reply is written. Install `clay` and ask the agent to run `clay --version`: it runs from the plugin's `bin`, and `ls` is still the system's. Install `pinecone`, use a chat, restart the app and continue it: its `resume` hook runs on that turn. |
| `feature:hooks-command` | tour | `crates/core/builtins/hooks/SKILL.md`, `crates/desktop/src/state/plugins.rs`, `web/src/lib/toolNames.ts` | `/hooks` (no `manage-` prefix) lists hooks with one `list_plugins` call; creates a hook (saved off), changes its command and deletes it; Hooks agrees. A guard's approval box reads `Hook · plugin · name`, and a blocked shell call tells the agent the hook's reason. |
| `feature:plugin-sources` | tour | `crates/core/src/plugins/marketplace.rs`, `crates/core/src/plugins/scans.rs`, `web/src/screens/Sources.tsx` | With a fresh HOME open Discover: plugins appear while the catalog is checked with "Checking N/M" beside the filters, unchecked ones say "Checking…", cards and group counts hold still until it finishes, and a checked one installs before then. Sources: a repository without a marketplace says so in one line; a local fixture previews with name, publisher and count, then adds; hiding it removes its cards; removing it keeps its plugin, whose row shows "Source removed". |
| `feature:plugin-updates` | tour | `crates/core/src/plugins/scans.rs`, `web/src/screens/Plugins.tsx` | Install a plugin with a hook from a local fixture, turn the hook on, change the fixture's hook and run `themis plugin marketplace changes NAME`: only that entry is checked again. Installed shows the version change and Update; after Update the changed hook is off and an unchanged part keeps its switch. A plugin row opens to its parts, each with a working switch; the tabs read Plugins, Skills, Connections, Hooks. Settings › General has Check sources for updates. |
| `feature:plugin-packages` | tour | `crates/core/src/plugins/package.rs`, `crates/core/src/plugins/marketplace.rs` | Install `hyperframes` from Discover: no "Some parts skipped" for size, `plugins/registry.json` stays small, `plugins/packages/` holds its files, and a chat that @mentions it finds them under `.themis/plugin-files/`; edit a file there and start another turn: the edit is gone. Install `carta-crm` and turn on its hook: the compiled hook under `hooks/bin` runs. |
| `feature:plugin-icons` | tour | `crates/core/src/plugins/marketplace.rs`, `web/src/screens/PluginLogo.tsx` | Discover shows brand logos on matching plugins and colored initial tiles on the rest, with no icon requests while browsing. A local fixture with a bundled PNG shows its own icon in Discover and in Installed, and keeps it after its source is removed. |
| `tool:manage_integrations` | tour | `crates/desktop/src/state/integration_tools.rs` | Ask the agent to disable then enable a plugin; Integrations agrees. |
| `tool:manage_automations` | tour | `crates/desktop/src/state/integration_tools.rs`, `crates/desktop/src/state/automation.rs` | The daily automation appears paused in Automations and Tasks. |
| `feature:subagent-profiles` | tour | `crates/core/src/plugins/agents.rs`, `crates/desktop/src/state/subagent_tools.rs`, `web/src/screens/Plugins.tsx` | Install `pr-review-toolkit` from Discover: its card counts subagents and it has no "Some parts skipped" warning, and View names them in words. Ask "use the code-reviewer profile to review this file": a subagent starts and Tasks shows it working read-only. Turn the plugin off and ask again: the agent reports that no such profile exists and no subagent starts. |
| `feature:subagent-mcp` | tour | `crates/desktop/src/state/subagents.rs`, `crates/core/src/plugins/agents.rs`, `web/src/screens/SubagentSettings.tsx`, `web/src/screens/SubagentDetail.tsx` | With a connection enabled, ask for a subagent that uses one of its tools: in Custom the call asks under the subagent's name, its detail says "Files read only · Connections: <server>", and a Stop during a slow call ends it. Turn off Allow connections in Settings › Subagents and ask again: the subagent says it doesn't have that tool and the main agent still does. |
| `feature:subagent-web` | tour | `crates/desktop/src/state/subagents.rs`, `crates/core/src/plugins/agents.rs`, `web/src/screens/SubagentSettings.tsx`, `web/src/screens/SubagentDetail.tsx` | In Custom ask for a subagent that reads a web page: the site asks under the subagent's name, its detail says "Files read only · Web", and a deny rule on that host refuses it in YOLO too. Turn off Allow web in Settings › Subagents and ask again: the subagent has no web tools and the main agent still does. |
| `tool:todo_write` | tour | `crates/desktop/src/state/subagent_tools.rs` | A subagent whose profile names `TodoWrite` keeps a plan; its current item shows as its status in Tasks. |
| `feature:subagent-claude-tools` | tour | `crates/core/src/tools/shell_scope.rs`, `crates/desktop/src/state/subagents.rs`, `crates/desktop/src/state/subagent_tools.rs` | Install `sonarqube` and use its reviewer: its detail says "Shell: git diff:*, git status:*, …", `git diff` runs, and asking it to run `git push` or `sh -c "git diff && rm x"` is refused without any prompt, in YOLO too. Install `spotify-ads-api` and use its builder: its question appears in the chat headed with its name, and your answer reaches it. A profile naming `TodoWrite` shows its current item as its status. |
| `feature:subagent-models` | tour | `crates/desktop/src/state/subagent_tools.rs`, `web/src/screens/SubagentSettings.tsx` | Add a model under Allowed models, then ask "use <model> for the review and the default for the docs": Tasks shows that model on one row only; a model that fails at once shows "<model> → <fallback>". |
| `feature:subagent-chat` | tour | `web/src/screens/SubagentCard.tsx`, `crates/desktop/src/state/subagents.rs` | Ask for a subagent whose answer the main agent should check and send back: the main agent says what it is about to do before each step, the status names who it waits for, the chat shows no subagent bubbles, its Tasks detail shows the brief, messages and replies as a conversation, and the same subagent answers again with its earlier exchange in mind. |
| `feature:subagent-writes` | tour | `crates/desktop/src/state/subagents.rs`, `web/src/lib/changeAuthors.ts` | Ask for one subagent that writes a file in `notes/` and tries one outside it: the file in `notes/` appears, the other is refused, the detail says "Writes notes/", and Workspace › Changes filters by that subagent. With Allow file writes off in Settings › Subagents the same request stays read-only. |
| `feature:subagents-ui` | tour | `web/src/screens/SubagentCard.tsx`, `web/src/screens/SubagentDetail.tsx` | Ask for two subagents in Custom mode: the stack shows two ringed circles, a subagent's command asks under its name, the overview shows its approval, a circle opens its detail in Tasks with Stop, and the rings stop when they end. |
| `feature:goal` | tour | `crates/desktop/src/state/goal.rs`, `web/src/screens/GoalBar.tsx` | Edit, pause after this turn, resume and remove from the goal bar; the goal continues across turns and ends with one summary. Each turn opens in the agent's own thinking-aloud words with no "Goal:" label, turns are divided by a plain line, and no status box appears in the chat. |
| `feature:checkpoints` | tour | `crates/core/src/runtime.rs`, `crates/core/src/runtime/maintenance.rs` | At 32k the goal passes a checkpoint; the context meter drops and the work carries on. |
| `feature:queue-and-steer` | tour | `crates/desktop/src/state/queue.rs`, `web/src/screens/QueueStack.tsx` | Queue, edit and remove a message; steer one in and see it handled. In Custom mode, while an approval box waits: a queued message leaves the box there; steering shows the message in the chat at once, the box goes away with no expiry warning, the agent reads the message without the box being answered, and it shows once, also after a reload. The same action asks again later. |
| `feature:approvals` | smoke | `web/src/components/approval`, `crates/desktop/src/state/chat_grants.rs` | Allow once, Allow in this chat (no prompt next time), Deny; after a click on the box, keys 1–3 decide (typed in the composer, they do not). |
| `feature:yolo` | tour | `web/src/lib/yolo.ts`, `crates/core/src/sandbox.rs` | Switch the shield to YOLO: no prompts; a deny rule still blocks. |
| `feature:stop` | tour | `web/src/screens/ThreadComposer.tsx` | Esc twice stops the run with the hint shown first. |
| `feature:turn-summary` | smoke | `crates/core/src/runtime.rs` | Every turn ends with a short natural summary; thinking folds only after the turn. |
| `feature:turn-end-guard` | tour | `crates/core/src/runtime/turn_end.rs`, `crates/core/builtins/prompts/runtime-turn-end-guard.md` | On a free model that narrates, run a multi-step goal: turns no longer stop on a line that only announces the next step or on a tool call written as text. The tool runs after the line, the line shows once, an offer or a question to you ends the turn, and `logs/diagnostics-0.jsonl` holds `turn_end_guard` records with a reason and no reply text. |
| `feature:model-picker` | smoke | `web/src/screens/ThreadHeader.tsx` | Change the model and effort; the next run uses them. |
| `feature:attachments` | tour | `web/src/screens/AttachmentPreview.tsx`, `crates/desktop/src/state/attachments.rs` | Attach a file with +; the agent reads it. |
| `feature:mentions` | tour | `web/src/components/PromptInput.tsx` | `/` lists skills, `@` lists plugins; the chosen one is used. |
| `feature:overview-box` | tour | `web/src/screens/OverviewBox.tsx` | Shows changes, tasks and the live browser; its icon opens Workspace and back returns to the box. |
| `feature:usage` | tour | `crates/desktop/src/usage.rs` | Tokens and cost under the reply match Settings › Usage. |
| `feature:sound-and-notifications` | tour | `web/src/lib/completionSound.ts`, `crates/desktop/src/notifications.rs` | Sound off by default; switched on, it plays when a reply finishes. |
| `feature:projects-and-chats` | tour | `web/src/screens/Sidebar.tsx` | Create, switch and rename a project; rename and remove a chat. |
| `feature:search` | tour | `web/src/screens/Palette.tsx` | ⌘K finds a chat and a command. |
| `feature:help` | tour | `web/src/screens/Sidebar.tsx` | Help opens the shortcuts. |
| `feature:companion` | tour | `web/src/components/KnightCompanion.tsx` | The companion shows state notes and can be resized and moved. |
| `feature:cli` | tour | `crates/desktop/src/cli.rs` | `themis status`, `themis thread list` and `themis doctor` work against the running app. |
| `settings:General` | tour | `web/src/screens/Settings.tsx` | Toggle the reply sound; it takes effect on the next reply. |
| `settings:Appearance` | tour | `web/src/screens/Appearance.tsx` | Preview, revert and apply a theme. |
| `settings:Models` | tour | `web/src/screens/ModelsSettings.tsx` | Change the default model; a new chat uses it. |
| `settings:Subagents` | tour | `web/src/screens/SubagentSettings.tsx` | Set Maximum subagents to 2; `runtime.jsonc` shows `max_parallel: 2` and the next spawn runs two at a time. |
| `settings:Usage` | tour | `web/src/screens/UsageSettings.tsx` | Today's chats appear with their tokens. |
| `settings:Secrets` | tour | `web/src/screens/SecretsSettings.tsx` | Add then remove a dummy secret. |
| `settings:Permissions` | tour | `web/src/screens/PermissionsView.tsx` | The config file reads as valid; Open opens it; the default mode applies to a new chat. |
| `settings:Browser` | tour | `web/src/screens/BrowserSettings.tsx` | Turn Developer mode on and off; `browser_devtools` follows. |
| `settings:Web` | tour | `web/src/screens/WebSettings.tsx` | Check the search server. |
| `settings:Advanced` | tour | `web/src/screens/Settings.tsx` | Lower Steps per reply; a long run stops at it; reset limits. |
| `workspace:changes` | smoke | `web/src/screens/WorkspacePanel.tsx` | The goal's changes and diff appear live. |
| `workspace:files` | tour | `web/src/screens/WorkspacePanel.tsx` | Browse to `tour/slug.py` and read it. |
| `workspace:branches` | tour | `web/src/screens/WorkspacePanel.tsx` | Lists `tour`. |
| `workspace:pulls` | tour | `web/src/screens/WorkspacePanel.tsx` | Shows PRs or the reason it cannot. |
| `workspace:tasks` | tour | `web/src/screens/TasksView.tsx` | Shows the run and the server; Stop and Pause work. |
| `workspace:goal` | tour | `web/src/screens/GoalView.tsx` | During a goal, the Goal tab sorts the checklist into Pending and Complete; the list appears when the goal is set, a steered request lands in Pending at once, editing the goal redoes the list, items move as checks pass, and the overview box counts them. |
| `workspace:browser` | tour | `web/src/screens/BrowserView.tsx` | Drive the page by hand while the agent uses it; Capture to message works. |
| `view:skills` | tour | `web/src/screens/Skills.tsx`, `web/src/screens/Plugins.tsx` | Create, edit and delete a skill; install, enable and disable a catalog plugin. |
| `view:automations` | tour | `web/src/screens/Automations.tsx` | Create, run now, pause and delete an automation. |
| `view:queue` | tour | `web/src/screens/ReviewQueue.tsx` | Items appear and can be cleared. |
| `config:approval` | tour | `crates/core/src/configuration.rs` | Add a deny rule; the tool is blocked in Custom and YOLO. |
| `config:sandbox` | tour | `crates/core/src/sandbox.rs` | With `"network": "block"` a shell `curl` fails. |
| `config:browser` | tour | `crates/core/src/configuration.rs` | Set by Settings › Browser; the file shows the change. |
| `config:web` | tour | `crates/core/src/configuration.rs` | Set by Settings › Web; the file shows the change. |
| `config:skills` | tour | `crates/core/src/skills.rs` | A skill under a configured path is found. |
| `config:hooks` | tour | `crates/core/src/configuration.rs`, `crates/desktop/src/state/hook_wakes.rs` | With `"hooks": {"max_wakes": 0}`, a local plugin whose `Stop` hook has `asyncRewake` and exits 2 starts no turn on its own, and the next message's turn reads what it reported. `session_idle_minutes` and `prompt_hook_calls` are checked under `feature:hook-sessions` and `feature:hook-questions`. |
| `config:subagents` | tour | `crates/core/src/configuration.rs` | With `"enabled": false` in the project's `.themis/config.jsonc` the agent offers no subagents. |
| `config:instructions` | tour | `crates/core/src/configuration.rs` | A listed file's rule is followed. |
| `config:providers` | tour | `crates/core/src/providers.rs` | A configured provider lists in Settings › Models. |
| `config:models` | tour | `crates/core/src/providers/catalog.rs` | A `reasoning_effort` override is the default effort. |
| `config:logging` | tour | `crates/core/src/diagnostics.rs` | Diagnostics rotate under app data, with no prompts or keys. |
| `config:compaction` | tour | `crates/core/src/runtime/maintenance.rs` | Settings here change the checkpoint and summary model. |
| `config:goal` | tour | `crates/desktop/src/state/goal.rs` | `goal.check_model` is used for the goal check. |

## 5. The goal soak (unwatched)

```sh
python3 -m evals.soak run --model muse-spark-1.3-contributor
```

`evals/soak` runs one long goal through the real app server and CLI: 120 prose memos (about 196k
tokens) read and written one at a time under a 64k budget, so the run passes many checkpoints, a
follow-up is queued midway, and three recall questions follow the goal. It tests our machinery,
not the model's skill: goal turns continue, checkpoints succeed and keep the first message's rules,
tools behave. Runs take 20–90 minutes and about $0.30–0.60, under a $1.50 per-run cap. The result
is a trend against the last 5 runs of the same model, budget and job: a **breakage** flag on a
harness measure is our bug; a **regression** flag on a model measure asks a person to look at the
trace. Commit the run's `history.jsonl` line with the report. See `evals/soak/README.md`.

## 6. The verification eval (unwatched)

```sh
node scripts/bundle-searxng.mjs --if-missing
python3 -m evals.verify run --model muse-spark-1.3-contributor --profiles none,mixed
```

`evals/verify` gives each run a generated Ledgerly project (about 100 files, too big for the
context) and a realistic request whose cause is away from its symptom; hidden checks score the
share of requirements met. The `mixed` profile adds messages mid-task (a change of direction, a
tightened requirement, "add a test", an unrelated question and fix), and two scenarios pile up
requests in one chat. Budgets are 32k, 64k and auto (16k and 32k for `accumulating-requests`). It
reads verification habits (a check after the last change, unchecked or false claims, a bug
reproduced first, a regression test), evidence for lookups, and harness health (checkpoints,
re-reads after them, our tool errors). Runs take 2–6 minutes and about $0.01–0.04 each. The result
is a trend per model, scenario, budget and profile; compare a branch with `--themis`, `--label`
and `python3 -m evals.verify ab`. Before trusting a consistent partial score, open the kept project
and check the failing hidden test asks only for what the request and docs state. See
`evals/verify/README.md`.

## Reporting and fixing

Report each step with its exact command and result: the smoke check, every row run (passed,
failed or skipped and why), the soak's flags, end state, lost runs and their reasons, and the
verification eval's trend. Scan the scratch folders for saved reasoning or the key. A bug found
here is fixed on its own `codex/<task>` branch with a regression test (and a CLI end-to-end case
for backend changes), then merged and verified the same way.