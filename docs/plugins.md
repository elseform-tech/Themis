# Plugins and prompt skills

The desktop Integrations screen browses plugin bundles, skills, MCP connections, hooks and marketplace sources. It provides basic install, enable/disable and uninstall controls. Chat management skills and the CLI handle creation, configuration, testing and troubleshooting through shared validated APIs. Personal plugins have local project scope (`.themis/plugins`) or global app-data scope. App and CLI use the same local server and revisioned store.

## Prompt workflow

Type `/` to choose a skill or `@` to choose an enabled installed plugin. A plugin mention binds its bundle and exposes enabled skill metadata, including manual-only skills, without injecting every instruction body. Skill selection remains explicit. Type `/create-skill` to ask the agent to design one. The editor renders an icon and skill name; persisted prompts contain `[[skill:ID]]` or `[[plugin:g--NAME]]` / `[[plugin:l--NAME]]`. Disabled or missing plugin mentions fail explicitly. New plugin references use stable scope/plugin/skill identities; plugin version and saved revision remain separate. Existing revision-pinned references still resolve their original instruction snapshot, so updates preserve saved prompts. Current enablement and removal controls apply to old references too. Disabled or uninstalled plugins cause an explicit unavailable-reference error. Old revision references keep their labels but are excluded from new autocomplete selections. Each skill, connection, and hook can be enabled independently. Removing one component preserves its siblings; a later marketplace update can restore components that still exist upstream.

In a chat, Up/Down on the first/last line recalls previously submitted prompts and restores the current draft. Arrow keys select slash suggestions when autocomplete is open. Enter selects a suggestion before it can send a chat; Shift+Enter inserts a newline.

Automations select skills in their task prompt. Continuing automations use the thread's current model and effort, but do not inherit its attached skills. Legacy standalone automation skill IDs remain supported and are migrated into prompt references when edited.

Search filters installed capabilities and the public catalog. Click a plugin name to see the bundled skill count and names. Each skill has one view action that opens the complete instruction body as rendered Markdown. The original `SKILL.md` remains captured byte-for-byte; frontmatter is omitted from the pretty view. Long documents scroll inside the viewer. Public plugin and standalone skill previews use the same validated importer without installing or enabling the package. The list scrolls beneath fixed search controls. Management skills remain available in normal chat; there are no Ask Themis controls. Creating, importing arbitrary sources, configuring, testing and updating integrations happens in chat or the CLI. Icons use a supplied validated HTTPS or bundled SVG asset; packages without one use a fallback.

## Agent skill creation

The built-in creator owns `crates/core/builtins/create-skill/SKILL.md` and `scripts/verify.py`. Its instructions tell the agent to write a JSON draft, run the materialized verifier at `.themis/skills/create-skill/verify.py`, test helper scripts, then call `save_skill`. Management tools are available in every chat, independently of installed plugin activation. Built-in management skills guide plugin, skill, MCP, hook and automation setup; normal approvals still apply. Local scope is the default; global scope must be requested. Backend validation and optimistic revision checks also run on every save.

Run the standalone verifier:

```sh
python3 crates/core/builtins/create-skill/scripts/verify.py /path/to/skill.json
python3 crates/core/builtins/create-skill/scripts/verify.py /path/to/skill-directory
```

It checks required fields, names, instruction/script limits, known tools, duplicate scripts, traversal, and directory symlinks. It does not evaluate whether instructions solve the task. Python 3 is required for this check.

## Marketplace compatibility

The default source is Anthropic's `claude-plugins-official` Git repository; individual packages are installed by the user. Additional HTTPS Git repositories or local marketplace directories can be added. Repository imports support an explicit branch, tag, or commit and safe subdirectory. Personal imports accept local skill/plugin folders, a standalone `SKILL.md`, native JSON bundles, MCP JSON files, and pasted MCP JSON. Archives and JSONC are rejected explicitly. Imports read Claude marketplace/plugin manifests, `SKILL.md`, UTF-8 resources, `.mcp.json`, and command hooks. Imported MCP servers and hooks begin disabled. Source metadata records local paths, repository refs/subdirectories, or marketplace origin. Upstream icons are retained when they are validated HTTPS URLs or bundled SVG resources; unavailable binary icons are reported. Make a personal copy to customize a marketplace package without losing changes on update.

Unsupported components are listed in the plugin detail view. Binary resources, legacy commands, Claude subagents/LSP/settings, external config-file declarations, and restricted tool matchers are not executed. The importer supports simple scalar/block YAML frontmatter. Unsupported source types are rejected rather than silently ignored. MCP imports accept Claude `mcpServers` and OpenCode `mcp` shapes, normalize local command arrays and HTTP URLs, and require `${VARIABLE}` or `{env:VARIABLE}` environment references. Literal credentials, browser OAuth, unsupported transports/headers, and unknown MCP options are rejected. Imported connections remain disabled until explicitly enabled. This is compatibility with supported components, not the full Claude plugin runtime.

## Skill discovery

Themis reads conventional user and project `.agents/skills`, `.codex/skills`, and `.claude/skills` folders automatically. Discovery does not register or change the source folders. Skills with the same display name receive separate source-path identities; identical canonical sources are deduplicated in `.agents`, `.codex`, then `.claude` order. Source directory symlinks are canonicalized for read-only discovery, cycles are skipped, and symlink resources inside a package are rejected explicitly. The scan stops at eight directory levels and 1000 skills. Invalid or oversized skills appear as disabled packages with a compatibility notice. `disable-model-invocation: true` keeps a skill available for explicit slash invocation while excluding it from automatic discovery.

User and project activation overrides persist in Themis. Discovered skills can be disabled; uninstalling their source requires removing the original file. Text reference/script resources are available as per-run copies inside the project sandbox, with no executable permissions granted by discovery. Original absolute `SKILL.md` paths remain available as metadata. Instructions and imported scripts still follow the normal tool approvals when used.

## MCP and hooks

Connections support stdio and Streamable HTTP JSON-RPC, initialization, paginated tool discovery, and tool calls. Configure environment-variable references for stdio and optional bearer authentication; credentials are not stored as literal configuration values. Browser OAuth and legacy SSE transports are not implemented. Enabled connection startup and tool calls use Themis approval gates independently of skill selection. A failed connection is omitted with a sanitized warning; chat and management remain available to repair it. Blocking hooks retain their failure policy.

Canonical hook events are `RunStart`, `BeforeTool`, `AfterTool`, `BeforeCompaction`, and `RunEnd`; legacy event aliases remain supported. Hook matchers select tools; ignore/abort failure policy is explicit. Commands receive a JSON payload through stdin and may return JSON. Before-event hooks can block on failure. Hooks cannot grant tool permissions. Commands have bounded output/time; Unix process groups terminate descendants on timeout/cancellation. Imported resource roots are passed to hooks as `CLAUDE_PLUGIN_ROOT`, never substituted into shell source. Explicit chat/CLI tests run the saved configuration; normal runs request approvals.

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
- Anthropic skills `683bc88e56f3e09ba94f7055977f3d3aa499f202`, `skills/pdf`: one skill and 12 text resources imported. The same repository's skill-creator is explicitly rejected because its 32,805-byte instructions exceed the existing 32,768-byte limit; content is not truncated.
- Official Claude plugin catalog `d4226d062928f8d9505dbdeadd10217d23361052`: catalog read and `plugin-dev` installation passed; seven skills and nine compatibility notices.
- Official filesystem MCP npm package `@modelcontextprotocol/server-filesystem@2026.8.31`: imported disabled, explicitly enabled, launched, negotiated the supported MCP protocol, discovered 14 tools, and read an isolated marker through the actual `read_file` tool.

These checks verify supported imports and executable integration paths; they do not establish complete compatibility with upstream plugin runtimes, all skill workflows, browser OAuth, or current MCP specification revisions.

Standalone skills appear under Skills and are invoked with `/`. Plugins contains real plugin packages (including packages with one bundled skill) and uses `@`. Standalone MCP JSON and hooks stay in their respective categories. Imports record `package_kind`; older imports are classified from preserved source metadata/resources without rewriting the source files.

Plugins and Skills each have Installed and Not installed views. Disabled items remain installed. The initial standalone skill catalog contains pinned Anthropic PDF, Word and spreadsheet skills; other sources remain available through repository/folder imports and normal chat. Plugin/skill rows use supplied package icons, with file-type glyphs when the source supplies no artwork.

The integration browser uses searchable rows, icons, status labels and readable details. Plugin View shows a skill count and one view action per bundled skill. Skills render as formatted Markdown. MCP/hook details summarize their saved status without configuration editors or test buttons. Basic lifecycle controls use clear on/off switches, public Install buttons and confirmed Uninstall actions. Configuration and advanced management run through `/manage-plugins`, `/manage-skills`, `/manage-mcp` and `/manage-hooks` in chat, or the CLI. Automations remains separate with Instructions, Repeat and Time, plus optional controls under Advanced.
