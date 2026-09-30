# Plugins and prompt skills

The desktop Plugins screen manages reusable skills, MCP connections, hooks, and marketplace sources. Personal plugins have local project scope (`.themis/plugins`) or global app-data scope. App and CLI use the same local server and revisioned store.

## Prompt workflow

Type `/skill` to choose a skill or `/create-skill` to ask the agent to design one. The editor renders an icon and skill name; persisted prompts contain `[[skill:ID]]`. Installed plugin references include their scope and revision, so marketplace updates preserve saved prompts. Disabled or uninstalled plugins cause an explicit unavailable-reference error. Old references keep their labels but are excluded from new autocomplete selections.

In a chat, Up/Down on the first/last line recalls previously submitted prompts and restores the current draft. Arrow keys select slash suggestions when autocomplete is open. Enter selects a suggestion before it can send a chat; Shift+Enter inserts a newline.

Automations select skills in their task prompt. Continuing automations use the thread's current model and effort, but do not inherit its attached skills. Legacy standalone automation skill IDs remain supported and are migrated into prompt references when edited.

## Agent skill creation

The built-in creator owns `crates/core/builtins/create-skill/SKILL.md` and `scripts/verify.py`. Its instructions tell the agent to write a JSON draft, run the materialized verifier at `.themis/skills/create-skill/verify.py`, test helper scripts, then call `save_skill`. These management tools are available only when the creator is selected. Local scope is the default; global scope must be requested. Backend validation and optimistic revision checks also run on every save.

Run the standalone verifier:

```sh
python3 crates/core/builtins/create-skill/scripts/verify.py /path/to/skill.json
python3 crates/core/builtins/create-skill/scripts/verify.py /path/to/skill-directory
```

It checks required fields, names, instruction/script limits, known tools, duplicate scripts, traversal, and directory symlinks. It does not evaluate whether instructions solve the task. Python 3 is required for this check.

## Marketplace compatibility

The default source is Anthropic's `claude-plugins-official` Git repository; individual packages are installed by the user. Additional HTTPS Git repositories or local marketplace directories can be added. Imports read Claude marketplace/plugin manifests, `SKILL.md`, UTF-8 resources, `.mcp.json`, and command hooks. Imported MCP servers and hooks begin disabled. Make a personal copy to customize a marketplace package without losing changes on update.

Unsupported components are listed in the plugin detail view. Binary resources, legacy commands, Claude subagents/LSP/settings, external config-file declarations, and restricted tool matchers are not executed. The importer supports simple scalar/block YAML frontmatter. Source ref/path overrides are rejected rather than silently ignored. This is compatibility with supported components, not the full Claude plugin runtime.

## MCP and hooks

Connections support stdio and Streamable HTTP JSON-RPC, initialization, paginated tool discovery, and tool calls. Configure environment-variable references for stdio and optional bearer authentication; credentials are not stored as literal configuration values. Browser OAuth and legacy SSE transports are not implemented. Each selected plugin's enabled connection startup and tool calls use Themis approval gates.

Built-in hook events are `RunStart`, `BeforeToolCall`, `AfterToolCall`, `ToolCallFailed`, `RunFinished`, and `BeforeCompaction`. Commands receive a JSON payload through stdin and may return JSON. Before-event hooks can block on failure. Hooks cannot grant tool permissions. Commands have bounded output/time; Unix process groups terminate descendants on timeout/cancellation. Imported resource roots are passed to hooks as `CLAUDE_PLUGIN_ROOT`, never substituted into shell source. Test buttons explicitly run the displayed configuration; normal runs request approvals.

## CLI

```sh
themis plugin list
themis plugin create personal --scope local --project /path/to/project
themis plugin show personal --scope local --project /path/to/project
themis plugin edit personal --scope local --project /path/to/project
themis plugin marketplace list
themis plugin marketplace add community https://github.com/owner/repository.git
themis plugin marketplace browse claude-plugins-official
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

`mcp` and `hook` also support list, show, update, disable, and delete; skills support list/save/delete. `plugin save`/`import` accept a complete JSON bundle; export writes JSON to stdout. Edits use `$EDITOR` and compare the original revision before saving. Application management rejects mutations during active runs; agent creation saves a new revision while preserving the running snapshot.

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
