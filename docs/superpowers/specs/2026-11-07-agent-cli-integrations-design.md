# The agent manages integrations with the `themis` CLI

Date: 2026-11-07 · Branch: `codex/agent-cli` · Design only. Building starts after the file-split
refactoring is finished.

## Why

A live test on `main` at `39e2ede4` (`beam-free`, Custom mode, scratch home) asked the agent to
create, update, fix and remove each kind of integration. Every result was checked in the store with
`themis plugin show/list`, not taken from the agent's reply.

| Task | Result | Failed calls |
|---|---|---|
| Create a skill | done | 9: the schema says only `skill: {type: object}`, so the model learned each required field from a separate error |
| Update the skill | done | 4, same cause |
| Fix an existing MCP server whose script path was wrong | **not done** | many. `list_plugins` hides a server's command and arguments, there is no per-server save, and the agent tried `grep -rl … /` and `find` across temp folders |
| Create, change, delete a hook | done, but the first "deleted" came with **no tool call at all**. The real delete happened after the person pointed it out | 1, 1 |
| Add, enable, test a new MCP server (`import_json`) | done | 0 |
| Remove an MCP server and a skill (`remove_component`) | done. It also said the emptied plugin was "gone", which was false | 0 |
| Install, then uninstall `ralph-loop` | done, but `preview` took about 5 minutes because it cloned other plugins' repositories, and the agent again said "only personal remains", which was false | 3 |

Other bugs found:
- `marketplaces` lists `claude-plugins-official`, but `scan_marketplace` and `catalog` reject that name as "Unknown marketplace".
- Diagnostics label a JSON parse error `timeout` and a missing project `network`.
- Loading a built-in guide with the Skill tool asks for write approval.

The built-in guides (`crates/core/builtins/{plugins,skills,mcp,hooks}/SKILL.md`) were present and
loaded. The failures came from the tools, not from missing knowledge. The store was never
damaged: it rejected every malformed whole-plugin save.

The owner decided against adding more agent tools. The agent already has a shell and file writing;
it needs to know where integrations live and the correct way to change them.

## Decision

The agent uses the `themis` CLI through its shell, the same way Claude Code's agent can run
`claude mcp …` and Codex's can run `codex mcp …`. The CLI goes through the validated,
revision-checked store, which is the path the app itself uses. One guard is specific to Themis:
its CLI talks to the live app server, which also exposes approvals and settings, so calls from an
agent's shell are limited on the server.

## What the agent may and may not do

Allowed calls still pass the normal shell approvals: Custom asks unless rules allow, Auto's
reviewer checks, YOLO does not ask, and deny rules always win. Refused calls are refused by the
server in every mode, YOLO included.

### Allowed

- **Read and diagnose** (Read risk): `status`, `doctor`, `permissions`, `usage`, `models`,
  `auth status` (whether a key exists, never the key); diagnostics and logs (metadata only); MCP
  config status; resource use; reading runtime configuration and settings; `plugin list/show/
  inspect/preview/catalog/updates`; `mcp/hook/skill list/show`; its own chat's history pages,
  tasks, background-task logs and context use.
- **Test** (Execute risk, because these start code): `mcp test`, `hook test`, `skill verify`.
- **Change integrations** (through the store):
  - install, update, import or save plugins; add or update MCP servers, hooks and skills;
  - disable an MCP server or hook; switching on a hook or a command-based MCP server is refused
    in every mode (the person switches it on), one by URL may be switched on (Execute risk);
  - uninstall or remove (Destructive risk, never covered by "Allow in this chat");
  - add or remove marketplace sources (Network risk).
  - New MCP servers and hooks still start switched off.

### Refused for agent calls

| Area | Methods |
|---|---|
| Approving itself | `approve_action`, `answer_question` |
| Its own permissions | `set_thread_approval_mode`, `save_runtime_configuration`, `ensure_runtime_config`, opening the config editors |
| Settings | `update_settings`, `update_browser_settings`, `update_web_settings`, `update_subagent_settings`, `update_memory_settings`, `set_provider` |
| Secrets and keys | `set_secret`, `clear_secret`, `set_key`, `remove_key`, `list_keys`, `check_go_key` |
| The person's terminal | `terminal_*` |
| Projects | `create_project`, `rename_project`, `delete_project` |
| Other chats | sending, queuing, steering or stopping runs in other chats; `discard_thread`, `merge_thread`, `switch_branch` |
| Review decisions | `accept_file`, `discard_file`, `dismiss_review_item`, `continue_review_item` |
| The person's browser controls | Workspace `browser_*`, `clear_browsing_data` (the agent keeps its own browser tools) |
| The app | `server start/stop`, `serve`, `app launch/quit` |
| Memory | `delete_memory` and other memory edits |
| Anything else | `themis call` for a method not on the allowed list |

The sandbox wall on the app data folder stays: the agent never edits `registry.json` by hand.
Reading other chats' history and creating automations or goals through the CLI stay out of scope
for now; the agent already has tools for its own chat.

## Where integrations live (the guides give the resolved paths)

| Kind | Global | Project |
|---|---|---|
| Plugins, MCP servers, hooks, custom skills | app data `plugins/registry.json` (with its revision) and `plugins/packages/<name>/` | `.themis/plugins/` |
| Skills found on disk | `~/.agents/skills`, `~/.codex/skills`, `~/.claude/skills` | `skills.paths` from `.themis/config.jsonc` |
| Not an integration | `marketplace-cache/` (the catalog, not installed) | — |

The guides teach: **find** (read the definition; `themis plugin show`), **change** (write the new
definition to a scratch file, apply it with the matching `themis` command), **verify**
(`themis … test` or `plugin show` again, and report what the store says).

## Steps (one PR each, failing tests first)

1. **`themis` on the agent's PATH, plus the server guard.** Ship the CLI with the desktop app. Each
   run's shell gets a short-lived token that identifies the run and chat. The server refuses agent
   tokens on the methods above. The sandbox lets the CLI reach the local port. (Built: the CLI in
   an agent's shell gets the port and its run token from the environment instead, and the sandbox
   hides `server.json` from every agent command, YOLO included, because that file holds the
   person's own token and reading it would bypass the guard.) Tests: a CLI end-to-end test proves each refused method is refused for an agent token
   (approvals and permission mode first) and works without one, plus a scale test that the guard
   check stays O(1) per call.
2. **Risk levels for `themis` subcommands** in the shell's risk check: list/show/doctor are Read,
   test/enable are Execute, delete/uninstall are Destructive. This follows the git read-only rule
   from #268. Property test over parsed command lines. (Built, per the owner's table of
   2026-11-07: reads, changes (write or network), tests (execute) and removals (destructive,
   `plugin marketplace remove` included); switching on a hook, an MCP server or a plugin is asked
   every time, never covered by a chat grant or a broad allow rule, and in Auto goes to the
   person, never the reviewer. Only a bare `themis` is lowered; wrappers keep the ordinary risk
   and the removal and switch-on checks look inside them; unknown integration commands count as
   switching on. The agent's saves and imports keep new or changed MCP servers and hooks switched
   off, so an allowed change cannot start code. No new setting: a deny rule for shell commands
   starting with `themis` turns it off, YOLO included. Tests: property tests and a fuzz target
   over command lines, approval tests per mode with a scripted reviewer, and a CLI end-to-end
   run through the real app server. See "Approvals for `themis` calls" in
   `docs/configuration.md`.) **Owner decision 2026-11-07 (option A):** the app server never
   lets an agent switch on a hook or a command-based MCP server, in any mode, YOLO included,
   whatever the command, script or request. The store refuses it inside the registry write and
   keeps new or changed ones off on every agent save, import and install, for the CLI and for
   `manage_integrations` alike; the per-command argv dialog for agent-saved MCP commands is
   gone. Connections by URL keep the consent rule. The shell still asks every time before
   `hook/mcp/plugin enable`, though the server refuses the hook or command-based server anyway.
   **Security fix 2026-11-08:** steps 1–2 left `plugin_action` open to an agent's credential apart
   from secrets and sign-in, so a script holding the credential (no `themis` command for the
   shell's approval step to see) could have the server run `test_hook`/`test_mcp` outside the
   sandbox, write another project's `.themis/plugins/`, or fetch under `sandbox.network: "block"`.
   The guard now allows `plugin_action` per action from an explicit list (store reads and writes;
   fetching actions only when the run's network is not blocked), refuses `test_hook`, `test_mcp`
   and `mcp_status` with `connect`/`retry` for every agent credential, and binds the credential to
   its run's project and network block when it is issued.
   **Owner decision (2026-11-08): the agent's tests run in its sandbox.** In an agent's shell,
   `themis mcp test` and `themis hook test` no longer ask the server: the `themis` process starts
   the command itself, inside the run's sandbox and with its network setting, so a test can do
   nothing the shell could not already do. It keeps the Execute risk (Custom asks, Auto's
   reviewer decides). A failed test shows what the hook printed and tells the agent that the
   sandbox may be in the way, in which case it asks the person to run the same test in their own
   terminal. Step 4's `doctor --integrations` starts code the same way from an agent's shell.
3. **Clear inputs on the CLI path.** Complete skill, hook and MCP definitions in `--help` and
   errors; defaults for empty lists (`allowedTools`, `scripts`); one error listing every problem.
   Fix the marketplace name lookup and stop single-plugin `preview` from scanning the whole
   marketplace.
4. **One command that checks every installed integration**: `themis doctor --integrations`
   (Read risk except where it starts code). Today verification is piecemeal: `plugin inspect`
   checks a package before install, `mcp test` and `hook test` check one installed item and need
   `--project`, `skill verify` checks a file rather than an installed skill, and `doctor` checks
   the app (server, key, storage, scheduler) but no plugin, MCP server or hook. The new check
   covers global and project scope, installed and custom only: each plugin's stored definition
   against the store's validation; each enabled MCP server started and its tools listed (bounded
   by the existing 30-second start limit, Execute risk, so Custom asks once for the run); each
   hook's command found and runnable (not executed); each skill's paths present and its
   `SKILL.md` readable. It reports every problem with its item, scope and a one-line fix hint, as
   JSON with `--json`, and logs metadata only. The guides' verify step uses it after every change,
   and it is in the A/B rerun.
5. **Guides rewritten** around find, change, verify, with resolved paths. `/hooks` uses
   `themis hook …`. A tool advertises itself in every request and a CLI does not, so the agent
   also needs a reminder once the old tools go (decided 2026-11-07): one short system-prompt line
   for the main agent (plugins, skills, MCP servers and hooks are managed with `themis` in the
   shell; the `plugins`, `skills`, `mcp` and `hooks` guides show how) and one for subagents (they
   may inspect integrations with read-only `themis`; changes are the main agent's). Both are
   prompt files, verified in the constructed system message.
6. **A/B rerun of the 2026-11-07 test**: the old tools against the CLI path, same tasks and model.
   Score failed calls, false claims, and whether the store ended up right. Label it as a benchmark
   audit. It must include requests that never mention `themis` (for example "add an MCP server for
   X"), where the agent has to find the CLI unprompted, and a delegated task ("find out why this MCP
   server fails") that a subagent answers with read-only `themis`.
7. **If the CLI path is equal or better, remove `manage_integrations`, `list_plugins` and
   `save_skill` outright** (no aliases), with docs and live-check rows updated in the same PR.

Each step gets the configuration and visibility it needs per `AGENTS.md`, a live-check row, and a
real-app check before it is called done.

## Related, separate PR: `/subagents`

People find features in the composer's `/` menu, and nothing there says subagents exist. A
built-in `/subagents` skill, like `/hooks`: on its own it lists what is available (the built-in
subagent and plugins' subagent profiles, each read-only or with its write paths, its model, and the
Settings › Subagents limits) with one example request; `/subagents <task>` hands that task to
subagents. Its own PR, after steps 1 and 2.

## Separate question, not part of this plan

Twice the agent said a change was done when it was not. Once it made no tool call at all; once it
reported a plugin removed that was only emptied. Fewer failed calls will make this rarer, but a
check on claims of state change without a matching tool result is its own design.
