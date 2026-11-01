# Plugins and prompt skills

The desktop Integrations screen browses plugin bundles, skills, MCP connections, hooks and marketplace sources. Tile installs and new chat-managed integrations use global app-data scope, with one-click plugin/skill installation and basic lifecycle controls. Existing project integrations remain supported; advanced CLI imports can explicitly choose project scope. Advanced MCP configuration and diagnostics use files or the CLI. Chat management skills and the CLI use the same validated APIs. See [configuration and diagnostics](configuration.md) for locations, precedence, permissions, and logging. Personal plugins have local project scope (`.themis/plugins`) or global app-data scope. App and CLI use the same local server and revisioned store.

## Prompt workflow

Type `/` to choose a skill or `@` to choose an enabled installed plugin. A plugin mention binds its bundle and exposes enabled skill metadata, including manual-only skills, without injecting every instruction body. Skill selection remains explicit. Catalog paths are relative to the run workspace so `read_file` can open the materialized instructions without relaxing file-access boundaries. Type `/skills` to ask the agent to design one. The editor renders an icon and skill name; persisted prompts contain `[[skill:ID]]` or `[[plugin:g--NAME]]` / `[[plugin:l--NAME]]`. Disabled or missing plugin mentions fail explicitly. New plugin references use stable scope/plugin/skill identities; plugin version and saved revision remain separate. Existing revision-pinned references still resolve their original instruction snapshot, so updates preserve saved prompts. Current enablement and removal controls apply to old references too. Disabled or uninstalled plugins cause an explicit unavailable-reference error. Old revision references keep their labels but are excluded from new autocomplete selections. Each skill, connection, and hook can be enabled independently. Removing one component preserves its siblings; a later marketplace update can restore components that still exist upstream.

In a chat, Up/Down on the first/last line recalls previously submitted prompts and restores the current draft. Arrow keys select slash suggestions when autocomplete is open. Enter selects a suggestion before it can send a chat; Shift+Enter inserts a newline.

Automations select skills in their task prompt. Continuing automations use the thread's current model and effort, but do not inherit its attached skills. Legacy standalone automation skill IDs remain supported and are migrated into prompt references when edited.

Installed and Discover each retain independent search, source and status filters while the browser is open. Reset filters clears only the current view. Result counts and filtered empty states reflect the current selection. Discover shows cards with descriptions, source, and explicit compatibility status. Click a name to inspect a package. Install on a tile checks compatibility automatically and installs supported packages globally. Partial packages show omissions before Install supported parts; unsupported packages cannot install. A spinner marks progress and the tile retains ✓ Installed on completion. Plugin View lists only skills and their View actions; the separate partial-install decision exposes omissions. Not checked means no compatibility claim. Compatible means the format is supported, not that an external service has been tested. Unsupported packages have no supported capabilities, cannot be installed, and are hidden after inspection unless Unsupported or All statuses is selected. Refresh starts a new persisted compatibility scan. Each skill has one view action that opens the complete instruction body as rendered Markdown. The original `SKILL.md` remains captured byte-for-byte; frontmatter is omitted from the pretty view. Long documents scroll inside the viewer. Public plugin and standalone skill previews use the same validated importer without installing or enabling the package. The list scrolls beneath fixed search controls. Management skills remain available in normal chat; there are no Ask Themis controls. After inspection, compatible packages install with Install; partial packages use Install supported parts with omissions visible before the action. Supported components are installed while unsupported capabilities remain recorded; imported executable components stay disabled. Advanced source/configuration imports, compatibility inspection, MCP tests, and edits use chat or the CLI. Icons use a supplied validated HTTPS or bundled SVG asset; packages without one use a fallback.

## Agent skill creation

The built-in `/skills` command owns `crates/core/builtins/skills/SKILL.md` and `scripts/verify.py`. Its instructions tell the agent to write a JSON draft, run the materialized verifier at `.themis/skills/skills/verify.py`, test helper scripts, then call `save_skill`. Management tools are available in every chat, independently of installed plugin activation. Built-in management skills guide plugin, skill, MCP, hook and automation setup; normal approvals still apply. Global scope is the default for new chat-created skills; edits preserve the existing scope. Backend validation and optimistic revision checks also run on every save.

Run the standalone verifier:

```sh
python3 crates/core/builtins/skills/scripts/verify.py /path/to/skill.json
python3 crates/core/builtins/skills/scripts/verify.py /path/to/skill-directory
```

It checks required fields, names, instruction/script limits, known tools, duplicate scripts, traversal, and directory symlinks. Skill instruction bodies accept up to 256 KiB of UTF-8 text across import, discovery, creation, and runtime validation; bodies are never truncated to fit. Frontmatter does not count toward the instruction limit. Creator draft files are bounded at 2 MiB to allow JSON escaping and metadata; bundled script content remains capped at 64 KiB. It does not evaluate whether instructions solve the task. Python 3 is required for this check.

## Marketplace compatibility

The default source is Anthropic's `claude-plugins-official` Git repository; individual packages are installed by the user. Additional HTTPS Git repositories or local marketplace directories can be added. Repository imports support an explicit branch, tag, or commit and safe subdirectory. Personal imports accept local skill/plugin folders, a standalone `SKILL.md`, native JSON bundles, MCP JSON files, and pasted MCP JSON. Archives are rejected explicitly. Pasted MCP/native import JSON accepts JSONC comments and trailing commas; runtime and pasted configuration are limited to 1 MiB. File/repository import formats retain their own parser limits; JSONC is not universally supported by upstream manifest files. Imports read Claude marketplace/plugin manifests, `SKILL.md`, UTF-8 resources, `.mcp.json`, and command hooks. Imported MCP servers and hooks begin disabled. Source metadata records local paths, repository refs/subdirectories, or marketplace origin. Upstream icons are retained when they are validated HTTPS URLs or bundled SVG resources; unavailable binary icons are reported. Make a personal copy to customize a marketplace package without losing changes on update.

Unsupported components are prominently listed in the plugin detail view. All import paths reject packages with no supported capabilities; inspection remains available, and existing installed packages are not removed. Claude LSP/settings and restricted tool matchers are not executed. Plugin folders are read as follows (skill discovery keeps its stricter rules, below):

- Symlinks are followed when their target resolves inside the plugin folder and isn't a folder already being read. Other links (outside the plugin, broken, or looping) are not followed and are listed as "Link not followed: <path>".
- A plugin of text only, within 8 MiB, 1,000 files and 1 MiB a file, is stored whole in the registry. Any other plugin (larger, or with a large or binary file such as a compiled hook) keeps only its components in the registry (manifests, `SKILL.md`, commands, subagents, output styles, declared hook and MCP files) and every file on disk under `plugins/packages/<hash>/` beside the registry (project scope: `.themis/plugins/packages/`), with executable bits kept and set-id bits never. Every run checks the stored package against its hash and makes `.themis/plugin-files/` hold exactly the installed files: anything changed is put back and anything else there (a planted file, a link) is removed, for plugins kept in the registry too, because whoever can write the project can write that folder. So a plugin must keep its own data in `CLAUDE_PLUGIN_DATA`, not beside its files. A stored package that doesn't match its hash counts as missing. A check only hashes the files, so scanning a catalog stores nothing; installing from a check downloads them again and stops ("changed after it was checked") if they differ. Uninstalling removes packages nothing else refers to, after an hour. A package that has gone missing logs `plugins` / `package_missing`, and a hook that needed it fails like any missing script.
- A package may hold 512 MiB, 20,000 files and 64 MiB a file (the largest in the official catalog at the last check: 207 MiB, 7,927 files, 9.2 MiB). A single file past that is left out and listed as a notice. A repository may reach 2 GiB while downloading, history included, and a git command may take 180 seconds; past either the download stops and the check fails.
- A plugin folder past a package's bounds has only its own folders imported (`.claude-plugin`, `.mcp.json`, `SKILL.md`, `skills`, `commands`, `agents`, `hooks`, `scripts`, `assets` and declared paths), reported as partial ("Large repository") and logged as `plugins` / `package_over_limits`. If those alone are past the bounds the check fails ("Plugin exceeds resource limits").
- Notices, like unused skill frontmatter and Claude tool-name notes, appear in the compatibility details but don't make a plugin "Partially supported".
- `skills`, `mcpServers` and `hooks` in `plugin.json` or in the marketplace entry may name paths inside the plugin. Skill paths are folders of skills; MCP and hook paths are JSON files that are imported like `.mcp.json` and `hooks/hooks.json`. Paths that are missing or leave the plugin are reported.
- Every `SKILL.md` is a skill, unless that gives two skills the same id. Then only Claude's layout counts (a root `SKILL.md`, `skills/<name>/SKILL.md` and declared skill folders), so copies made for other tools and reference files inside a skill folder are resources, not skills. Ids that still collide come from the skill's folder path (`extra/review` becomes `extra-review`). A plugin with no skills in Claude's layout keeps all of them.
- An MCP server defined identically in `plugin.json` and `.mcp.json` is imported once; different definitions with the same name still fail.
- Claude plugin commands (`commands/**/*.md`, plus command files or folders declared as `commands` in `plugin.json` or the marketplace entry) become manual-only skills, used with `/` like any manual skill. The id is the file name with folders joined by `-` (`commands/git/clean.md` is `git-clean`); a command whose id a skill already uses gets `-command`. The `description` frontmatter, else the first line, is the description. Each command's instructions open with a short note (`builtins/prompts/skill-command-note.md`) saying that `$ARGUMENTS` (and `$1`, `$2`, …) is the text written after the command, that `!`command`` lines are commands to run with the shell tool, and that `@path` names a file to read; those runs and reads go through the normal approvals. Other command frontmatter (`allowed-tools`, `argument-hint`, `model`) is listed as notes.
- `output-styles/*.md` become manual-only skills named `style-<file>`, used with `/`. Themis has no lasting output style, so one applies to the turn it is chosen for, which the compatibility details note. `bin/` programs are found by the run's commands after the system's, so a system program of the same name always wins; they run under the same approvals and sandbox as any command. `defaultEnabled: false` in the manifest means the plugin arrives switched off.
- Claude plugin subagents (`agents/*.md`, plus files or folders declared as `agents` in `plugin.json` or the marketplace entry; a folder's direct `.md` children only) become subagent profiles in the plugin's own registry entry. See [Subagent profiles](#subagent-profiles).

The importer supports simple scalar/block YAML frontmatter. Unsupported source types are rejected rather than silently ignored. MCP imports accept Claude `mcpServers` and OpenCode `mcp` shapes, normalize local command arrays and HTTP URLs, and require credentials to be `${VARIABLE}` or `{env:VARIABLE}` references; plain non-secret values are allowed as described in [Keys for MCP servers](#keys-for-mcp-servers). Literal credentials and malformed definitions are rejected. Inspection reports unsupported transports/headers, and unknown MCP options with an offending field, reason, and remedy. Advanced strict imports require `--allow-partial` for unsupported components. Catalog installation explicitly offers supported components only for partial packages; skipped capabilities remain recorded. Partial installation never bypasses safety or type validation. Imported connections remain disabled until explicitly enabled. This is compatibility with supported components, not the full Claude plugin runtime.

## Subagent profiles

A profile is what `spawn_subagents` runs when a subagent names it as `profile`. The agent finds them on that tool's description: one line per profile (a name and at most 100 characters of its description), at most 2 KiB in all, then how many more there are. The list, and the `profile` field, appear only when an enabled plugin ships profiles. A name that isn't found, or a disabled plugin's, fails the call before any subagent starts, and the error names what there is (at most 2 KiB). When two plugins use one name, ask for `plugin:name`; a project plugin wins over a global one with the same plugin name. The profiles are captured when a run starts, like the rest of its configuration. Each profile follows its plugin's switch; there is no separate one.

- **Definition.** `name` and `description` frontmatter and a body are required, and frontmatter is limited to 64 KiB. A file without them, with frontmatter that never closes, a tool list that never closes, a repeated `name`, `description`, `model` or tool list, or a name that isn't a safe single word fails the import, as does a second file with the same name in one plugin. A name in words (`Company Researcher`) is asked for as `company-researcher`. Bodies have the 256 KiB limit of skill instructions and are never cut. Links leaving the plugin and declared paths outside it are reported and not read.
- **Instructions.** The body goes in the subagent's system message before its brief, labelled as coming from the plugin (`builtins/prompts/subagent-profile.md`), with the access it really has stated after it. It stays there through checkpoints, model fallbacks and `message_subagent`.
- **Tools.** `tools` (or `allowed-tools`) can only narrow what a subagent would have. `Read`, `Grep`, `Glob`, `LS` and `NotebookRead` stand for `read_file`, `list_dir` and `search_file`; `Bash` for `shell`; `Edit`, `MultiEdit` and `Write` for `write_file`, `apply_patch` and `create_dir`; `WebFetch` for `fetch_url` and `WebSearch` for `web_search`. `*`, or no list, keeps a subagent's usual tools. Three more come only when named, since a plain subagent has no use for them: `TodoWrite` (and `TaskCreate`, `TaskUpdate`, `TaskList`, `TaskGet`) for `todo_write`, its own plan, whose item in progress is its status line; `BashOutput`, `KillShell`, `TaskOutput` and `TaskStop` for `background_task`, with `shell` then able to start background programs that only it can see and that stop when it ends; and `AskUserQuestion` for `ask_user`, a question in the chat headed with its name, which must carry a recommendation since a subagent can't stop to wait, and whose answer goes to that subagent while it runs and otherwise to the main agent. The system message names the tools it has in Claude's words, for instructions written for Claude. Any other name is left out and listed as "Subagent tools left out", which makes the plugin Partially supported. That includes `Task` and `Agent`: subagents don't start subagents, a depth limit of one that is a decision and not a gap to close. It also includes a scope on any tool but the shell, such as `Edit(src/**)`. An empty list leaves no tools.
- **Scoped shell.** `Bash(git diff:*)`, `Bash(gh pr view *)` and `Bash(python3 *check.py*)` offer the shell held to those commands. It passes only what it can positively confirm, and refuses anything else before any approval, in YOLO too, harmless or not; the subagent gets a refusal and reports it (`subagents` / `shell_scope_refused`, with no command). A shell script (`sh -c`) never passes, because the shell expands its words after they are read. A command passes when:
  1. It is one simple command, direct or as the only command of an `sh -c` script: no chains, pipes, substitutions, assignments or redirections to files (`2>&1` and `>/dev/null` aside), and no wrapper (`env`, `sudo`, `xargs`, `timeout` and the like), even one a pattern names.
  2. Its words fit a pattern, word by word, with its program as written (`./git` and `/usr/bin/git` aren't `git`, and a pattern can't leave the program open). `:*` and a final ` *` take any further words, a last word ending in `*` may have words after it, and any other `*` stays in its word.
  3. If its program can run others, every option and `NAME=value` word is confirmed: named by the pattern as a word of its own, or, for options, on a short list for that subcommand that only choose what is shown (`git diff --stat`, `git log --oneline`). So `-Ic`, `-pe`, `--eval=x`, `git -c`, `git diff --output=x`, `tar xI`, `find -exec`, `rg --pre`, `make test SHELL=/x` and `pytest -p x` are refused by not being confirmed, not by being on a list of bad ones. What such a program runs must also be spelled out, so `python3:*`, `git *`, `npx:*` and `tar *` allow only the bare program. These are interpreters and shells, `awk` and `sed`, `npx` and its kin, `git`, `npm`, `make`, `cargo`, `go`, `gh`, `docker`, `ssh`, `tar`, `rsync`, `find`, `rg`, `pytest`, `jest`, `curl` and the like.

  A plain `Bash` beside a scope is the whole shell, and denying part of the shell takes all of it. The Tasks detail shows the limit ("Shell: git diff:*").

  **Scopes narrow what a subagent is offered; the sandbox is the wall.** A scope is not a security boundary against code in the repository: a program that runs project code (`npm test`, `pytest`, `make test`, `go run x`, `python3 check.py`, and `*check.py*` also fits `../evil-check.py`) runs whatever that code does, including a configuration file it finds by itself or by a path it is given, and a program outside the kinds above does what it does with whatever words its pattern lets through. What limits all of that is the OS sandbox, where a subagent with no `write_paths` has a read-only plan. With no enforcing sandbox (Windows, or the sandbox off) a scoped subagent still has its shell, as any subagent does, and its detail says "Shell not confined": the scope then still refuses what it can't confirm, but nothing contains what a named program does.
- **MCP tools.** A list may name `mcp__server__tool`, `mcp__plugin_name_server__tool`, `mcp__server__*` or `mcp__server`. These choose among the chat's connections and add none: only connections you enabled, and that came up when the run started, count, and neither a plugin nor a subagent can add or enable one. `*`, or no list, gets all the chat's MCP tools; an explicit list keeps unnamed ones out; `disallowedTools` takes them away. A named tool that isn't there (server not connected or switched off) is left out, the subagent is told, and it doesn't fail. That doesn't make the plugin Partially supported: whether a server is connected belongs to your chat, not to the plugin, and the name did map. MCP tools can act outside the project and aren't covered by a subagent's read-only file limit or its write paths, so its detail says "Files read only · Connections: docs" rather than "Read only". They count as no file permission: a list with only MCP tools has no write tools. Calls ask under the subagent's name in Custom, as its other tools do, and stopping it ends them; a stdio server's reply that arrives after that is ignored. `subagents.allow_mcp: false` turns all of this off.
- **Web tools.** A subagent has `fetch_url` and `web_search` (when the chat has a search address), unless its list leaves them out. They are the main agent's tools under the same rules: each site asks under the subagent's name in Custom, deny rules hold in YOLO, Settings › Browser's site permissions apply and `BeforeTool` and `AfterTool` hooks fire with `agent`. A profile's model and instructions can't add to that. `subagents.allow_web: false` takes them away, and so does `sandbox.network: "block"`, since these tools run in the app, where the OS sandbox can't stop them; a list that names them is told they aren't available (`subagents` / `web_left_out` logs why). The Tasks detail then reads "Files read only · Web" rather than "Read only".
- **Without a profile.** A plain subagent has no MCP tools unless the main agent lists them in `mcp_tools` on `spawn_subagents` (its own tool names, or `*`). With a profile the same field can only narrow what the list allows. The field is offered only when the run has MCP tools and `subagents.allow_mcp` is on, and names that don't exist are left out and reported back as `mcp_left_out`.
- **Writes.** A profile that lists `Edit` or `Write` still runs read-only unless the main agent gives it `write_paths`. Giving `write_paths` to a profile whose list has no write tools is refused rather than ignored. `subagents.allow_writes: false` keeps it read-only like any other.
- **Starting subagents.** A profile whose list names `Agent` or `Task` can start subagents of its own; `Agent(a, b)` limits it to those profiles, as `name` or `plugin:name`. `*` or no list doesn't give it this, and neither does the main agent. It goes one level: what such a subagent starts can't start any more, and is told so, so a plugin built on deeper chains runs only their first two levels. What it starts can do no more than it can: only its tools, inside its write paths, held to its shell limit and only its MCP tools. Results go to the subagent that asked, not the main agent, and it answers for them: stopping or ending it stops them. They wait in their own `subagents.max_parallel` slots, so a parent waiting for its helpers never blocks them. The Tasks detail says "Started by …". `subagents.allow_nested: false` turns this off.
- **Model.** `model` frontmatter is matched like a model the agent asks for, against `subagents.model` and `subagents.models`. With no match, or `inherit`, `subagents.model` applies. A `model` in the spawn call wins over the profile's.
- Other frontmatter (`effort`, `skills`, `maxTurns`, `memory` and so on) isn't used and is listed as a note; `color` is ignored.

Discover and Installed count them with a plugin's other parts ("1 skill · 2 subagents"), and a plugin's View names them ("Subagents: Code reviewer"). The `started` line in the `subagents` diagnostics names the profile.

## Skill discovery

Themis reads conventional user and project `.agents/skills`, `.codex/skills`, and `.claude/skills` folders automatically. User folders are under your home folder: `HOME` on macOS and Linux, `USERPROFILE` on Windows. Runtime `skills.paths` replaces the project search roots only; user discovery continues across the conventional home folders. Discovery does not register or change the source folders. Skills with the same display name receive separate source-path identities; identical canonical sources are deduplicated in `.agents`, `.codex`, then `.claude` order. Source directory symlinks are canonicalized for read-only discovery, cycles are skipped, and symlink resources inside a package are rejected explicitly. Project discovery rejects sources that escape the project root after canonicalization. The scan stops at eight directory levels and 1000 skills. Invalid or oversized skills appear as disabled packages with a compatibility notice. `disable-model-invocation: true` keeps a skill available for explicit slash invocation while excluding it from automatic discovery.

User and project activation overrides persist in Themis. Discovered skills can be disabled; uninstalling their source requires removing the original file. Text reference/script resources are available as per-run copies inside the project sandbox, with no executable permissions granted by discovery. Original absolute `SKILL.md` paths remain available as metadata. Instructions and imported scripts still follow the normal tool approvals when used.

## MCP and hooks

Connections support stdio and Streamable HTTP JSON-RPC, initialization, paginated tool discovery, and tool calls. Optional server-provided initialization instructions reach the model as labelled external tool documentation after startup; they do not grant permissions. Malformed instructions or bodies larger than 32 KiB are rejected. MCP/hook-only plugins have zero bundled skills; Themis does not invent a placeholder `SKILL.md`. Exact legacy generated placeholders are excluded on registry load while authored skills, original resources and revision identities remain preserved. Credentials are never literal configuration values: a server reads keys as `${NAME}` references (see [Keys](#keys-for-mcp-servers)). Remote servers that require OAuth sign-in are supported (below); the legacy SSE transport is not. Enabling a connection yourself (UI or CLI) is the consent to start that exact definition: a changed command, arguments or URL, an agent edit you did not approve, or a definition arriving from a repository or import does not start until you enable it again, and an agent enabling or changing a command-based connection asks you about the exact command every time (an explicit deny rule still blocks startup; see [configuration](configuration.md)); tool calls use Themis approval gates independently of skill selection, and **Allow in this chat** on one tool covers the server's other tools. A failed connection is omitted with a sanitized warning; chat and management remain available to repair it. Blocking hooks retain their failure policy.

Canonical hook events are `PromptSubmit`, `RunStart`, `BeforeTool`, `AfterTool`, `BeforeCompaction`, `Stop`, `RunEnd`, `SubagentStart` and `SubagentEnd`; legacy event aliases remain supported, and Claude plugins' `SubagentStart` and `SubagentStop` hooks import onto the last two. A subagent's `BeforeTool` and `AfterTool` hooks run like the main agent's, and every payload from a subagent carries `agent: {id, name}`; `RunStart` and `RunEnd` belong to the chat's run only. Hook matchers select tools; ignore/abort failure policy is explicit. Commands receive a JSON payload through stdin and may return JSON. Before-event hooks can block on failure, or by answering `{"decision": "block", "reason": "…"}` (also `{"block": true}`, or Claude's `hookSpecificOutput.permissionDecision: "deny"` with `permissionDecisionReason`) whatever their failure policy. A blocked tool call tells the model which hook blocked it and why; the reason is capped at 1 KiB, fenced and labelled as coming from the hook and not as instructions. Imported Claude hooks have no names of their own, so each takes the name of the script it runs, or else of when it runs (`before-shell`). Hooks cannot grant tool permissions. A hook's timeout is 1–600 seconds. MCP server processes and hooks are not run inside the OS sandbox that confines agent commands (see [sandbox](sandbox.md#what-it-covers)); they run with your permissions once you enable them. Commands have bounded output/time; Unix process groups terminate descendants on timeout/cancellation. Imported resource roots are passed to hooks as `CLAUDE_PLUGIN_ROOT`, never substituted into shell source. Explicit chat/CLI tests run the saved configuration; normal runs request approvals.

`PromptSubmit` runs once per turn, after `RunStart`, with the person's message as `prompt` in its payload. A hook that answers `{"block": true}` or `{"decision": "block"}` stops the turn before any model request ("Blocked by hook <name>: <reason>"); one that merely fails does not, unless its failure policy is abort. What it prints (plain text, or `context`, `additionalContext` or `hookSpecificOutput.additionalContext` in JSON) is given to the model after the message, labelled as coming from a hook and not as instructions, fenced and capped at 4 KiB; it never grants permissions and is not taken as the person's request. Claude plugins import `UserPromptSubmit` as `PromptSubmit` and `PostToolUseFailure` as the failure-only `ToolCallFailed`. `SessionStart` also imports as `PromptSubmit`, never blocking: its output is context for the model, so it runs on every turn (its context goes only into that turn's request, never into the saved history). A `SessionStart` matcher that includes `startup` counts as every turn; one limited to `resume` runs on a chat's first turn after the app started, when it has history (`source` says which); one limited to `clear` or `compact` is skipped, having no equivalent. Their tool matchers are mapped to Themis tools: `Bash` → `shell|background_task`, `Read` → `read_file`, `Write` → `write_file`, `Edit` and `MultiEdit` → `apply_patch|write_file`, `Grep` → `search_file`, `Glob` and `LS` → `list_dir|search_file`, `WebFetch` → `fetch_url`, `WebSearch` → `web_search`, `Agent` and `Task` → `spawn_subagents`. `NotebookEdit` counts as a file edit, because Themis has no notebook tool, and `Shell` as `Bash`. Alternatives that name MCP tools (`mcp__.*__run_query`, `__exec$`) are kept as a regular expression, stored after `re:` at the end of the matcher (`shell|background_task|re:mcp__aws.*`) and tested, unanchored as in Claude, against each MCP tool's two Claude names: `mcp__<server>__<tool>` and `mcp__plugin_<plugin>_<server>__<tool>`. The `git` tool runs commands as `shell` does, so a matcher naming `shell` (and so `Bash`) covers it too, and its `tool_name` is `Bash` with the whole command as `tool_input.command`. `Skill` isn't a tool here, but its hooks run when a skill is taken up, with `tool_name` `Skill` and `tool_input.skill` as `plugin:name`: when the agent reads a `SKILL.md` Themis put under `.themis/` for the run (the catalog it picks from, or a plugin's files), and for a skill you attach with `/` or a plugin you mention whose skill is attached, before the first request. They run once a run for each skill. A `PreToolUse` hook that refuses refuses the read, or stops the turn for an attached skill before the model sees it. A project's own file named `SKILL.md` isn't a skill. A name with no Themis tool is listed as a skipped part, and a matcher with nothing left is not imported. Any other pattern (`Bash.*`, `(Edit|Write)`, or a regular expression that doesn't compile, such as one with lookaround) is skipped whole, so a guard never matches nothing or everything by accident.

### How imported Claude hooks run

A hook's input has Themis's fields and the ones Claude scripts read: `hook_event_name`, `session_id` (the chat's id), `cwd`, `tool_name` (`Bash`, `Read`, `Write`, `Edit`, `Grep`, `LS`, `WebFetch`, `WebSearch`, `Agent`, or `mcp__<server>__<tool>`), `tool_input` and `tool_response`. `tool_input` is the Themis tool's arguments, with a shell or `git` call's whole command as the string `command` and a file as `file_path`. `permission_mode` is `default`, or `bypassPermissions` in YOLO. `transcript_path` is the chat's transcript in Claude's format, one JSON object a line (`app data/hook-sessions/<chat>/transcript.jsonl`, readable only by you, kept while the chat has enabled hooks, its older half dropped past 8 MiB, deleted with the chat). It holds your messages and tool results, so it is handed to hooks and never logged; a hook runs outside the sandbox with your permissions either way. A subagent's hooks get its own transcript path. `stop_hook_active` is on `Stop` and `SubagentStop`. `CLAUDE_PROJECT_DIR` is set beside `CLAUDE_PLUGIN_ROOT`, and `CLAUDE_PLUGIN_DATA` is the plugin's own folder in app data, kept across updates and removed when it is uninstalled.

`CLAUDE_ENV_FILE` is set for `SessionStart` hooks, and what they write reaches that plugin's own later hooks in the turn: not the agent's commands, and not another plugin's hooks. A variable can make a later command run other code, and a list of dangerous names can't be complete, so the rule says what is allowed: names under the plugin's own prefix, its name in capitals with other characters as `_` (`my-kit` may set `MY_KIT_ANYTHING`). Anything else is refused, in lower case or not, and so is a value that would need a shell (`$`, a backtick, `;` and the like): the file is read, never run, at most 64 variables and 64 KiB. The plugin's later hooks get the refused names in `THEMIS_ENV_REFUSED`, separated by commas, and `hook` / `env_refused` logs how many. This differs from Claude, where the variables reach Bash commands. In the official catalog the one plugin whose hooks write this file is `vercel`, with `VERCEL_PLUGIN_*` names it reads back in its own later hooks; plugin-dev only documents it, with examples like `PROJECT_TYPE` that this refuses.

A guard written for Claude looks at one `file_path`, so an imported hook runs once for each file a call touches, with that file as `file_path` and all of them in `file_paths`: every file in a patch, deleted ones too, and both ends of a move. If any run refuses, the call is refused and nothing is written. More than 64 files is refused outright rather than half checked. `Write` and `Edit` matchers (`write_file`, `apply_patch`) also cover `copy_file`, `move_file`, `delete_file` and `create_dir`, which write files as well. Exit code 2 refuses, with stderr as the reason, for imported hooks only (not native ones) and not for `SessionStart`, which can't stop a turn in Claude either. A blocking `PreToolUse` hook also blocks on any other failure, as it always did. A hook gets the timeout it asks for up to 600 seconds, and 60 when it asks for none (a native hook's default stays 10); your Stop ends one still running rather than waiting. A guard that runs out of time blocks.

Options on a command hook:

| Option | What happens |
|---|---|
| `statusMessage` | Ignored: it is only the text of Claude's spinner. |
| `async: false`, `asyncRewake: false` | Ignored: they are the defaults. |
| `async: true` | Stored as `background`. The hook starts and nobody waits; it can't block or add context, as in Claude, and it is asked about before it starts like any hook. Eight run at once at most; past that one is skipped and logged (`hook` / `background_skipped`). Its time limit bounds how long it can outlive the run. |
| `asyncRewake: true` | Runs like `async`, with up to 600 seconds unless it asks for less, and Stop ends it. When it exits 2, what it wrote (stderr, else stdout; at most 16 KiB) reaches the agent; see below. `rewakeMessage` goes above it, and `rewakeSummary` (or a `rewakeSummary` in a JSON line on stdout) is what you see. Any other exit says nothing. |
| `shell: "bash"` | Stored as `shell` and run with `bash -c`. If `bash` is missing the hook fails, so a guard still blocks. Other shells are skipped. |
| `args` | Joined onto the command, quoted so each reaches the program as written; only `${CLAUDE_PLUGIN_ROOT}` is filled in. Skipped on Windows, where hooks run in `cmd`. |
| `commandWindows` | Ignored outside Windows, where it never applies. On Windows the hook is skipped: these commands are written for PowerShell. |
| `if: "Bash(git push *)"` | Stored as `command_prefix`. The hook runs only for a shell or `git` tool call (background or not) running that program with those words among its arguments in order, which also finds `git -C . push` and `sh -c "x && git push"`. A wrapped script that can't be read (a variable as the program, say) and arguments that can't be read count as a match. `background_task` starts nothing (it lists, reads and stops what `shell` started), so only a call that doesn't read as one of those runs the hook. Script files and other interpreters aren't read, as for deny rules. Patterns inside words (`Bash(git*commit*)`) are matched on the command and its arguments joined by spaces, `*` being any text, and read the same way. Another pattern character, a condition with nothing but `*`, or one for another tool is skipped. |
| Anything else | The hook is skipped and the option is named. |

What a background hook reports is the plugin's word: the agent gets it fenced and labelled as not from you, not instructions and granting nothing, and the chat shows a row for it. At most four notes wait per chat, the oldest going first (`hook` / `rewake_dropped`), and they don't outlive the app, whose stopping ends the hook too. If a turn is running it reads the note at its next step. If the chat is idle the hook may start a turn, which shows as **Plugin hook** rather than a message of yours, and whose message is the runtime's fixed text: nothing the hook wrote goes through the path of your messages, so it can't attach a skill or a plugin or run a `/` command. The report reaches the agent only as the labelled note, and you see the plugin's summary in a row in the chat. Such a turn happens: at most `hooks.max_wakes` (3 unless `runtime.jsonc` says; 0 for never) in a row until you write again, and none after a turn that failed or that you stopped, in YOLO (nobody would be asked anything, with everything open), or while a goal runs (its next turn reads it). In each of those the note waits for the chat's next turn (`hook` / `rewake_waits` says why). Woken turns keep the chat's approvals, so a tool that asks still asks. This is narrower than Claude, which wakes without a limit of ours: the review plugins that use it say exit 2 is rare and ask to be acknowledged, and a loop that reports every turn stops at the limit.

`prompt`, `agent` and `http` hooks are skipped ("prompt hook has no Themis equivalent"): nothing in Themis asks a model or a web address whether a tool may run.

`Stop` runs when a turn would end on a reply, before the run finishes, with `last_assistant_message` and `stop_hook_active` (true once a hook has kept this turn going), for the chat's run and not a subagent's. A hook answering `{"decision": "block", "reason": "…"}`, or exiting 2 with the reason on stderr, keeps the turn going: the model gets the reason (at most 32 KiB, fenced and labelled as the plugin's, not the person's, and granting nothing) as its next message. That is how a loop plugin works, so nothing stops it but the hook letting go, the run's turn limit (each round is a request, so `total_turns` bounds it) or your Stop. A hook that merely fails doesn't keep anything going. Each round logs `hook` / `stop_kept_going`. It doesn't run when the run failed or you stopped it.

Events: `StopFailure` imports as `RunFailed`, which runs at `RunEnd` only when the run failed (not when you stopped it). These stay skipped because their meaning doesn't hold:

- `SessionEnd`: a Themis chat has no end to attach to. A Themis run is one turn of a chat, not a Claude session: `SessionStart` runs on every turn because its output is context the turn needs, but running `SessionEnd` scripts (uploads, cleanup of session state, consolidation) after every turn would repeat or undo them.
- `UserPromptExpansion`: nothing marks a `/` skill being expanded, and a guard reading the expansion's fields would find nothing on `PromptSubmit` and let it pass.
- `PermissionRequest`, `PermissionDenied`: these can answer an approval prompt, and hooks never grant permissions.
- `PostToolBatch`, `PostCompact`, `FileChanged`, `CwdChanged`, `ConfigChange`, `InstructionsLoaded`, `Notification`, `Elicitation`, `ElicitationResult`, `TaskCreated`, `TaskCompleted`, `TeammateIdle`: Themis has no such moment.

## Signing in to remote MCP servers (OAuth)

A remote server that answers with a sign-in challenge (HTTP 401 with `WWW-Authenticate:
Bearer`) shows **Sign in needed** in Integrations › Connections with a **Sign in** button. Themis
follows the MCP authorization spec: it reads the server's protected-resource metadata
(RFC 9728) to find its authorization server and that server's metadata (RFC 8414), registers
itself as a client (dynamic client registration, RFC 7591), and opens the sign-in page in your
own browser with PKCE (S256) and the `resource` parameter (RFC 8707). A one-time listener on
`127.0.0.1` receives the result (five minutes at most); the tokens are stored in the Keychain
(never in configuration files) and refreshed when they expire. **Sign out** forgets them.
Sign-in and sign-out are offered only for servers you installed. A server whose sign-in needs a
client registered by hand (no registration endpoint, and no client named in its plugin) is reported
as such; use a `Bearer ${NAME}` header for it instead. A server whose own key header has a value
never signs in; a header written `${NAME:-}` doesn't count until it has one, so a plugin like that
takes a key or a sign-in.

An `oauth` block in a plugin's configuration is honoured, and the sign-in happens after installing:

| Field | What it does |
|---|---|
| `clientId` | The client to sign in as, instead of registering one (a client registered with the provider, or the address of a client metadata document). The page comes back to `http://localhost:PORT/callback`, since a registered client is held to the exact address it was registered with; a client registered by Themis itself uses `http://127.0.0.1:PORT/callback`. |
| `callbackPort` | The port the listener takes; a port already in use is reported before any page opens. Absent, any free port. |
| `scopes` (`scope` in OpenCode's form) | What to ask for, in place of the server's `WWW-Authenticate` scope and then its `scopes_supported`. |
| `authServerMetadataUrl` | Where the authorization server's metadata is (HTTPS, or HTTP on this computer), in place of discovery. |
| `clientSecret` | A secret holding the client's secret, as `${NAME}`. |

Any other field keeps that connection out, and is named. The token asks for the protected resource
the server's metadata names when that covers its address (`https://mcp.slack.com` for
`https://mcp.slack.com/mcp`), and a refresh names the same one; metadata about another server is
refused. A preset client its provider registered for Claude (`render`, `mongodb-atlas` and `slack`
ship one) keeps that connection out, with the reason named: Themis doesn't sign in as another app.
The rest of those plugins still installs, and a client the provider registers for Themis works like
any other.

How far each catalog plugin's real provider was checked, without signing in to anything:
[integrations verification](integrations-verification.md#mcp-sign-in-providers).

## Keys for MCP servers

A server reads a key as `${NAME}`, the same placeholder MCP documentation uses: in `env` for a
local server (`"env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"}`) or in a header for a
remote one, as an optional literal prefix followed by one reference (`"Authorization": "Bearer
${API_KEY}"`, `"X-Api-Key": "Token ${API_KEY}"`, `"CONTEXT7_API_KEY": "${CONTEXT7_API_KEY}"`).
Header names are letters, digits, `-` and `_`; `Accept`, `Content-Type`, `Content-Length`,
`Host` and the MCP session headers are reserved, and `Authorization` may appear once. When the
server starts, each `${NAME}` is the secret named NAME in **Settings › Secrets**, or else the
environment variable of that name (a saved key wins). Keys are stored in the system credential
store (Keychain, Windows Credential Manager, Linux Secret Service, the same store as sign-ins);
app data `keys.json` keeps only the names, because those stores cannot list entries. A server
whose secrets are missing shows **Secret needed** with **Add secret**, which opens Settings › Secrets
on that name; every missing name is reported at once. A header written `${NAME:-}` is optional: left
empty it isn't sent and the server doesn't wait for it.

A definition can name any secret in Settings › Secrets, or any variable of the app's environment: the
key of a model provider or of another plugin as easily as its own. So a plugin's connection reads a
secret only once you allowed that name for that place. With its secrets all present it shows **Needs
your OK**, "Sends `OPENCODE_KEY` to `evil.example`" and **Allow**, and nothing has been sent until you
do (nor can a sign-in start). The place is where a connection would really go: the host and any port as the URL parser reads them
from the address with its values filled in, or for a local program its name, held to its command,
arguments, folder and the plugin's files. When it changes, also because a value in the address did, or
a new secret is read, it asks again. A secret where the host goes, or a value that looks like one, is
refused outright, since connecting would hand it to name lookup; redirects are never followed. An agent can't allow it, and a test it asks for
(`test_mcp`) can't read secrets at all. Connections you wrote yourself (with no plugin package,
catalog or repository behind them) don't ask, since you named the secret. An update that changes
headers, plain values, defaults or `oauth` of a connection turns it back off until you enable it
again, as a change to its command or address always did. A `clientSecret` is also held to the
server's own host or an authorization server it declares: Themis won't send it to a token address a
plugin's own metadata names elsewhere, and doesn't follow redirects from one.

A plugin that ships a literal value where a key goes (in `env`, a header, `clientSecret`, a `:-default`
of a key, or a stand-in such as `[your aws profile]`, `your-api-key` or a value that just repeats the
field's name, like `cld-cloud-name: cloud_name`) doesn't get to keep it. The literal becomes a
reference to a named secret, `PLUGIN_FIELD` in capitals (`cloudinary` and `cld-api-key` give
`CLOUDINARY_CLD_API_KEY`; another value for the same field in the same plugin takes the server's name
too), and the value is **discarded**: it is rewritten in the plugin's copies in the registry and the
package, and a value that isn't a stand-in is also taken out of the plugin's other text files under
1 MiB, so neither the model nor a process can read it there. The source folder isn't touched, and
neither is a marketplace's own clone of a plugin kept inside its repository, whose contents the
repository published. The import says which secret each one became, in names only. Same goes for an
address shipped empty (`"url": ""`): it reads a secret named for the server, since each person has
their own. A literal is refused in a query string, in a value mixed with a reference, in a
marketplace entry's inline definition, and in anything you paste or type (`themis mcp import`, Edit as
mcp.json): there you hear about it at once rather than having what you wrote dropped. Settings › Secrets lists each saved secret and each secret
an installed server reads, with the servers that use it and its state (Saved, From environment,
Missing), and offers Add or Replace (a masked field), Remove, and a new name. The secret
OpenCode Go uses (Settings › Models › Providers, `OPENCODE_KEY` by default) is always
listed with "OpenCode Go" among its users, and the secret of each provider in `runtime.jsonc`
with that provider's name, so it works in MCP configuration as `${NAME}` too. Values are never
returned to the UI or written to logs; a server's stderr is redacted of the keys it was given.
Names are letters, digits and `_` (up to 128); values are 1–4096 printable characters without
spaces. The model provider key stays in Settings › Models.

References may also be written `${env:NAME}` or `{env:NAME}`. Claude plugins' install-time settings, `${user_config.option}`, read the secret named after the option in capitals (`${user_config.bigquery_project}` reads `BIGQUERY_PROJECT`), so they are entered in Settings › Secrets like any key. An `env` reference may carry a default, `${NAME:-value}`, used when the secret isn't saved or set; a default must be a plain value as below. A remote server's URL may contain references, including at the start (`${JFROG_URL}/mcp`); it is checked against the HTTPS rule once the references are filled in, when the server starts.

Plain values are allowed where they can't be credentials: `env` values and headers whose name doesn't suggest a credential (no `key`, `token`, `secret`, `password`, `auth`, `cookie`, `session`, `private`, `cert` or similar), at most 256 printable characters, with no run of 20 or more characters mixing letters and digits and no user name or password in a URL. That covers settings such as `"FASTMCP_LOG_LEVEL": "ERROR"` and tags such as `"x-exa-source": "claude-code-plugin"`. URL queries follow the same rule for each parameter (`?client=claude-code-plugin` is fine, `?api_key=…` is refused). `${CLAUDE_PLUGIN_ROOT}` and `${PLUGIN_ROOT}` in the command, arguments or plain `env` values become the installed plugin's folder when the server starts; they are never keys. The same rules apply to imports, Integrations › Connections › Edit as mcp.json and the CLI. Defaults may also appear in URL and header references (`${LOGFIRE_MCP_URL:-https://logfire-us.pydantic.dev/mcp}`).

Imported servers may also carry `description`, `note`, `_note`, `_comment`, `_meta` and `disabled`, which have no effect (imported servers start disabled), and `timeout` and `autoApprove`, which are listed as notes because Themis's own timeouts and approvals apply. A plugin's `userConfig` declaration is noted, since its settings are entered in Settings › Secrets. Preset `oauth` clients are still unsupported. `cwd` is where a local server runs: relative to the project, or `${CLAUDE_PLUGIN_ROOT}/…`, and it must stay inside the project; a missing folder fails the connection.

## Programs MCP servers start with

An app opened from Finder, the Dock or a desktop launcher gets the system's minimal `PATH`.
At startup Themis adds your login shell's `PATH` (three-second limit), so a server whose
command is `npx`, `uvx`, `docker` or a program in `~/.local/bin` starts as it does in your
terminal. A program that still cannot be found is named in the connection details ("'npx'
was not found on this computer"); install it or save the command with its full path.

## Edit as mcp.json (Integrations › Connections)

**Open** writes your personal (non-marketplace) global MCP servers to one copy,
`mcp-edit/mcp.json` in the app data folder, in the standard `mcpServers` shape that Claude
Desktop, Claude Code and Cursor use, keyed `integration/server`: local servers as
`{"command": "npx", "args": [...], "env": {"KEY": "${NAME}"}}`, remote ones as
`{"type": "http", "url": "...", "headers": {"Authorization": "Bearer ${NAME}"}}` (any key header).
The list starts with commented-out examples, one per way to authenticate (a local server with a
key, a remote server with a key header, a remote server that signs in); remove the `//` to use
one. Whether a
server runs is not in the file (switch it in Integrations; an `"enabled"` you add is used),
and opens it in the editor chosen under **Open with** (the same choice as Settings ›
Permissions). While it is open Themis watches it: each save is parsed as JSONC and every
integration whose servers changed is saved through the integrations store with the revision
the copy was made from, exactly as an edit in the CLI. The row then shows **Applied** with the
integrations changed, **Invalid · line N** (nothing is applied), or a **conflict** when that
integration changed elsewhere since the copy was opened (open it again). Adding a server under
an existing personal integration name adds it, and a new name makes a new personal integration;
removing one removes it; a marketplace integration's name is refused. As with any edit, a changed command, arguments or URL does
not start until you enable it again. Marketplace integrations are not listed: make a personal
copy to edit one. The CLI uses the same methods: `open_mcp_config` (with `"open": false` to
only write and watch the copy), `apply_mcp_config` and `mcp_config_status`.

## CLI

```sh
themis plugin list
themis plugin create personal --scope local --project /path/to/project
themis plugin show personal --scope local --project /path/to/project
themis plugin edit personal --scope local --project /path/to/project
themis plugin marketplace list
themis plugin marketplace preview owner/repository
themis plugin marketplace add owner/repository
themis plugin marketplace browse claude-plugins-official
themis plugin marketplace check claude-plugins-official
themis plugin inspect /path/to/package --scope global
themis mcp inspect /path/to/mcp.json --scope global
themis plugin import /path/to/package --scope global --allow-partial
themis plugin install PACKAGE@claude-plugins-official
themis plugin update PACKAGE
themis plugin disable PACKAGE
themis plugin uninstall PACKAGE
themis skill create THREAD_ID 'Create a global code-review skill'
themis skill verify /path/to/skill.json
themis skill save personal /path/to/skill.json --scope local --project /path/to/project
themis mcp add personal docs /path/to/mcp.json
themis mcp test personal docs --project /path/to/project
themis hook add personal guard /path/to/hook.json
themis hook enable personal guard
themis hook test personal guard --project /path/to/project
```

`mcp` and `hook` also support list, show, update, disable, and delete; skills support list/save/delete. `plugin save`/`import` accept a complete JSON bundle; export writes JSON to stdout. Edits use `$EDITOR` and compare the original revision before saving. Changes create a new revision while active runs preserve their immutable snapshot; changes apply on the next run.

## Verification (2026-10-01)

Fresh local commands and outcomes:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: hit the host-dependent synthetic Keychain test failure (`The user name or passphrase you entered is not correct`).
- `cargo test --workspace -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: 224 passed, no failures; one Keychain test filtered; live-provider test and illustrative doctest ignored.
- `cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info -- --skip entered_keys_survive_new_store_and_can_be_forgotten`: passed; Rust line coverage 83.1% (11,071/13,328 lines). The prior local report was 85.8%; the new plugin runtime lowers aggregate coverage. No frontend coverage report is available.
- `npm --prefix web run lint`, `npm --prefix web test`, `npm --prefix web run build`: passed; 171 frontend tests. Vite still reports chunks above 500 kB and Vitest reports its existing deprecated environment configuration.
- `ruff check quality_dashboard` and `python3 -m unittest quality_dashboard.test_dashboard`: passed, 13 dashboard tests. Product LOC now includes the built-in Python verifier.
- Native debug app bundle built with an isolated `ai.themis.plugins.qa` identifier. Native checks verified Plugins naming, plugin/component persistence, creator autocomplete/inline insertion without sending, automation inline skill save/reopen, and a synthetic hook test. QA automation was saved disabled; no live provider request was made.
- Live CLI browsing of the default official marketplace succeeded; catalog metadata was retrieved without installing a package.
- The CLI plugin E2E drives the actual `themis` binary against an isolated server. The creator request-path integration test captures the mocked LLM request and confirms creator instructions, management tools, and materialized verifier. Python acceptance/rejection and directory validation run as real subprocesses.

The running dashboard was refreshed through `/api/metrics`: 93 AST-analyzed app files; average cyclomatic 2.46, cognitive 2.20, maintainability 73.8. Reviewed new core files: hooks 4.42/3.50/58.5, marketplace 5.93/4.86/66.4, MCP 4.00/4.52/67.9, registry 2.89/1.65/70.1 (cyclomatic/cognitive/maintainability). These are analyzer metrics, not readiness scores. Live agent-generated skill quality, third-party MCP/OAuth behavior, and native history recall during a completed live run remain unverified. History recall has an automated editor regression test.

## Real upstream acceptance (2026-10-06)

Run the network acceptance explicitly:

```sh
cargo test -p themis-core --test plugin_imports real_public_plugin_and_skill_import_acceptance -- --ignored --nocapture
```

The isolated test imports public sources at exact commits; it does not install into the application's user configuration:

- Superpowers `8ca22dba9a94f28898bbce59f2537ff4d87c747d`: 15 skills imported; two compatibility notices; Claude lifecycle matcher/options excluded. Its reviewed upstream session-start script executes through an explicit native `RunStart` adapter and returns valid JSON. Claude-specific context-injection output is not interpreted as a portable hook contract.
- Anthropic skills `683bc88e56f3e09ba94f7055977f3d3aa499f202`, `skills/pdf`: one skill and 12 text resources imported. At this historical check, skill-creator was rejected by the former 32 KiB limit. The current 256 KiB instruction limit accepts its 32,805-byte body; the pinned acceptance test now checks full import.
- Official Claude plugin catalog `d4226d062928f8d9505dbdeadd10217d23361052`: catalog read and `plugin-dev` installation passed; seven skills and nine compatibility notices.
- Official filesystem MCP npm package `@modelcontextprotocol/server-filesystem@2026.8.31`: imported disabled, explicitly enabled, launched, negotiated the supported MCP protocol, discovered 14 tools, and read an isolated marker through the actual `read_file` tool.

These checks verify supported imports and executable integration paths; they do not establish complete compatibility with upstream plugin runtimes, all skill workflows, every OAuth provider, or current MCP specification revisions.

Standalone skills appear under Skills and are invoked with `/`. Plugins contains real plugin packages (including packages with one bundled skill) and uses `@`. Standalone MCP JSON and hooks stay in their respective categories. Imports record `package_kind`; older imports are classified from preserved source metadata/resources without rewriting the source files.

Plugins has Installed and Discover views. Skills lists installed, bundled, and locally discovered skills; it has no separate Discover catalog. Disabled items remain installed. Discover new packages through Plugins. Standalone skills remain available through repository/folder imports and normal chat. Plugin/skill rows use supplied package icons, with file-type glyphs when the source supplies no artwork.

The integration browser uses searchable cards, icons, status labels and readable details. Discover shows every marketplace together. Its toolbar has search (name, description and publisher), a Status filter (Compatible — fully working first, then partly working — Fully compatible, Partially supported, Unsupported, Check failed, Not checked, Installed, All statuses) and a Source filter (All sources or one marketplace); **+** adds a marketplace. Each marketplace's plugins appear as soon as its own check is ready, while "Checking catalog" shows the others are still running. Group chips with counts sit above the cards: All, Coding, Cloud & deploy, Data & databases, Monitoring, Security, Work apps, Design and Other. The official catalog's groups come from the bundled `crates/core/builtins/plugin-groups.json` (`themis plugin marketplace check` lists any entry it doesn't cover); other marketplaces use each entry's catalog `category`, else Other. The chosen group is remembered on this computer. Cards show the plugin, its publisher and marketplace, its description and what it adds ("Connects to airtable · 8 skills"). Fully working plugins carry no badge; a partly working one shows a collapsed "Some parts skipped (N)" warning, an unsupported one "Unsupported (N)" and a failed check "Check failed". Catalog installations always use the global user registry and are available to every project. Existing project-scoped integrations remain visible with their scope; advanced CLI imports retain explicit local support. Personal includes local/discovered sources; Public identifies remote repository or marketplace origin, not an independent assertion about repository access permissions. Status filters match the labels: Compatible (green), Partially supported (amber), Unsupported (red) and Check failed (red), and Not checked (neutral). Available includes only Compatible and Partially supported packages; Check failed and Not checked have their own filters; All statuses includes them. Installed filters completed global installations. Installed items filter by Enabled or Disabled. Color always accompanies a text label. Plugin View lists its actual skills and View actions. Skills render as formatted Markdown. MCP details show the complete saved command and observed connection state. Advanced configuration, diagnostics, and execution tests use the shared chat tools or CLI. Basic lifecycle controls use clear on/off switches, public Install buttons and confirmed Uninstall actions. Advanced management also runs through `/plugins`, `/skills`, `/mcp` and `/hooks` in chat, or the CLI. Automations remains separate with Instructions, Repeat and Time, plus optional controls under Advanced.

## Setup and operational status

Installed and Enabled describe storage and activation, not service health. MCP rows show Connecting, Connected, Failed or Disabled. Imports leave executable components disabled; enabling a connection authorizes automatic background initialization. Inspect environment/dependency prerequisites and test connections through the existing chat/CLI workflow. A successful explicit MCP test is evidence for that invocation only. Marketplace scans run in the shared backend and save their results. New marketplaces remain unavailable until every entry has been checked; failed entries show a reason and cannot install. Standalone skill imports use the same validation; bundled skills retain their parent source.

Catalog tiles reuse persisted full-marketplace checks. Scans check up to four packages concurrently per marketplace and save each completed result; navigating away does not cancel them. Compatibility describes import support, not a successful authenticated runtime test. Refresh retries failed checks. Plugin View shows only bundled skills and their View actions; installation limitations appear only when installing a partially supported package. The [design rules](../DESIGN.md) define copy, status colors, filters, and interaction requirements.

## Saved compatibility scans

`scan_marketplace` starts or resumes background scanning and returns `scanning`, `ready`, or `failed`, plus completed/total counts, the catalog and the checks finished so far. Adding a marketplace starts its scan immediately. Each entry's check is saved with its own `check_revision`, so a checked plugin can be previewed and installed while the rest of its catalog is still being checked; unchecked entries show "Checking…". Browsing and manual preview share the same saved result. Individual import failures remain visible as Check failed and disable installation until refresh.

Derived metadata and full previews are stored under app-data `compatibility-cache/<marketplace>/`, separately from the revisioned integration registry. They persist across navigation and server restarts. Interrupted work resumes on the next scan/browse/preview request, retaining completed entries. Refresh explicitly replaces the snapshot; source changes and cache schema changes invalidate it. Catalogs are limited to 2,000 uniquely named entries; individual persisted files retain the existing 32 MiB store limit. No MCP server or hook executes during scanning.

A per-marketplace OS lock allows one scan at a time and blocks removal while it runs (a retryable busy error). Previews and installs don't take it: an entry's saved check is written atomically, and the catalog check goes on around it. Desktop installs include the reviewed `check_revision`: if that entry was checked again, installation stops and asks the user to reopen the plugin. The validated saved package is installed, so a later upstream edit cannot change the approved contents. Updating through the CLI explicitly refreshes the full marketplace first. Previously installed packages remain available during scans.

## Icons

A plugin's icon is, in order: its own icon (a bundled SVG, PNG or WebP up to 256 KiB, or an HTTPS URL); a bundled brand logo; a tile with its initial on a colour from its name. HTTPS icons are downloaded once, during the compatibility check, and browsing makes no network requests. Saved icons live in the content-named app-data `plugin-icons/` folder, so an installed plugin keeps its icon after its source is removed.

Brand logos come from [Simple Icons](https://simpleicons.org) (CC0; the marks themselves remain their owners' trademarks), limited to publishers in the catalog (`web/src/lib/brandLogos.json`, `crates/core/builtins/plugin-brands.json`; a test keeps the set under 300 KiB). A logo is matched on the GitHub owner in an entry's source URL, or on `author` in the official catalog only, because `author` is just a claim; never on an MCP domain. Saving a plugin can't set either field.

## Updates

When the app opens, each shown source not checked in the last day is checked for upstream changes in the background (Settings › General › Check sources for updates, on by default). A Git source compares its latest commit (`git ls-remote`) and downloads again only if it moved; an entry in its own repository compares that repository's ref, unless the catalog pins a `sha`; a folder compares a bounded hash of its files, not modification times. Only entries that changed are checked again. If a source can't be reached, its saved checks stay and it is tried again next time.

An installed plugin whose upstream package differs from what was installed shows the version change and Update. Updating installs the newly checked package and is always your choice. Connections and hooks that are new or changed arrive turned off; unchanged ones keep their switch. `themis plugin updates` lists them, and `themis plugin marketplace changes NAME` looks for changes now.

## Sources

Discover › Sources lists every marketplace with its plugin count and when it was last checked, or its progress. Add marketplace takes one field: `owner/repo`, an HTTPS Git URL or a local folder. Check reads only the source's manifest (for Git, a shallow, sparse, blob-filtered clone of `.claude-plugin/marketplace.json`) and shows its name, publisher and plugin count before anything is added; Add uses the manifest's name. A source that can't be reached, isn't a marketplace or lists more than 2,000 plugins says so in one line. A switch shows or hides a source in Discover (`enabled` in `marketplaces.json`, written under the marketplace lock). Removing a source deletes its saved checks and keeps plugins installed from it; their rows show "Source removed" and they can no longer be updated. The official source can be hidden but not removed.

Raising the instruction limit or changing import rules invalidates older saved marketplace checks automatically (the saved check carries a version). `themis plugin marketplace check NAME` waits for a complete check and prints how many plugins have each status (`supported`, `partial`, `unsupported`, `failed`) and how many plugins each reason affects, most common first; `marketplace refresh NAME` fetches the source again first. Existing installed packages and their revisions remain unchanged.

The active Installed/Discover view has a persistent accent underline and a brief selection shimmer (static under reduced motion). Marketplace scanning shows the interactive companion in the center of the content area with a progress ring and completed/total percentage; catalog retrieval is indeterminate until the total is known. Compatibility warnings are collapsed by default with an icon, detail count and amber emphasis. Expand them to read full reasons, offending fields and remedies where available. Rejected local skills remain visible as Unavailable with the validation reason and no activation control.

Failed scans retain the complete validation error chain instead of only the outer component name. Older compatibility snapshots are automatically rechecked for the more informative results.

MCP rows and details show the saved executable and all arguments, with shell quoting for spaces, quotes and empty arguments. Remote entries show their endpoint. Status, activation switch and uninstall action share one compact row. Enabled connections initialize in the background when the app opens or the selected project changes. The app stays usable while each connection starts or fails. Refresh retries failed connections. The shared backend retains transports by project, component and saved revision; chat runs reuse them with their own approval hooks. Connected means initialization and tool discovery succeeded; it does not continuously probe an idle remote service. Local process exit and transport-call failure change the status to Failed.

Transient integration errors and installation confirmations use the shared side toast with its six-second timer and dismiss action. Compatibility details remain attached to the affected item as collapsed warnings.

Background startup is bounded to 30 seconds per connection. Disabled connections never start; explicit user/project approval denials block automatic startup. Enabling authorizes connection setup only, not tool execution. Changes reconcile existing connections; old transports referenced by active runs retain their snapshot until the run releases them. Connections are held in memory by the shared app server, without a second writable registry.

Open an MCP connection’s details to see its exposed tool count, names and descriptions. This reads cached discovery metadata; it does not execute a tool. Tools appear after the connection completes.

Agent inventory (`list_plugins` and `manage_integrations` with `action: "list"`) returns metadata only: `{total, counts: {mcp: {total, enabled}}, items, next_offset}`. It accepts `kind` (`plugin`, `skill`, `mcp`, `hook`), `offset` (default 0), and `limit` (default 25, range 1–100). Totals/counts cover the filtered inventory before pagination; `total` counts packages, while `counts.mcp` counts server definitions. An enabled server requires both its package and component enabled; this does not establish connection health. File bodies, instruction bodies, scripts and MCP environment/command configuration are omitted. Full package inspection remains available through the existing UI/CLI `plugin_action` list API; `plugin_action` with `action: "inventory"` exposes the compact result for local shell processing. Results from inspection or mutation still pass through the shared bounded agent delivery boundary described in [context checkpoints](context-and-attachments.md).
